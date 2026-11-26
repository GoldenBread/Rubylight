//! Tests for UDP datagram retries, wait budgets, segmented batches, and socket waits.
mod interfaces;
mod reply;

use super::batch::segmented_retries_in_place;
use super::datagram::{
    BUFFER_RETRY_DELAY, SegmentedFailure, WaitBudget, buffer_available, classify_segmented_error,
    send_with_retry, transient, writable,
};
use super::*;
use anyhow::Result;
use std::{net::UdpSocket, os::windows::io::AsRawSocket};
use windows::Win32::Networking::WinSock::*;

#[test]
fn a_refused_datagram_or_segmented_batch_retries_while_the_budget_allows() {
    // Both the audio/datagram path and WSASendMsg use this operation. A
    // refused batch has not been submitted, so retrying it cannot duplicate
    // any successful datagrams.
    for bytes in [128, 44 * 1456] {
        for error in [WSAEWOULDBLOCK, WSAENOBUFS] {
            let mut sends = 0;
            let mut waits = 0;
            let result = send_with_retry(
                || {
                    sends += 1;
                    if sends == 1 { Err(error) } else { Ok(bytes) }
                },
                |reported| {
                    assert_eq!(reported, error);
                    waits += 1;
                    true
                },
            );
            assert_eq!(result, Ok(bytes));
            assert_eq!((sends, waits), (2, 1));
        }
    }
    assert_eq!(
        send_with_retry(|| Ok(128), |_| panic!("a successful send must not wait")),
        Ok(128)
    );
}

#[test]
fn persistent_pressure_and_an_expired_budget_do_not_loop_or_hide_errors() {
    for max_waits in [0, 2] {
        let mut sends = 0;
        let mut waits = 0;
        let result: std::result::Result<usize, _> = send_with_retry(
            || {
                sends += 1;
                Err(WSAENOBUFS)
            },
            |_| {
                waits += 1;
                waits <= max_waits
            },
        );
        assert_eq!(result, Err(WSAENOBUFS));
        assert_eq!(sends, max_waits + 1);
        assert_eq!(waits, max_waits + 1);
    }
    for error in [WSAEHOSTUNREACH, WSAECONNRESET, WSAEINVAL, WSAENOTSOCK] {
        let result: std::result::Result<usize, _> = send_with_retry(
            || Err(error),
            |_| panic!("only buffer pressure permits a retry"),
        );
        assert_eq!(result, Err(error));
    }
}

#[test]
fn one_frame_shares_a_single_wait_for_room_in_the_socket() {
    let ms = std::time::Duration::from_millis;
    let start = std::time::Instant::now();
    // The frame's deadline, not a fresh WRITABLE_WAIT per batch: a busy CPU
    // may keep the socket full for a whole picture period.
    let mut budget = WaitBudget::for_frame(Some(start + ms(20)), None);
    assert_eq!(budget.take(start), 20);
    assert_eq!(budget.take(start + ms(12)), 8);
    assert_eq!(budget.take(start + ms(20)), 0);
    assert!(budget.backoff_at(start + ms(20)).is_none());
    // A frame paced out over most of its period meets its first full socket
    // late: it still gets the usual wait from then, not what is left.
    let mut late = WaitBudget::for_frame(Some(start + ms(20)), None);
    assert_eq!(late.take(start + ms(18)), WRITABLE_WAIT.as_millis() as i32);
    // A later batch of the same frame shares the deadline the first one set.
    let mut next = WaitBudget::for_frame(Some(start + ms(20)), late.deadline);
    assert_eq!(next.take(start + ms(21)), 1);
    assert_eq!(next.take(start + ms(22)), 0);
    // Outside a frame one batch gets the old short wait.
    let mut batch = WaitBudget::default();
    assert_eq!(batch.take(start), WRITABLE_WAIT.as_millis() as i32);
}

/// Review 2026-11-17: one brief refusal spent the frame's whole wait, so a
/// later full socket in the same frame was dropped at once. A batch that
/// drops nothing ends the full-socket spell.
#[test]
fn a_batch_sent_whole_ends_the_frames_full_socket_spell() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let mut batch = Batch::default();
    batch.begin_frame(std::time::Duration::from_millis(20));
    let past = std::time::Instant::now() - std::time::Duration::from_millis(1);
    batch.frame.as_mut().unwrap().deadline = Some(past);
    assert!(batch.frame_wait_spent());
    batch.send(&sender, &[vec![7; 64]], receiver.local_addr()?)?;
    assert_eq!(batch.dropped, 0);
    assert!(!batch.frame_wait_spent());
    assert!(batch.frame.unwrap().deadline.is_none());
    batch.end_frame();
    assert!(batch.frame.is_none());
    Ok(())
}

