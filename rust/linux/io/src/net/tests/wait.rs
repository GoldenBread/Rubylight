//! Wait budgets, retries and error classes of a send into a full socket.
use super::super::{
    AUDIO_ENOBUFS_WAIT, Batch, DatagramPass, ReplySocket, WRITABLE_WAIT, WritableWaits,
    batch::{SegmentedFailure, classify_segmented_error, segmented_retries_in_place},
    datagram_lost, send_datagram_pass,
    wait::{
        BUFFER_RETRY_DELAY, WaitBudget, buffer_available, send_with_retry, transient, writable,
    },
};
use anyhow::Result;
use std::{
    net::UdpSocket,
    time::{Duration, Instant},
};

fn ms(count: u64) -> Duration {
    Duration::from_millis(count)
}

#[test]
fn a_refused_datagram_or_segmented_batch_retries_while_the_budget_allows() {
    // A refused batch has not been submitted, so retrying it cannot duplicate datagrams.
    for bytes in [128, 44 * 1456] {
        for error in [libc::EAGAIN, libc::ENOBUFS] {
            let (mut sends, mut waits) = (0, 0);
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
        let (mut sends, mut waits) = (0, 0);
        let result: Result<usize, _> = send_with_retry(
            || {
                sends += 1;
                Err(libc::ENOBUFS)
            },
            |_| {
                waits += 1;
                waits <= max_waits
            },
        );
        assert_eq!(result, Err(libc::ENOBUFS));
        assert_eq!((sends, waits), (max_waits + 1, max_waits + 1));
    }
    for error in [
        libc::EHOSTUNREACH,
        libc::ECONNRESET,
        libc::EINVAL,
        libc::ENOTSOCK,
    ] {
        let result: Result<usize, _> = send_with_retry(
            || Err(error),
            |_| panic!("only buffer pressure permits a retry"),
        );
        assert_eq!(result, Err(error));
    }
}

#[test]
fn one_frame_shares_a_single_wait_for_room_in_the_socket() {
    let start = Instant::now();
    // The frame's deadline, not a fresh WRITABLE_WAIT per batch.
    let mut budget = WaitBudget::for_frame(Some(start + ms(20)), None);
    assert_eq!(budget.take(start), 20);
    assert_eq!(budget.take(start + ms(12)), 8);
    assert_eq!(budget.take(start + ms(20)), 0);
    assert!(budget.backoff_at(start + ms(20)).is_none());
    // A frame paced out over most of its period meets its first full socket late: it still
    // gets the usual wait from then, not what is left.
    let mut late = WaitBudget::for_frame(Some(start + ms(20)), None);
    assert_eq!(late.take(start + ms(18)), WRITABLE_WAIT.as_millis() as i32);
    // A later batch of the same frame shares the deadline the first one set.
    let mut next = WaitBudget::for_frame(Some(start + ms(20)), late.deadline);
    assert_eq!(next.take(start + ms(21)), 1);
    assert_eq!(next.take(start + ms(22)), 0);
    // Outside a frame one batch gets the plain short wait.
    assert_eq!(
        WaitBudget::default().take(start),
        WRITABLE_WAIT.as_millis() as i32
    );
}

#[test]
fn a_batch_shares_one_writable_wait_instead_of_one_per_datagram() -> Result<()> {
    let start = Instant::now();
    let us = Duration::from_micros;
    let mut budget = WaitBudget::default();
    assert_eq!(budget.take(start), 4);
    assert_eq!(budget.take(start + us(2_500)), 2);
    assert_eq!(budget.take(start + us(3_999)), 1);
    assert_eq!(budget.take(start + us(4_000)), 0);
    assert_eq!(budget.take(start + us(250_000)), 0);
    // A spent budget refuses at once, without polling the socket.
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let mut spent = WaitBudget::default();
    spent.deadline = Some(Instant::now());
    let mut waits = WritableWaits::default();
    assert!(!writable(&socket, &mut spent, Some(&mut waits)));
    assert_eq!(waits.count, 0);
    Ok(())
}

#[test]
fn buffer_backoff_consumes_the_same_batch_budget_as_writable_waits() {
    let start = Instant::now();
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
    assert_eq!(now - start, ms(3));
    assert_eq!(budget.take(now), 1);
    assert_eq!(budget.take(start + WRITABLE_WAIT), 0);
    assert!(budget.backoff_at(start + WRITABLE_WAIT).is_none());
    // Earlier socket polling also spends the backoff budget.
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
fn enobufs_budget_gives_a_wider_window_without_changing_eagain_or_the_default() {
    let start = Instant::now();
    let mut default_budget = WaitBudget::default();
    assert_eq!(
        default_budget.deadline_for(start, true),
        start + WRITABLE_WAIT
    );
    assert_eq!(
        default_budget.deadline_for(start, false),
        start + WRITABLE_WAIT
    );
    let mut frame =
        WaitBudget::for_frame_with_enobufs(Some(start + ms(7)), None, Some(start + ms(30)), None);
    assert_eq!(frame.deadline_for(start, false), start + ms(7));
    assert_eq!(frame.deadline_for(start, true), start + ms(30));
    assert!(frame.backoff_for(start + ms(7), false).is_none());
    assert_eq!(
        frame.backoff_for(start + ms(25), true),
        Some(start + ms(26))
    );
    assert!(frame.backoff_for(start + ms(29), true).is_none());
}

/// A socket that polls writable but refuses is paused on from its second refusal, not spun on.
#[test]
fn a_socket_that_polls_writable_but_refuses_is_not_spun_on() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let mut budget = WaitBudget::for_frame(Some(Instant::now() + ms(20)), None);
    let mut sends = 0;
    let started = Instant::now();
    let result: Result<usize, _> = send_with_retry(
        || {
            sends += 1;
            Err(libc::EAGAIN)
        },
        |error| buffer_available(&socket, error, &mut budget, None),
    );
    assert_eq!(result, Err(libc::EAGAIN));
    // Slow wakes only lower the count of tries; the pause bounds it from above.
    assert!(started.elapsed() >= ms(10));
    assert!((2..=40).contains(&sends), "{sends} tries in 20 ms");
    Ok(())
}

#[test]
fn no_buffer_space_waits_even_when_the_socket_is_already_writable() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut budget = WaitBudget::default();
    let mut waits = WritableWaits::default();
    let retried = buffer_available(&socket, libc::ENOBUFS, &mut budget, Some(&mut waits));
    assert_eq!((waits.count, waits.last_error), (1, libc::ENOBUFS));
    assert!(waits.elapsed >= BUFFER_RETRY_DELAY);
    // A busy machine may wake after the shared deadline; it must then refuse the retry
    // rather than restart the budget.
    assert_eq!(retried, waits.elapsed < WRITABLE_WAIT);
    let mut spent = WaitBudget::default();
    spent.deadline = Some(Instant::now());
    assert!(!buffer_available(
        &socket,
        libc::ENOBUFS,
        &mut spent,
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 1);
    Ok(())
}

#[test]
fn writable_waits_are_counted_only_when_traced() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(Batch::default().waits.is_none());
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut waits = WritableWaits::default();
    let started = Instant::now();
    for _ in 0..2 {
        assert!(writable(
            &socket,
            &mut WaitBudget::default(),
            Some(&mut waits)
        ));
    }
    assert_eq!(waits.count, 2);
    assert!(waits.elapsed <= started.elapsed());
    Ok(())
}

