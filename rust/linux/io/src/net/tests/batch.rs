//! Batching, UDP segmentation, single datagrams and loss counting over loopback.
use super::super::{
    Batch, DatagramWait, ReplySocket, Segments, configure_udp, send_datagram,
    send_datagram_with_wait,
};
use anyhow::Result;
use std::{
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

/// The loopback addresses to test on; IPv6 only where this machine has it.
fn hosts() -> Vec<&'static str> {
    let mut hosts = vec!["127.0.0.1:0"];
    if UdpSocket::bind("[::1]:0").is_ok() {
        hosts.push("[::1]:0");
    }
    hosts
}

fn receiver(host: &str) -> Result<UdpSocket> {
    let receiver = UdpSocket::bind(host)?;
    receiver.set_read_timeout(Some(Duration::from_secs(5)))?;
    Ok(receiver)
}

#[test]
fn native_udp_batches_preserve_datagrams_destinations_and_short_tail() -> Result<()> {
    for host in hosts() {
        let sender = ReplySocket::bind(host)?;
        let receiver = receiver(host)?;
        let mut packets: Vec<_> = (0..7).map(|i| vec![i; 512]).collect();
        packets.push(vec![91; 13]);
        assert_eq!(Batch::count(&packets, 16384), 8);
        let mut batch = Batch::default();
        // The block's short last datagram rides in the same segmented send.
        let sent = batch.send(&sender, &packets, receiver.local_addr()?)?;
        assert_eq!(sent, 7 * 512 + 13);
        assert_eq!((batch.system_calls, batch.sent_packets()), (1, 8), "{host}");
        for expected in &packets {
            let mut buffer = [0u8; 2048];
            let (size, peer) = receiver.recv_from(&mut buffer)?;
            assert_eq!(&buffer[..size], expected);
            assert_eq!(peer, sender.local_addr()?);
        }
        // The compatibility path on the same socket.
        batch.offload = false;
        batch.send(&sender, &packets[..2], receiver.local_addr()?)?;
        assert_eq!(batch.system_calls, 3);
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
    for host in hosts() {
        let sender = UdpSocket::bind(host)?;
        let receiver = receiver(host)?;
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
fn segments_count_a_failed_run_as_lost_and_go_on() -> Result<()> {
    // An IPv4 socket cannot reach an IPv6 peer: every datagram fails, none is fatal, and an
    // address error does not turn segmentation off. The failed run is tried again one
    // datagram at a time.
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
fn a_batch_sent_whole_ends_the_frames_full_socket_spell() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let mut batch = Batch::default();
    batch.begin_frame(Duration::from_millis(20));
    let past = Instant::now() - Duration::from_millis(1);
    if let Some(frame) = batch.frame.as_mut() {
        frame.deadline = Some(past);
    }
    assert!(batch.frame_wait_spent());
    batch.send(&sender, &[vec![7; 64]], receiver.local_addr()?)?;
    assert_eq!(batch.dropped, 0);
    assert!(!batch.frame_wait_spent());
    assert!(batch.frame.is_some_and(|frame| frame.deadline.is_none()));
    batch.end_frame();
    assert!(batch.frame.is_none());
    Ok(())
}

#[test]
fn single_datagrams_arrive_from_the_socket_they_were_sent_on() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let receiver = receiver("127.0.0.1:0")?;
    let peer = receiver.local_addr()?;
    assert!(send_datagram(&sender, b"audio", peer)?);
    assert!(send_datagram_with_wait(
        &sender,
        b"again",
        peer,
        &mut DatagramWait::default()
    )?);
    assert_eq!(sender.send_to(b"control", peer)?, 7);
    for expected in [&b"audio"[..], b"again", b"control"] {
        let mut buffer = [0u8; 16];
        let (size, from) = receiver.recv_from(&mut buffer)?;
        assert_eq!(&buffer[..size], expected);
        assert_eq!(from, sender.local_addr()?);
    }
    Ok(())
}

/// Linux does not tell an unconnected socket about the ICMP answer of a closed port, so the
/// sends to one are neither errors nor drops.
#[test]
fn sends_to_a_closed_port_neither_fail_nor_count_as_dropped() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let closed = UdpSocket::bind("127.0.0.1:0")?.local_addr()?;
    let packets = vec![vec![7u8; 256]; 4];
    let mut batch = Batch::default();
    for _ in 0..10 {
        assert!(send_datagram(&sender, &packets[0], closed)?);
        batch.send(&sender, &packets, closed)?;
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(batch.dropped, 0);
    Ok(())
}

#[test]
fn an_oversized_datagram_is_counted_as_oversized_but_still_dropped() {
    let mut batch = Batch::default();
    batch.dont_fragment = true;
    batch.too_large(2, libc::EMSGSIZE);
    assert_eq!((batch.dropped, batch.oversized), (2, 2));
    batch.refuse(3);
    assert_eq!(
        (batch.dropped, batch.oversized, batch.sent_packets()),
        (5, 2, 0)
    );
}

#[test]
fn configure_udp_asks_for_the_four_megabyte_send_buffer() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    configure_udp(&socket)?;
    // An unprivileged process gets what net.core.wmem_max allows, which the kernel doubles.
    let limit = std::fs::read_to_string("/proc/sys/net/core/wmem_max")
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let wanted = super::super::SEND_BUFFER_DEFAULT.min(2 * limit);
    assert!(socket2::SockRef::from(&socket).send_buffer_size()? >= wanted);
    Ok(())
}

#[test]
fn a_datagram_larger_than_the_receive_buffer_is_reported_as_lost() -> Result<()> {
    let host = ReplySocket::bind("127.0.0.1:0")?;
    host.set_read_timeout(Some(Duration::from_secs(5)))?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let target: SocketAddr = host.local_addr()?;
    sender.send_to(&[1; 100], target)?;
    let error = host.receive(&mut [0u8; 10]).expect_err("truncated");
    assert!(super::super::datagram_lost(&error), "{error}");
    sender.send_to(&[2; 5], target)?;
    let (size, from, local) = host.receive(&mut [0u8; 10])?;
    assert_eq!((size, from), (5, sender.local_addr()?));
    assert_eq!(local, Some([127, 0, 0, 1].into()));
    Ok(())
}