/// Review 2026-11-17: the retry loop polled a socket that reported writable
/// and refused again at once, spinning for the whole frame wait. From the
/// second refusal on it pauses a millisecond first, so a 20 ms wait is about
/// twenty tries.
#[test]
fn a_socket_that_polls_writable_but_refuses_is_not_spun_on() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut budget = WaitBudget::for_frame(
        Some(std::time::Instant::now() + std::time::Duration::from_millis(20)),
        None,
    );
    let mut sends = 0;
    let started = std::time::Instant::now();
    let result: std::result::Result<usize, _> = send_with_retry(
        || {
            sends += 1;
            Err(WSAEWOULDBLOCK)
        },
        |error| buffer_available(&socket, error, &mut budget, None),
    );
    assert_eq!(result, Err(WSAEWOULDBLOCK));
    assert!(started.elapsed() >= std::time::Duration::from_millis(15));
    assert!((2..=30).contains(&sends), "{sends} tries in 20 ms");
}

#[test]
fn buffer_backoff_consumes_the_same_batch_budget_as_writable_waits() {
    let start = std::time::Instant::now();
    let mut now = start;
    let mut budget = WaitBudget::default();
    let mut retries = 0;
    for _ in 0..64 {
        if let Some(deadline) = budget.backoff_at(now) {
            assert!(deadline > now);
            now = deadline;
            retries += 1;
        }
    }
    assert_eq!(retries, 3);
    assert_eq!(now - start, std::time::Duration::from_millis(3));
    assert_eq!(budget.take(now), 1);
    assert_eq!(budget.take(start + WRITABLE_WAIT), 0);
    assert!(budget.backoff_at(start + WRITABLE_WAIT).is_none());

    // Earlier socket polling also spends the backoff budget. A late timer
    // wake leaves no new retry allowance for later packets in that batch.
    let mut budget = WaitBudget::default();
    assert_eq!(budget.take(start), 4);
    assert!(
        budget
            .backoff_at(start + WRITABLE_WAIT - BUFFER_RETRY_DELAY)
            .is_none()
    );
    assert!(budget.backoff_at(start + WRITABLE_WAIT * 2).is_none());
}

#[test]
fn no_buffer_space_waits_even_when_the_socket_is_already_writable() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut budget = WaitBudget::default();
    let mut waits = WritableWaits::default();
    let retried = buffer_available(&socket, WSAENOBUFS, &mut budget, Some(&mut waits));
    assert_eq!(waits.count, 1);
    assert!(waits.elapsed >= BUFFER_RETRY_DELAY);
    // A busy test machine may wake after the shared deadline; it must then
    // refuse the retry rather than restart the budget.
    assert_eq!(retried, waits.elapsed < WRITABLE_WAIT);
    let mut spent = WaitBudget {
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert!(!buffer_available(
        &socket,
        WSAENOBUFS,
        &mut spent,
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 1);
    assert!(spent.backoff_timer.is_none());
    Ok(())
}

#[test]
fn writable_waits_are_counted_only_when_traced() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(Batch::default().waits.is_none());
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut waits = WritableWaits::default();
    let started = std::time::Instant::now();
    assert!(writable(
        &socket,
        &mut WaitBudget::default(),
        Some(&mut waits)
    ));
    assert!(writable(
        &socket,
        &mut WaitBudget::default(),
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 2);
    assert!(waits.elapsed <= started.elapsed());
    Ok(())
}

#[test]
fn a_batch_shares_one_writable_wait_instead_of_one_per_datagram() -> Result<()> {
    let start = std::time::Instant::now();
    let ms = std::time::Duration::from_micros;
    let mut budget = WaitBudget::default();
    assert_eq!(budget.take(start), 4);
    assert_eq!(budget.take(start + ms(2_500)), 2);
    assert_eq!(budget.take(start + ms(3_999)), 1);
    assert_eq!(budget.take(start + ms(4_000)), 0);
    assert_eq!(budget.take(start + ms(250_000)), 0);
    // A spent budget refuses at once, without polling the socket.
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let mut spent = WaitBudget {
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    let mut waits = WritableWaits::default();
    assert!(!writable(&socket, &mut spent, Some(&mut waits)));
    assert_eq!(waits.count, 0);
    Ok(())
}

#[test]
fn waiting_for_input_wakes_when_a_datagram_arrives() -> Result<()> {
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    assert!(!wait_readable(receiver.as_raw_socket(), 0)?);
    sender.send_to(b"input", receiver.local_addr()?)?;
    let waited = std::time::Instant::now();
    assert!(wait_readable(receiver.as_raw_socket(), 1000)?);
    assert!(waited.elapsed() < std::time::Duration::from_millis(500));
    Ok(())
}

#[test]
fn control_wait_wakes_for_feedback_without_client_input() -> Result<()> {
    let control = UdpSocket::bind("127.0.0.1:0")?;
    let feedback = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let wait =
        |timeout| wait_readable_or_wake(control.as_raw_socket(), feedback.as_raw_socket(), timeout);
    assert!(!wait(0)?);
    let address = feedback.local_addr()?;
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        sender.send_to(&[1], address)
    });
    let started = std::time::Instant::now();
    assert!(wait(1000)?);
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    thread.join().unwrap()?;
    feedback.recv(&mut [0])?;
    assert!(!wait(0)?);
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    sender.send_to(&[1], control.local_addr()?)?;
    assert!(wait(1000)?);
    Ok(())
}

