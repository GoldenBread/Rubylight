//! Tests for WebRTC pacing queue, simulated loss, batching, queue bounds and guest fan-out.

use super::super::*;
use super::*;
use butterpollo_media_policy::audio::OpusLayout;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

#[test]
fn the_loss_switch_drops_only_video_and_about_the_share_asked_for() {
    let queue = || SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()).with_loss(10);
    let (mut lossy, mut clean) = (
        queue(),
        SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()),
    );
    let video = rtp_packet(1200);
    let lost = (0..100_000)
        .filter(|_| lossy.lost_in_the_air(&video))
        .count();
    assert!((800..=1200).contains(&lost), "{lost} of 100000 at 1%");
    assert!((0..1000).all(|_| !lossy.lost_in_the_air(&control_packet())));
    assert!((0..1000).all(|_| !clean.lost_in_the_air(&video)));
    // The seed is fixed: a second queue loses the same packets.
    let mut second = queue();
    let again = (0..100_000)
        .filter(|_| second.lost_in_the_air(&video))
        .count();
    assert_eq!(again, lost);
}

#[test]
fn simulated_losses_are_counted_and_reported_only_with_the_switch_on() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (handle, _rx, _idr) = detached(TransportOptions {
        loss_permille: 500,
        ..TransportOptions::default()
    });
    let mut queue =
        SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()).with_loss(500);
    let to = receiver.local_addr().unwrap();
    for _ in 0..200 {
        queue.push(Outgoing {
            destination: to,
            ..rtp_packet(100)
        });
    }
    for _ in 0..20 {
        queue.push(Outgoing {
            destination: to,
            ..control_packet()
        });
    }
    send_due(&socket, &mut queue, &mut Batch::default(), &handle.shared);
    let stats = handle.stats();
    assert!(queue.is_empty());
    assert!(
        (50..=150).contains(&stats.simulated_losses),
        "{} of 200 video datagrams at 50%",
        stats.simulated_losses
    );
    assert_eq!(stats.simulated_losses + stats.datagrams_sent, 220);
    assert_eq!(handle.simulated_losses(), Some(stats.simulated_losses));
    let (clean, _rx, _idr) = detached(TransportOptions::default());
    assert_eq!(clean.simulated_losses(), None);
}

#[test]
fn a_batch_goes_out_in_order_in_fewer_writes_than_datagrams() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let receivers: Vec<UdpSocket> = (0..2)
        .map(|_| UdpSocket::bind("127.0.0.1:0").unwrap())
        .collect();
    for receiver in &receivers {
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
    }
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let mut queue = SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now());
    // A frame's packets (equal, its last shorter), control and audio of other sizes around
    // them, and a second peer in the middle: each datagram tagged with its place.
    let sizes = [
        100, 1200, 1200, 1200, 1200, 1200, 1200, 1200, 1200, 700, 160, 1200, 1200, 90,
    ];
    let mut expected: Vec<Vec<Vec<u8>>> = vec![Vec::new(), Vec::new()];
    for (i, &size) in sizes.iter().enumerate() {
        let peer = usize::from(i == 10);
        let mut packet = Outgoing {
            destination: receivers[peer].local_addr().unwrap(),
            ..if (1..10).contains(&i) || (11..13).contains(&i) {
                rtp_packet(size)
            } else {
                Outgoing {
                    data: vec![0x16; size],
                    ..control_packet()
                }
            }
        };
        packet.data[2] = i as u8;
        expected[peer].push(packet.data.clone());
        queue.push(packet);
    }
    let mut batch = Batch::default();
    send_due(&socket, &mut queue, &mut batch, &handle.shared);
    assert!(queue.is_empty());
    for (receiver, expected) in receivers.iter().zip(&expected) {
        for want in expected {
            let mut buffer = [0u8; 2048];
            let (size, _) = receiver.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..size], &want[..], "datagram {}", want[2]);
        }
    }
    let stats = handle.stats();
    assert_eq!(stats.datagrams_sent, sizes.len() as u64);
    assert_eq!(stats.bytes_sent, sizes.iter().sum::<usize>() as u64);
    assert_eq!(stats.send_errors, 0);
    // Where segmentation works (loopback has it on every supported Windows): the lone 100, the
    // frame and its short last, the 160 to the other peer, then the two 1200s and the 90.
    println!(
        "{} datagrams, {} writes",
        sizes.len(),
        batch.segments.system_calls
    );
    assert_eq!(batch.segments.system_calls, 4);
}

