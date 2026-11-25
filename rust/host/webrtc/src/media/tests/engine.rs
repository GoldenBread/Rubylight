//! Tests for WebRTC media engine video/audio packetization, DataChannels, RTX and AV1 OBUs.

use super::super::*;
use super::*;
use std::sync::atomic::Ordering;
use str0m::media::KeyframeRequestKind;

#[test]
fn video_arrives_byte_identical_for_every_codec() {
    type Frame = fn(bool, usize) -> Vec<u8>;
    let cases: [(VideoCodec, Frame, bool); 4] = [
        (VideoCodec::H264, h264_frame, false),
        (VideoCodec::Hevc, hevc_frame, false),
        (VideoCodec::Hevc, hevc_frame, true),
        (VideoCodec::Av1, av1_frame, false),
    ];
    for (codec, make, hdr) in cases {
        let mut link = Link::connected(spec(codec, hdr));
        let sent = [make(true, 30_000), make(false, 4_000), make(false, 900)];
        for (i, frame) in sent.iter().enumerate() {
            link.video(frame.clone(), 16 * i as u64, i == 0);
            link.step(Duration::from_millis(16));
        }
        link.run(Duration::from_millis(50));
        let expected: Vec<Vec<u8>> = sent
            .iter()
            .map(|f| {
                if codec == VideoCodec::Av1 {
                    without_td(f)
                } else {
                    f.clone()
                }
            })
            .collect();
        let video: Vec<&Received> = link.media.iter().filter(|m| m.pt == 96).collect();
        assert_eq!(video.len(), expected.len(), "{codec:?}: frames received");
        for (i, (got, want)) in video.iter().zip(&expected).enumerate() {
            assert!(
                got.data == *want,
                "{codec:?} frame {i}: {}",
                diff(&got.data, want)
            );
        }
    }
}

#[test]
fn marker_bit_is_on_the_last_packet_of_each_frame_only() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 30_000), 0, true);
    link.step(Duration::from_millis(16));
    link.video(h264_frame(false, 5_000), 16, false);
    link.run(Duration::from_millis(50));
    let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
    let mut frames: Vec<Vec<&Rtp>> = Vec::new();
    for p in video {
        match frames.last_mut() {
            Some(f) if f[0].timestamp == p.timestamp => f.push(p),
            _ => frames.push(vec![p]),
        }
    }
    assert_eq!(frames.len(), 2);
    assert!(
        frames[0].len() > 20,
        "a 30 KB keyframe is many packets: {}",
        frames[0].len()
    );
    for f in &frames {
        let (last, rest) = f.split_last().unwrap();
        assert!(last.marker, "the last packet of a frame carries the marker");
        assert!(rest.iter().all(|p| !p.marker), "no other packet does");
        assert!(
            f.windows(2).all(|w| w[1].seq == w[0].seq.wrapping_add(1)),
            "sequence numbers are gapless"
        );
    }
}

#[test]
fn rtp_timestamps_follow_the_capture_instant() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // Captured 0, 7 and 24 ms after the session start, but all handed over in one go.
    for (ms, key) in [(0, true), (7, false), (24, false)] {
        link.video(h264_frame(key, 2_000), ms, key);
    }
    link.run(Duration::from_millis(50));
    let mut stamps: Vec<u32> = Vec::new();
    for p in link.rtp.iter().filter(|p| p.pt == 96) {
        if stamps.last() != Some(&p.timestamp) {
            stamps.push(p.timestamp);
        }
    }
    assert_eq!(stamps.len(), 3);
    assert_eq!(
        stamps[1].wrapping_sub(stamps[0]),
        7 * 90,
        "7 ms is 630 ticks of 90 kHz"
    );
    assert_eq!(stamps[2].wrapping_sub(stamps[1]), 17 * 90);
    let times: Vec<u64> = link
        .media
        .iter()
        .filter(|m| m.pt == 96)
        .map(|m| m.time.numer())
        .collect();
    assert_eq!(times.len(), 3);
    assert_eq!(times[2] - times[0], 24 * 90);
}

#[test]
fn every_video_packet_asks_for_no_playout_delay() {
    let mut link = Link::connected(spec(VideoCodec::Hevc, true));
    link.video(hevc_frame(true, 10_000), 0, true);
    link.run(Duration::from_millis(30));
    let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
    assert!(!video.is_empty());
    for p in video {
        let (min, max) = p
            .playout
            .expect("the playout-delay extension is on every packet");
        assert!(min.is_zero() && max.is_zero());
    }
}