#[test]
fn native_udp_batches_preserve_datagrams_destinations_and_short_tail() -> Result<()> {
    for host in ["127.0.0.1:0", "[::1]:0"] {
        let sender = ReplySocket::bind(host)?;
        let receiver = UdpSocket::bind(host)?;
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let mut packets: Vec<_> = (0..7).map(|i| vec![i; 512]).collect();
        packets.push(vec![91; 13]);
        assert_eq!(Batch::count(&packets, 16384), 8);
        let mut batch = Batch::default();
        // The block's short last datagram rides in the same segmented send.
        assert_eq!(
            batch.send(&sender, &packets, receiver.local_addr()?)?,
            7 * 512 + 13
        );
        assert_eq!(batch.system_calls, 1, "{host}");
        for expected in &packets {
            let mut buffer = [0u8; 2048];
            let (size, peer) = receiver.recv_from(&mut buffer)?;
            assert_eq!(&buffer[..size], expected);
            assert_eq!(peer, sender.local_addr()?);
        }
        println!(
            "{host}: {} datagrams, {} send calls",
            packets.len(),
            batch.system_calls
        );
        // Verify the compatibility path explicitly on the same socket.
        batch.offload = false;
        batch.send(&sender, &packets[..2], receiver.local_addr()?)?;
        for expected in &packets[..2] {
            let mut buffer = [0u8; 512];
            let n = receiver.recv(&mut buffer)?;
            assert_eq!(&buffer[..n], expected);
        }
    }
    Ok(())
}

#[test]
fn a_batch_counts_equal_sizes_then_at_most_one_shorter() {
    let count = |sizes: &[usize], limit: usize| {
        let packets: Vec<Vec<u8>> = sizes.iter().map(|&n| vec![0; n]).collect();
        Batch::count(&packets, limit)
    };
    assert_eq!(count(&[], 16384), 0);
    // Segmentation cannot send an empty datagram, so one stands alone.
    assert_eq!(count(&[0, 0], 16384), 1);
    assert_eq!(count(&[512, 512, 512], 16384), 3);
    // The block's last datagram is shorter: it ends the batch, and stops it.
    assert_eq!(count(&[512, 512, 13, 512], 16384), 3);
    assert_eq!(count(&[512, 13], 16384), 2);
    // An empty or longer datagram is never in the batch.
    assert_eq!(count(&[512, 512, 0], 16384), 2);
    assert_eq!(count(&[512, 512, 513], 16384), 2);
    // At most 64 datagrams, and the limit leaves no slot for a tail.
    assert_eq!(count(&[100; 70], 65507), 64);
    assert_eq!(count(&[512, 512, 13], 1024), 2);
    // At most 65507 bytes, tail included.
    assert_eq!(count(&[1200; 64], 65507), 54);
    assert_eq!(count(&[65507, 1], 65507), 1);
}