#[test]
fn datagram_pass_shares_the_enobufs_deadline_across_packets() -> Result<()> {
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let peer = receiver.local_addr()?;
    // Healthy sends succeed without allocating any refusal deadline.
    let mut pass = DatagramPass::new(AUDIO_ENOBUFS_WAIT);
    assert!(send_datagram_pass(&sender, &[1, 2, 3], peer, &mut pass)?);
    assert!(pass.deadline.is_none() && pass.enobufs_deadline.is_none());
    // Once a pass exhausted its shared ENOBUFS deadline, later packets do not sleep again.
    let past = Instant::now() - ms(1);
    let mut expired =
        WaitBudget::for_frame_with_enobufs(Some(past), Some(past), Some(past), Some(past));
    let mut waits = WritableWaits::default();
    assert!(!buffer_available(
        &receiver,
        libc::ENOBUFS,
        &mut expired,
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 0);
    Ok(())
}

#[test]
fn transient_send_errors_drop_packets_instead_of_ending_the_stream() {
    for error in [
        libc::EAGAIN,
        libc::ENOBUFS,
        libc::EHOSTUNREACH,
        libc::ECONNRESET,
    ] {
        assert!(transient(error), "{error}");
    }
    for error in [libc::EINVAL, libc::ENOTSOCK, libc::EFAULT, libc::EMSGSIZE] {
        assert!(!transient(error), "{error}");
    }
    for code in [
        libc::ENETUNREACH,
        libc::ENETRESET,
        libc::EMSGSIZE,
        libc::ENOBUFS,
    ] {
        assert!(datagram_lost(&std::io::Error::from_raw_os_error(code)));
    }
    assert!(datagram_lost(&std::io::ErrorKind::WouldBlock.into()));
    assert!(!datagram_lost(&std::io::Error::from_raw_os_error(
        libc::ENOTSOCK
    )));
}

#[test]
fn classify_segmented_error_falls_back_on_enobufs_without_disabling_offload() {
    assert_eq!(
        classify_segmented_error(libc::ENOBUFS),
        SegmentedFailure::FallbackDatagrams
    );
    for error in [
        libc::EINVAL,
        libc::ENOPROTOOPT,
        libc::EOPNOTSUPP,
        libc::EMSGSIZE,
    ] {
        assert_eq!(
            classify_segmented_error(error),
            SegmentedFailure::DisableOffload
        );
    }
    for error in [libc::EAGAIN, libc::ECONNRESET, libc::ENETDOWN] {
        assert_eq!(classify_segmented_error(error), SegmentedFailure::DropBatch);
    }
    assert_eq!(
        classify_segmented_error(libc::EACCES),
        SegmentedFailure::Fatal
    );
    assert!(!segmented_retries_in_place(libc::ENOBUFS));
    assert!(segmented_retries_in_place(libc::EAGAIN));
}