#[test]
fn the_pacer_sends_one_millisecond_of_media_per_group() {
    let now = Instant::now();
    // 12 Mbps is 1500 bytes of wire traffic a millisecond: one 1100-byte datagram (1166 on the
    // wire) fits, a second does not.
    let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
    for _ in 0..5 {
        queue.push(rtp_packet(1100));
    }
    queue.push(control_packet());
    let first = queue.next_batch(now);
    assert_eq!(
        first.packets.len(),
        2,
        "the control datagram and one media datagram"
    );
    assert!(!first.packets[0].paced && first.packets[1].paced);
    queue.sent(now, &first);
    assert!(
        queue.next_batch(now).packets.is_empty(),
        "the next group waits"
    );
    let due = queue.due(now).unwrap();
    assert!(
        due > now && due <= now + Duration::from_millis(1),
        "{:?}",
        due - now
    );
    let second = queue.next_batch(due);
    assert_eq!(second.paced_packets, 1);
}

#[test]
fn a_large_frame_raises_the_wireless_pacing_rate_to_what_it_needs() {
    let now = Instant::now();
    let wireless = Pacing::Paced {
        configured_kbps: 0,
        link_bps: 0,
        wireless: true,
    };
    // A 1 Mbps stream is paced at 2 Mbps (one datagram a group) until a frame needs more.
    let packets = |note: Option<usize>| {
        let mut queue = SendQueue::new(wireless, 1_000, PERIOD, false, now);
        if let Some(bytes) = note {
            queue.note_frame(bytes);
        }
        for _ in 0..40 {
            queue.push(rtp_packet(1100));
        }
        queue.next_batch(now).paced_packets
    };
    assert_eq!(packets(None), 1);
    // 60 datagrams of 1166 bytes per frame period is 33.6 Mbps; with the quarter on top, 42.
    assert_eq!(packets(Some(60 * 1166)), 4);
}

#[test]
fn the_ceiling_is_a_real_cap() {
    let now = Instant::now();
    let rate = |pacing: Pacing, frame: usize| {
        let mut queue = SendQueue::new(pacing, 20_000, PERIOD, false, now);
        queue.note_frame(frame);
        queue.rate()
    };
    // A 300 KB frame (144 Mbps at 60 fps) on a 100 Mbps link is held to 4/5 of the link.
    let link = |link_bps, wireless| Pacing::Paced {
        configured_kbps: 0,
        link_bps,
        wireless,
    };
    let huge = 300_000;
    assert_eq!(rate(link(100_000_000, false), huge), 80_000_000);
    assert_eq!(rate(link(100_000_000, true), huge), 80_000_000);
    // No link known: wired 800 Mbps, and wireless is clamped to the same.
    assert_eq!(rate(link(0, false), huge), 800_000_000);
    assert_eq!(rate(link(0, true), huge * 10), 800_000_000);
    // A configured cap holds, whatever the frame needs.
    assert_eq!(rate(capped(50_000), huge), 50_000_000);
    // Never below what the stream itself needs (1.1 times its bitrate).
    assert_eq!(rate(capped(5_000), 0), 22_000_000);
    assert_eq!(rate(Pacing::Off, huge), u64::MAX);
}