#[test]
fn a_segment_run_is_equal_sizes_then_at_most_one_shorter() {
    let run = |sizes: &[usize]| {
        let packets: Vec<Vec<u8>> = sizes.iter().map(|&n| vec![0; n]).collect();
        Segments::run(&packets)
    };
    assert_eq!(run(&[]), 0);
    assert_eq!(run(&[300]), 1);
    assert_eq!(run(&[1200, 1200, 1200]), 3);
    // A shorter datagram ends the run; a longer one starts the next.
    assert_eq!(run(&[1200, 1200, 700, 1200]), 3);
    assert_eq!(run(&[100, 1200, 1200]), 1);
    assert_eq!(run(&[1200, 1300]), 1);
    // At most 64 datagrams and 65507 bytes.
    assert_eq!(run(&[100; 70]), 64);
    assert_eq!(run(&[1200; 64]), 54);
    assert_eq!(run(&[65507, 1]), 1);
    // Segmentation cannot send an empty datagram.
    assert_eq!(run(&[0, 0]), 1);
    assert_eq!(run(&[1200, 1200, 0]), 2);
}

#[test]
fn segments_keep_mixed_datagrams_in_order_in_one_write_per_run() -> Result<()> {
    for host in ["127.0.0.1:0", "[::1]:0"] {
        let sender = UdpSocket::bind(host)?;
        let receiver = UdpSocket::bind(host)?;
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let sizes = [90, 1200, 1200, 1200, 1200, 350, 1100, 1100, 160, 1];
        let packets: Vec<Vec<u8>> = sizes
            .iter()
            .enumerate()
            .map(|(i, &n)| (0..n).map(|b| (i * 31 + b) as u8).collect())
            .collect();
        let peer = receiver.local_addr()?;
        let mut segments = Segments::default();
        let sent = segments.send(&sender, &packets, peer);
        assert_eq!((sent.datagrams, sent.failed), (sizes.len(), 0), "{host}");
        assert_eq!(sent.bytes, sizes.iter().sum::<usize>());
        for expected in &packets {
            let mut buffer = [0u8; 2048];
            let (size, from) = receiver.recv_from(&mut buffer)?;
            assert_eq!(&buffer[..size], expected, "{host}");
            assert_eq!(from, sender.local_addr()?);
        }
        // 90; the four 1200s and their 350; the two 1100s and their 160; 1.
        assert!(segments.offload, "{host}: segmentation stays on");
        assert_eq!(segments.system_calls, 4, "{host}");
        // Without segmentation, one datagram per write, still in order.
        segments.offload = false;
        let sent = segments.send(&sender, &packets[1..6], peer);
        assert_eq!(sent.datagrams, 5);
        assert_eq!(segments.system_calls, 9, "{host}");
        for expected in &packets[1..6] {
            let mut buffer = [0u8; 2048];
            let size = receiver.recv(&mut buffer)?;
            assert_eq!(&buffer[..size], expected, "{host}");
        }
    }
    Ok(())
}