#[test]
fn hdr_colour_space_is_never_sent() {
    let mut link = Link::connected(spec(VideoCodec::Hevc, true));
    link.video(hevc_frame(true, 10_000), 0, true);
    link.run(Duration::from_millis(30));
    // Offered (the SDP test) but str0m has no serializer for it, and the playout delay and mid
    // extensions are there, so the block parsing is real.
    assert!(
        link.wire_ext_ids.contains(&6) && link.wire_ext_ids.contains(&4),
        "{:?}",
        link.wire_ext_ids
    );
    assert!(!link.wire_ext_ids.contains(&9), "{:?}", link.wire_ext_ids);
}

#[test]
fn nothing_is_written_before_the_peer_is_connected() {
    let mut link = Link::new(spec(VideoCodec::H264, false));
    let frame = VideoFrame {
        data: h264_frame(true, 1_000).into(),
        capture: link.at(0),
        keyframe: true,
    };
    assert!(!link.host.write_video(link.now, frame).unwrap());
    let packet = AudioPacket {
        data: vec![1, 2, 3],
        capture: link.at(0),
        position: 0,
    };
    assert!(!link.host.write_audio(link.now, packet).unwrap());
}

#[test]
fn connecting_asks_for_a_keyframe() {
    let link = Link::connected(spec(VideoCodec::H264, false));
    assert!(link.idr.contains(&IdrReason::Connected), "{:?}", link.idr);
}

#[test]
fn pli_and_fir_reach_the_idr_callback() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 3_000), 0, true);
    link.run(Duration::from_millis(30));
    link.idr.clear();
    let mid = link.client_video.expect("video negotiated");
    for kind in [KeyframeRequestKind::Pli, KeyframeRequestKind::Fir] {
        link.client
            .direct_api()
            .stream_rx_by_mid(mid, None)
            .expect("receiving video")
            .request_keyframe(kind);
        link.run(Duration::from_millis(30));
    }
    assert!(link.idr.contains(&IdrReason::Pli), "{:?}", link.idr);
    assert!(link.idr.contains(&IdrReason::Fir), "{:?}", link.idr);
}

#[test]
fn opus_packets_pass_through_untouched() {
    for spec in [
        spec(VideoCodec::H264, false),
        surround_spec(6),
        surround_spec(8),
    ] {
        let channels = spec.audio.channels;
        let mut link = Link::connected(spec);
        // Not valid Opus: the transport must not look inside.
        let packets: Vec<Vec<u8>> = [120, 300, 640]
            .iter()
            .enumerate()
            .map(|(i, n)| filler(*n, 20 + i as u8))
            .collect();
        for (i, p) in packets.iter().enumerate() {
            link.audio(p.clone(), 10 * i as u64, 480 * i as u64);
            link.step(Duration::from_millis(10));
        }
        link.run(Duration::from_millis(40));
        let got: Vec<&Vec<u8>> = link
            .media
            .iter()
            .filter(|m| m.pt == 111)
            .map(|m| &m.data)
            .collect();
        assert_eq!(got.len(), packets.len(), "{channels} channels");
        for (g, p) in got.iter().zip(&packets) {
            assert!(*g == p, "{channels} channels: {}", diff(g, p));
        }
    }
}

/// The RTP times of the Opus packets the client has received so far.
fn audio_times(link: &Link) -> Vec<u64> {
    link.media
        .iter()
        .filter(|m| m.pt == 111)
        .map(|m| m.time.numer())
        .collect()
}

#[test]
fn audio_packets_stamped_with_one_capture_instant_still_advance_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // 5 ms CELT packets (240 samples) read in pairs, as the audio thread does from 10 ms WASAPI chunks.
    let packet = |n: u8| vec![0xE8, n, n];
    for i in 0..8u64 {
        link.audio(packet(i as u8), 10 * (i / 2), 240 * i);
        link.step(Duration::from_millis(5));
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 8);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_late_wake_that_drops_nothing_delivers_its_packets_contiguously() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    // The thread wakes 25 ms late, under the capture's 30 ms trim limit, and reads the five
    // packets that built up at once: one capture instant, five consecutive positions.
    for i in 1..=5u64 {
        link.audio(packet(i as u8), 30, 240 * i);
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 6);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_late_wake_that_trimmed_its_backlog_is_a_gap_as_long_as_the_trim() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    // 100 ms late: the capture trims 90 ms (4320 samples) of the 100 ms waiting and keeps two
    // packets. The audio thread adds the trim to the position, so the receiver conceals it,
    // and the RTP time is back in step with the wall (105 ms in, 5040 ticks).
    link.audio(packet(1), 105, 240 + 4320);
    link.audio(packet(2), 105, 240 + 4320 + 240);
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 3);
    assert_eq!(times[1] - times[0], 240 + 4320, "{times:?}");
    assert_eq!(times[2] - times[1], 240, "{times:?}");
}