#[test]
fn a_late_wake_catches_up_but_a_stall_earns_a_bounded_burst() {
    let now = Instant::now();
    // 12 Mbps with 1100-byte datagrams: one datagram, 0.78 ms of wire time, per group.
    let groups_at = |late: Duration| {
        let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
        for _ in 0..200 {
            queue.push(rtp_packet(1100));
        }
        let first = queue.next_batch(now);
        queue.sent(now, &first);
        // The timer fires `late` after the first group was due again.
        let at = queue.due(now).unwrap() + late;
        let mut sent = 0;
        loop {
            let batch = queue.next_batch(at);
            if batch.paced_packets == 0 {
                return sent;
            }
            queue.sent(at, &batch);
            sent += batch.paced_packets;
        }
    };
    let on_time = groups_at(Duration::ZERO);
    let late_2ms = groups_at(Duration::from_millis(2));
    let stalled = groups_at(Duration::from_millis(500));
    assert_eq!(on_time, 1);
    assert!(
        late_2ms >= 3,
        "a 2 ms late wake sends the groups it missed: {late_2ms}"
    );
    assert!(
        (2..=8).contains(&stalled),
        "a 500 ms stall earns {CATCH_UP_CREDIT:?} of credit, not a burst of 400 groups: {stalled}"
    );
}

#[test]
fn a_backlog_beyond_the_cap_is_detected_but_a_lone_keyframe_is_not() {
    let now = Instant::now();
    // 80 Mbps: 50 ms of backlog is 500 KB.
    let mut queue = SendQueue::new(capped(80_000), 20_000, PERIOD, false, now);
    // A 1 MB keyframe is twice the 50 ms budget, but it is one frame.
    queue.note_frame(1_000_000);
    for _ in 0..860 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    // Two more of them (3 x frame > 2 x frame) is a pile-up.
    for _ in 0..1800 {
        queue.push(rtp_packet(1100));
    }
    assert!(queue.backlog_exceeded());
    assert_eq!(queue.drop_paced(), 2660);
    assert!(!queue.backlog_exceeded() && queue.is_empty());
    // Small frames: the 50 ms budget alone decides.
    queue.note_frame(20_000);
    for _ in 0..400 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    for _ in 0..200 {
        queue.push(rtp_packet(1100));
    }
    assert!(queue.backlog_exceeded(), "{}", queue.paced_wire_bytes);
}

#[test]
fn pacing_off_hands_everything_over_at_once() {
    let now = Instant::now();
    let mut queue = SendQueue::new(Pacing::Off, 0, PERIOD, false, now);
    for _ in 0..50 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    assert_eq!(queue.next_batch(now).packets.len(), 50);
    assert!(queue.is_empty());
}

#[test]
fn idle_gap_advances_due_so_the_next_frame_stays_paced() {
    let now = Instant::now();
    // 12 Mbps: one 1100-byte datagram (~0.78 ms) per group.
    let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
    queue.push(rtp_packet(1100));
    let first = queue.next_batch(now);
    assert_eq!(first.paced_packets, 1);
    queue.sent(now, &first);
    assert!(queue.is_empty());
    // After a 500 ms idle gap, the next frame must still pace one group at a time
    // instead of bursting CATCH_UP_CREDIT worth of groups at once.
    let later = now + Duration::from_millis(500);
    for _ in 0..10 {
        queue.push(rtp_packet(1100));
    }
    let second = queue.next_batch(later);
    assert_eq!(second.paced_packets, 1);
    queue.sent(later, &second);
    assert_eq!(
        queue.next_batch(later).paced_packets,
        0,
        "stale due must not burst after an idle gap"
    );
}

