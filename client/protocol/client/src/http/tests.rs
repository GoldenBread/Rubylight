//! Unit tests for [`HostClient`] request targets, launch timeouts, and placeholder box art.

use super::*;
use std::io::{Read, Write};

// Nightjar #88: a launch waited for the PC's answer without any limit.
#[test]
fn a_launch_the_pc_never_answers_fails_with_its_own_words() {
    assert!(LAUNCH_READ_TIMEOUT > READ_TIMEOUT);
    let timed_out = || Error::Io(std::io::ErrorKind::TimedOut.into());
    let unanswered = launch_failure(timed_out(), LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT);
    assert!(unanswered.is_launch_unanswered());
    assert!(
        unanswered.to_string().contains("within 120 seconds"),
        "{unanswered}"
    );
    assert!(
        matches!(&unanswered, Error::Io(e) if e.kind() == std::io::ErrorKind::Other),
        "not a dropped connection a retry would cure"
    );
    let would_block = Error::Io(std::io::ErrorKind::WouldBlock.into());
    assert!(
        launch_failure(would_block, LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT)
            .is_launch_unanswered()
    );
    // A connect that timed out early stays the usual failure.
    let early = launch_failure(timed_out(), Duration::from_secs(5), LAUNCH_READ_TIMEOUT);
    assert!(!early.is_launch_unanswered());
    assert!(matches!(&early, Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut));
    // Anything else is passed on.
    assert!(matches!(
        launch_failure(Error::Cancelled, LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT),
        Error::Cancelled
    ));
    assert!(!Error::Cancelled.is_launch_unanswered());
}

#[test]
fn a_detached_client_still_reaches_the_host_after_a_cancel() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let host = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request);
        socket
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });
    let identity = Arc::new(identity::tests::fixture());
    let client = HostClient::new("127.0.0.1", port, identity)
        .unwrap()
        .with_https_port(47984);
    client.cancel_pending_requests();
    assert!(matches!(
        client.server_info_http(true),
        Err(Error::Cancelled)
    ));
    let detached = client.detached();
    assert_eq!(detached.state.lock().unwrap().https_port, 47984);
    assert!(!matches!(
        detached.server_info_http(true),
        Err(Error::Cancelled)
    ));
    host.join().unwrap();
}

#[test]
fn only_a_second_screens_client_marks_its_requests() {
    let identity = Arc::new(identity::tests::fixture());
    let first = HostClient::new("127.0.0.1", 47989, identity).unwrap();
    let target = first.url_target("launch", Some("appid=1"));
    assert!(target.starts_with("launch?appid=1&uniqueid="), "{target}");
    assert!(!target.contains(SECOND_SCREEN_PARAMETER), "{target}");
    let second = first.detached().with_second_screen();
    for (path, query) in [
        ("launch", Some("appid=1")),
        ("cancel", None),
        ("api/abr", None),
    ] {
        let target = second.url_target(path, query);
        assert!(target.ends_with("&rlSecondScreen=1"), "{target}");
        assert_eq!(
            target.matches(SECOND_SCREEN_PARAMETER).count(),
            1,
            "{target}"
        );
    }
    assert!(
        second
            .detached()
            .url_target("bitrate", None)
            .ends_with("&rlSecondScreen=1")
    );
    assert!(
        !first
            .url_target("bitrate", None)
            .contains(SECOND_SCREEN_PARAMETER)
    );
}

#[test]
fn the_hosts_placeholder_box_art_is_no_cover() {
    let placeholder = include_bytes!("../../tests/fixtures/box-placeholder.png");
    assert!(is_placeholder_box_art(placeholder));
    let mut other = placeholder.to_vec();
    other[600] ^= 1;
    assert!(
        !is_placeholder_box_art(&other),
        "a cover of the same size is a cover"
    );
    assert!(!is_placeholder_box_art(&[]));
    // In the monorepo the fixture is the host's own placeholder.
    let host = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../rust/assets/package/box.png"
    );
    if let Ok(host) = std::fs::read(host) {
        assert_eq!(host, placeholder.to_vec());
    }
}