#[test]
fn lost_audio_is_a_gap_of_exactly_its_length_at_any_size() {
    // Around the old 40 ms wall-time threshold: every gap, long or short, is what was lost.
    for lost in [0u64, 1, 1919, 1920, 1921, 9600] {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let packet = |n: u8| vec![0xE8, n, n];
        link.audio(packet(0), 0, 0);
        link.step(Duration::from_millis(5));
        link.audio(packet(1), 5, 240 + lost);
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 2, "{lost}");
        assert_eq!(times[1] - times[0], 240 + lost, "{lost}: {times:?}");
    }
}

#[test]
fn a_packet_dropped_upstream_still_advances_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    // The queue was full, or the peer not yet connected, for the packets at 240 and 480:
    // the audio thread numbered them anyway.
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    link.audio(packet(3), 20, 720);
    link.step(Duration::from_millis(5));
    link.audio(packet(4), 25, 960);
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 3);
    assert_eq!(times[1] - times[0], 720, "{times:?}");
    assert_eq!(times[2] - times[1], 240, "{times:?}");
}

#[test]
fn packets_the_host_cannot_measure_do_not_repeat_a_timestamp() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // A one-byte code 3 packet has no frame count: nothing in it says how long it is.
    for i in 0..4u64 {
        link.audio(vec![0xEB], 5 * i, 240 * i);
        link.step(Duration::from_millis(5));
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 4);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_slow_device_clock_over_an_hour_never_jumps_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // The device delivers 48 kHz samples 0.05% slower than wall time (1.8 s an hour), with
    // one packet sampled every 3 s of audio, ten times what the old wall-time rule
    // tolerated before resyncing. RTP time must differ by exactly the sample position.
    let step = 144_000u64;
    let packets = 1200u64;
    for i in 0..packets {
        let position = i * step;
        let wall_ms = position / 48 * 10_005 / 10_000;
        link.audio(vec![0xE8, 1, 2], wall_ms, position);
        for _ in 0..60 {
            link.step(Duration::from_millis(50));
        }
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len() as u64, packets);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], step, "{pair:?}");
    }
}

#[test]
fn control_and_gamepad_channels_carry_messages_both_ways() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    assert!(
        link.host
            .send_control(b"{\"type\":\"hello\"}", false)
            .unwrap()
    );
    assert!(link.host.send_gamepad(&[1, 2, 3, 4], true).unwrap());
    link.run(Duration::from_millis(30));
    assert!(
        link.channel_data
            .contains(&("control".into(), b"{\"type\":\"hello\"}".to_vec()))
    );
    assert!(
        link.channel_data
            .contains(&("gamepad".into(), vec![1, 2, 3, 4]))
    );

    link.client_writes("control", b"{\"type\":\"idr\"}");
    link.client_writes("gamepad", &[9, 8, 7]);
    link.run(Duration::from_millis(30));
    assert!(link.host_events.contains(&TransportEvent::Control {
        data: b"{\"type\":\"idr\"}".to_vec(),
        binary: true
    }));
    assert!(link.host_events.contains(&TransportEvent::Gamepad {
        data: vec![9, 8, 7],
        binary: true
    }));
}

#[test]
fn the_input_channel_is_accepted_only_when_authenticated() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("input".into())),
        "{:?}",
        link.host_events
    );
    assert!(
        !link
            .host_events
            .contains(&TransportEvent::ChannelOpen("input".into()))
    );
    assert!(
        link.closed_by_host.contains(&"input".to_string()),
        "the refused channel is closed"
    );

    link.authenticated.store(true, Ordering::Release);
    let input = link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelOpen("input".into())),
        "{:?}",
        link.host_events
    );
    link.client_writes_to(input, &[5, 6, 7]);
    link.run(Duration::from_millis(30));
    assert!(link.host_events.contains(&TransportEvent::Input {
        data: vec![5, 6, 7],
        binary: true
    }));

    // Revoking the session closes the channel on the next message.
    link.authenticated.store(false, Ordering::Release);
    link.client_writes_to(input, &[1]);
    link.run(Duration::from_millis(30));
    assert!(!link.host_events.contains(&TransportEvent::Input {
        data: vec![1],
        binary: true
    }));
    let rejected = link
        .host_events
        .iter()
        .filter(|e| **e == TransportEvent::ChannelRejected("input".into()))
        .count();
    assert_eq!(rejected, 2, "the revoked channel is refused and closed");
}

#[test]
fn unknown_channel_labels_are_closed() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.authenticated.store(true, Ordering::Release);
    link.client_opens("shell");
    // A second channel that claims a host label is refused too.
    link.client_opens("control");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("shell".into())),
        "{:?}",
        link.host_events
    );
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("control".into())),
        "{:?}",
        link.host_events
    );
    assert!(link.closed_by_host.contains(&"shell".to_string()));
}