#[test]
fn a_full_frame_queue_drops_and_asks_for_a_keyframe_without_blocking() {
    let (handle, rx, idr) = detached(TransportOptions {
        video_queue: 2,
        ..TransportOptions::default()
    });
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
    // The media thread isn't draining: the third frame is dropped at once.
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::QueueFull]);
    // Later delta frames would reference the dropped one, so they are dropped too.
    while let Ok(Command::Video(_)) = rx.try_recv() {
        handle.shared.pending_video.fetch_sub(1, Ordering::AcqRel);
    }
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(
        handle.submit_video(frame(true)),
        Submit::Queued,
        "a keyframe goes through"
    );
    assert_eq!(
        handle.submit_video(frame(false)),
        Submit::Queued,
        "and delta frames follow it"
    );
    assert_eq!(handle.stats().video_dropped, 2);
}

#[test]
fn audio_queue_overflow_drops_without_blocking() {
    let (handle, _rx, idr) = detached(TransportOptions {
        audio_queue: 1,
        ..TransportOptions::default()
    });
    let packet = || AudioPacket {
        data: vec![1],
        capture: Instant::now(),
        position: 0,
    };
    assert_eq!(handle.submit_audio(packet()), Submit::Queued);
    assert_eq!(handle.submit_audio(packet()), Submit::Dropped);
    assert!(
        idr.lock().unwrap().is_empty(),
        "a lost audio packet needs no keyframe"
    );
    assert_eq!(handle.stats().audio_dropped, 1);
}

#[test]
fn the_audio_sender_shares_the_handles_queue_and_counters() {
    let (handle, rx, _idr) = detached(TransportOptions {
        audio_queue: 1,
        ..TransportOptions::default()
    });
    assert!(
        handle.audio_sender().is_none(),
        "no layout, no audio end: stereo and surround must both say what they encode"
    );
    let layout = OpusLayout::select(6, false, None).unwrap();
    let handle = handle.with_audio_layout(layout.clone());
    let sender = handle.audio_sender().unwrap();
    assert_eq!(sender.layout(), &layout);
    let packet = || AudioPacket {
        data: vec![7],
        capture: Instant::now(),
        position: 0,
    };
    assert_eq!(sender.submit(packet()), Submit::Queued);
    assert_eq!(
        handle.submit_audio(packet()),
        Submit::Dropped,
        "one queue, whichever end fills it"
    );
    assert!(matches!(rx.try_recv(), Ok(Command::Audio(p)) if p.data == [7]));
    assert_eq!(handle.stats().audio_dropped, 1);
    // The sender outlives nothing: with the handle's threads gone a packet is dropped.
    drop(rx);
    handle.shared.pending_audio.store(0, Ordering::Release);
    assert_eq!(sender.submit(packet()), Submit::Dropped);
    assert_eq!(handle.stats().audio_dropped, 2);
}

fn queued_video(rx: &mpsc::Receiver<Command>) -> Vec<VideoFrame> {
    let mut frames = Vec::new();
    while let Ok(command) = rx.try_recv() {
        if let Command::Video(frame) = command {
            frames.push(frame);
        }
    }
    frames
}

#[test]
fn without_guests_a_frame_keeps_its_own_buffer() {
    let (owner, rx, _idr) = detached(TransportOptions::default());
    assert_eq!(owner.submit_video(frame(true)), Submit::Queued);
    let frames = queued_video(&rx);
    assert!(
        matches!(
            frames.as_slice(),
            [VideoFrame {
                data: FrameData::Owned(_),
                ..
            }]
        ),
        "no guests, no shared copy: the owner's path is the one it had before guests"
    );
}