#[test]
#[ignore = "timing: cargo test -p butterpollo-win-io --lib segment_burst_timing -- --ignored --nocapture"]
fn segment_burst_timing() -> Result<()> {
    // A frame's burst over loopback, segmented against one send_to per datagram, alternating:
    // how long the writes take and when the burst's last datagram reaches the receiver.
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Networking::WinSock::{SO_RCVBUF, SOCKET, SOL_SOCKET, setsockopt};
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let buffer = (16i32 << 20).to_ne_bytes();
    let socket = SOCKET(receiver.as_raw_socket() as usize);
    // SAFETY: the socket is open for the call and the option value is a valid i32 buffer.
    let set = unsafe { setsockopt(socket, SOL_SOCKET, SO_RCVBUF, Some(&buffer)) };
    assert_eq!(set, 0);
    let peer = receiver.local_addr()?;
    let (arrived, last) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        let mut count = 0usize;
        while let Ok(size) = receiver.recv(&mut buffer) {
            count += 1;
            if size > 0 && buffer[0] == 1 {
                let _ = arrived.send((Instant::now(), count));
                count = 0;
            }
        }
    });
    let percentile = |values: &mut Vec<f64>, p: f64| {
        values.sort_by(f64::total_cmp);
        values[((values.len() - 1) as f64 * p).round() as usize]
    };
    // A 4K60 P frame at 23 Mbps (48 KB) and a 4K key frame (300 KB), as 1200-byte RTP packets.
    for (name, count) in [("p-frame", 41usize), ("key-frame", 251)] {
        let mut packets = vec![vec![0u8; 1200]; count];
        packets[count - 1] = vec![1u8; 700];
        let mut results: [(Vec<f64>, Vec<f64>); 2] = Default::default();
        let mut segments = Segments::default();
        for trial in 0..4000 {
            let offload = trial % 2 == 0;
            segments.offload = offload;
            let start = Instant::now();
            let sent = segments.send(&sender, &packets, peer);
            let written = start.elapsed();
            let (at, received) = last.recv_timeout(Duration::from_secs(2))?;
            assert_eq!((sent.datagrams, received), (count, count), "{name}");
            assert_eq!(segments.offload, offload, "segmentation stays on");
            let result = &mut results[usize::from(offload)];
            result.0.push(written.as_secs_f64() * 1e6);
            result.1.push(at.duration_since(start).as_secs_f64() * 1e6);
            std::thread::sleep(Duration::from_millis(1));
        }
        for (mode, (written, arrived)) in ["send_to", "segmented"].iter().zip(&mut results) {
            println!(
                "{name:9} {mode:9} write us p50 {:7.1} p99 {:7.1}  last arrival us p50 {:7.1} p90 {:7.1} p99 {:7.1}",
                percentile(written, 0.5),
                percentile(written, 0.99),
                percentile(arrived, 0.5),
                percentile(arrived, 0.9),
                percentile(arrived, 0.99),
            );
        }
    }
    Ok(())
}

#[test]
fn segments_count_a_failed_run_as_lost_and_go_on() -> Result<()> {
    // An IPv4 socket cannot reach an IPv6 peer: every datagram fails, none is fatal, and an
    // address error does not turn segmentation off. The failed run is tried again one datagram
    // at a time, so a passing error loses only the datagrams it hits.
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let packets = vec![vec![7u8; 500]; 5];
    let mut segments = Segments::default();
    let sent = segments.send(&sender, &packets, "[::1]:9".parse()?);
    assert_eq!((sent.datagrams, sent.failed), (0, 5));
    assert!(sent.error.is_some());
    assert!(segments.offload);
    assert_eq!(segments.system_calls, 6);
    Ok(())
}