fn nack_run(rtx: bool) -> (Vec<Rtp>, usize) {
    let mut s = spec(VideoCodec::H264, false);
    s.rtx = rtx;
    let mut link = Link::connected(s);
    link.drop_rtp = Some(link.rtp_seen + 3);
    link.video(h264_frame(true, 20_000), 0, true);
    link.run(Duration::from_millis(600));
    let frames = link.media.iter().filter(|m| m.pt == 96).count();
    (link.rtp, frames)
}

#[test]
fn a_lost_packet_is_resent_only_when_rtx_is_negotiated() {
    let (with_rtx, _) = nack_run(true);
    assert!(
        with_rtx.iter().any(|p| p.pt == 97),
        "with RTX the NACK is answered"
    );

    // Without an RTX payload type str0m marks every packet non-nackable, so a NACK would be
    // ignored; the offer doesn't even negotiate NACK, so receivers send a PLI instead.
    let (without, frames) = nack_run(false);
    assert!(
        without.iter().all(|p| p.pt == 96),
        "no retransmission without RTX"
    );
    let mut seqs: Vec<u16> = without.iter().map(|p| p.seq).collect();
    let total = seqs.len();
    seqs.dedup();
    assert_eq!(seqs.len(), total, "no duplicate sequence numbers");
    assert_eq!(frames, 0, "the frame with the hole is never completed");
}

#[test]
fn av1_temporal_delimiters_are_removed_and_the_rest_kept() {
    let strip = |frame: &[u8]| strip_av1_temporal_delimiters(frame.to_vec()).unwrap();
    let keyframe = av1_frame(true, 300);
    assert_eq!(strip(&keyframe), &keyframe[2..]);
    let plain = obu(6, &filler(200, 1));
    assert_eq!(strip(&plain), plain);
    // A TD in the middle (two temporal units in one buffer) goes too.
    let two = [plain.clone(), TD.to_vec(), plain.clone()].concat();
    assert_eq!(strip(&two), [plain.clone(), plain].concat());
    // An OBU without a size field takes the rest.
    let tail = [obu(1, &[1, 2, 3]), vec![(6 << 3), 9, 9, 9]].concat();
    assert_eq!(strip(&tail), tail);
    // Malformed input is an error, not a guess.
    assert!(strip_av1_temporal_delimiters(vec![0x32, 0x40, 1, 2]).is_err());
    assert!(strip_av1_temporal_delimiters(vec![0x80, 0]).is_err());
}

#[test]
fn av1_frames_keep_their_buffer_when_delimiters_go() {
    // The frame moves on to str0m in the buffer the encoder filled: no copy.
    for frame in [av1_frame(true, 4_000), obu(6, &filler(4_000, 1))] {
        let at = frame.as_ptr();
        let stripped = strip_av1_temporal_delimiters(frame).unwrap();
        assert_eq!(stripped.as_ptr(), at);
    }
}

#[test]
fn multi_frame_drain_reports_each_emitted_frames_wire_bytes_to_the_pacer() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // Queue a tiny delta frame F1 and a huge keyframe F2 back-to-back before `advance()`,
    // reproducing `media_loop` draining multiple `Command::Video` messages together.
    let small = h264_frame(false, 400);
    let big = h264_frame(true, 180_000);
    link.video(small, 0, false);
    link.video(big, 16, true);

    let mut queue = SendQueue::new(
        Pacing::Paced {
            configured_kbps: 20_000,
            link_bps: 0,
            wireless: true,
        },
        20_000,
        Duration::from_millis(16),
        false,
        link.now,
    );

    // Advance repeatedly (just like `media_loop`), draining `queue` as packets go out.
    let mut saw_large_frame = false;
    for _ in 0..10 {
        let _ = link.host.advance(link.now).unwrap();
        let frame_wire = link.host.take_frame_wire_bytes();
        if frame_wire > 0 {
            queue.note_frame(frame_wire);
            if frame_wire >= 150_000 {
                saw_large_frame = true;
            }
        }
        let mut paced_count = 0;
        for out in link.host.drain_outgoing() {
            if out.paced {
                paced_count += 1;
            }
            queue.push(out);
        }
        // A lone large keyframe must NEVER trip `backlog_exceeded` even after F1 drained `queue`.
        assert!(
            !queue.backlog_exceeded(),
            "lone keyframe tripped backlog_exceeded (frame_wire={frame_wire}, paced_wire={})",
            queue.paced_wire_bytes
        );
        if paced_count > 0 {
            // Drain all queued packets (simulating F1 finishing before F2 is emitted next tick).
            while let Some(due) = queue.due(link.now) {
                link.now = link.now.max(due);
                let batch = queue.next_batch(link.now);
                queue.sent(link.now, &batch);
            }
        }
        link.now += Duration::from_millis(1);
    }
    assert!(
        saw_large_frame,
        "expected F2's large wire-byte count to be reported on its own advance() turn"
    );
}