#[test]
fn a_slow_guest_drops_frames_alone_and_the_owner_keeps_every_one() {
    let (owner, owner_rx, owner_idr) = detached(TransportOptions {
        video_queue: 8,
        ..TransportOptions::default()
    });
    let (slow, slow_rx, slow_idr) = detached(TransportOptions {
        video_queue: 1,
        ..TransportOptions::default()
    });
    let (fast, fast_rx, _fast_idr) = detached(TransportOptions {
        video_queue: 8,
        ..TransportOptions::default()
    });
    owner.followers().add(1, slow.feed());
    owner.followers().add(2, fast.feed());
    assert_eq!(owner.submit_video(frame(true)), Submit::Queued);
    for _ in 0..4 {
        assert_eq!(
            owner.submit_video(frame(false)),
            Submit::Queued,
            "a guest that cannot keep up never costs the owner a frame"
        );
    }
    assert_eq!(owner.stats().video_dropped, 0);
    assert!(owner_idr.lock().unwrap().is_empty(), "nor a keyframe");
    assert_eq!(slow.stats().video_dropped, 4);
    assert_eq!(
        slow_idr.lock().unwrap().first(),
        Some(&IdrReason::QueueFull),
        "the slow guest asks for its own"
    );

    let owner_frames = queued_video(&owner_rx);
    let slow_frames = queued_video(&slow_rx);
    let fast_frames = queued_video(&fast_rx);
    assert_eq!(owner_frames.len(), 5);
    assert_eq!(
        slow_frames.len(),
        1,
        "the keyframe, then its queue was full"
    );
    assert_eq!(
        fast_frames.len(),
        5,
        "one slow guest does not hold back another"
    );
    let (FrameData::Shared(a), FrameData::Shared(b)) =
        (&owner_frames[0].data, &slow_frames[0].data)
    else {
        panic!("a frame with guests is shared");
    };
    assert!(
        Arc::ptr_eq(a, b),
        "one buffer for every transport, not a copy each"
    );
}

#[test]
fn a_guest_that_left_gets_nothing_more() {
    let (owner, _owner_rx, _owner_idr) = detached(TransportOptions::default());
    let (guest, guest_rx, _guest_idr) = detached(TransportOptions::default());
    let followers = owner.followers();
    followers.add(7, guest.feed());
    followers.add(7, guest.feed());
    assert_eq!(followers.len(), 1, "joining twice is one follower");
    owner.submit_video(frame(true));
    assert!(followers.remove(7));
    assert!(!followers.remove(7));
    owner.submit_video(frame(true));
    assert_eq!(queued_video(&guest_rx).len(), 1);
}

#[test]
fn audio_reaches_every_guest_and_a_full_guest_queue_drops_only_there() {
    let (owner, owner_rx, _) = detached(TransportOptions::default());
    let (guest, guest_rx, _) = detached(TransportOptions {
        audio_queue: 1,
        ..TransportOptions::default()
    });
    let owner = owner.with_audio_layout(OpusLayout::select(2, false, None).unwrap());
    owner.followers().add(1, guest.feed());
    let sender = owner.audio_sender().unwrap();
    let packet = |n: u8| AudioPacket {
        data: vec![n],
        capture: Instant::now(),
        position: 0,
    };
    assert_eq!(sender.submit(packet(1)), Submit::Queued);
    assert_eq!(owner.submit_audio(packet(2)), Submit::Queued);
    assert_eq!(owner.stats().audio_dropped, 0);
    assert_eq!(guest.stats().audio_dropped, 1);
    let audio = |rx: &mpsc::Receiver<Command>| -> Vec<Vec<u8>> {
        rx.try_iter()
            .filter_map(|c| match c {
                Command::Audio(p) => Some(p.data),
                _ => None,
            })
            .collect()
    };
    assert_eq!(audio(&owner_rx), [vec![1], vec![2]]);
    assert_eq!(audio(&guest_rx), [vec![1]]);
}

#[test]
fn a_shared_av1_frame_loses_its_temporal_delimiters_once() {
    let frame = av1_frame(true, 3000);
    let shared = FrameData::Owned(frame.clone())
        .share(VideoCodec::Av1)
        .unwrap();
    assert_eq!(&shared[..], without_td(&frame).as_slice());
    let again = FrameData::Shared(shared.clone())
        .share(VideoCodec::Av1)
        .unwrap();
    assert!(
        Arc::ptr_eq(&shared, &again),
        "a shared frame is ready as it is"
    );
}