#[test]
fn transient_send_errors_drop_packets_instead_of_ending_the_stream() -> Result<()> {
    for error in [WSAEWOULDBLOCK, WSAENOBUFS, WSAEHOSTUNREACH, WSAECONNRESET] {
        assert!(transient(error), "{}", error.0);
    }
    for error in [WSAEINVAL, WSAENOTSOCK, WSAEFAULT, WSAEMSGSIZE] {
        assert!(!transient(error), "{}", error.0);
    }
    for code in [WSAENETUNREACH, WSAENETRESET, WSAEMSGSIZE, WSAENOBUFS] {
        assert!(datagram_lost(&std::io::Error::from_raw_os_error(code.0)));
    }
    assert!(!datagram_lost(&std::io::Error::from_raw_os_error(
        WSAENOTSOCK.0
    )));
    // Without SIO_UDP_CONNRESET disabled, the ICMP reply from a closed
    // port turns later sends into WSAECONNRESET on Windows.
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let closed = UdpSocket::bind("127.0.0.1:0")?.local_addr()?;
    let packets = vec![vec![7u8; 256]; 4];
    let mut batch = Batch::default();
    for _ in 0..20 {
        send_datagram(&sender, &packets[0], closed)?;
        batch.send(&sender, &packets, closed)?;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    configure_udp(&sender)?;
    assert!(socket2::SockRef::from(&sender).send_buffer_size()? >= 1 << 20);
    Ok(())
}

#[test]
fn enobufs_budget_gives_wider_recovery_window_without_changing_wouldblock_or_default() {
    let ms = std::time::Duration::from_millis;
    let start = std::time::Instant::now();

    // Default budget keeps WRITABLE_WAIT for both WSAEWOULDBLOCK and WSAENOBUFS.
    let mut default_budget = WaitBudget::default();
    assert_eq!(
        default_budget.deadline_for(start, true),
        start + WRITABLE_WAIT
    );
    assert_eq!(
        default_budget.deadline_for(start, false),
        start + WRITABLE_WAIT
    );

    // Frame budget with separate enobufs_until widens WSAENOBUFS without moving WSAEWOULDBLOCK.
    let mut frame_budget =
        WaitBudget::for_frame_with_enobufs(Some(start + ms(7)), None, Some(start + ms(30)), None);
    assert_eq!(frame_budget.deadline_for(start, false), start + ms(7));
    assert_eq!(frame_budget.deadline_for(start, true), start + ms(30));
    assert!(frame_budget.backoff_for(start + ms(7), false).is_none());
    assert_eq!(
        frame_budget.backoff_for(start + ms(25), true),
        Some(start + ms(26))
    );
    assert!(frame_budget.backoff_for(start + ms(29), true).is_none());
}

#[test]
fn datagram_pass_shares_enobufs_deadline_across_packets() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let peer = receiver.local_addr()?;

    // Healthy sends succeed without allocating any refusal deadline.
    let mut pass = DatagramPass::new(AUDIO_ENOBUFS_WAIT);
    assert!(send_datagram_pass(&sender, &[1, 2, 3], peer, &mut pass)?);
    assert!(pass.deadline.is_none());
    assert!(pass.enobufs_deadline.is_none());

    // Once a pass exhausts its shared ENOBUFS deadline, subsequent packets retry
    // non-blockingly without sleeping again.
    let past = std::time::Instant::now() - std::time::Duration::from_millis(1);
    let mut expired =
        WaitBudget::for_frame_with_enobufs(Some(past), Some(past), Some(past), Some(past));
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let mut waits = WritableWaits::default();
    assert!(!buffer_available(
        &socket,
        WSAENOBUFS,
        &mut expired,
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 0);
    Ok(())
}

#[test]
fn classify_segmented_error_falls_back_on_enobufs_without_disabling_offload() {
    assert_eq!(
        classify_segmented_error(WSAENOBUFS),
        SegmentedFailure::FallbackDatagrams
    );
    for offload_err in [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE] {
        assert_eq!(
            classify_segmented_error(offload_err),
            SegmentedFailure::DisableOffload
        );
    }
    for drop_err in [WSAEWOULDBLOCK, WSAECONNRESET, WSAENETDOWN] {
        assert_eq!(
            classify_segmented_error(drop_err),
            SegmentedFailure::DropBatch
        );
    }
    assert_eq!(classify_segmented_error(WSAEACCES), SegmentedFailure::Fatal);
}

#[test]
fn segmented_enobufs_falls_back_to_datagrams_without_waiting_in_place() {
    assert!(!segmented_retries_in_place(WSAENOBUFS));
    assert!(segmented_retries_in_place(WSAEWOULDBLOCK));
}

#[test]
fn route_unavailable_is_separated_from_socket_backpressure() {
    for error in [
        WSAENETDOWN,
        WSAENETUNREACH,
        WSAENETRESET,
        WSAEHOSTUNREACH,
        WSAEHOSTDOWN,
        WSAEADDRNOTAVAIL,
    ] {
        assert!(datagram::route_unavailable(error), "{}", error.0);
        assert!(transient(error), "{}", error.0);
    }
    for error in [WSAEWOULDBLOCK, WSAENOBUFS, WSAECONNRESET, WSAEMSGSIZE] {
        assert!(!datagram::route_unavailable(error), "{}", error.0);
    }

    let mut batch = Batch::default();
    let start = batch.drops();
    batch.dropped(10, WSAENETUNREACH);
    let route_delta = batch.drops_since(start);
    assert!(route_delta.any());
    assert!(route_delta.route_only());
    assert_eq!(route_delta.dropped, 10);
    assert_eq!(route_delta.route_unavailable, 10);
    assert_eq!(route_delta.refused, 0);

    let mid = batch.drops();
    batch.dropped(4, WSAEWOULDBLOCK);
    let congested = batch.drops_since(mid);
    assert!(!congested.route_only());
    assert_eq!(congested.refused, 4);
    assert_eq!(congested.route_unavailable, 0);

    let combined = batch.drops_since(start);
    assert!(!combined.route_only());
    assert_eq!(combined.dropped, 14);
    assert_eq!(combined.route_unavailable, 10);
    assert_eq!(combined.refused, 4);
}
