//! Unit tests for RTSP negotiation, error sanitization, and session warnings.

use super::*;
use butterpollo_session::session::Warnings;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn a_codec_refusal_clears_when_a_retry_asks_for_an_available_codec() {
    let warnings = Warnings::default();
    note_codec_refusal(&warnings, Some("AV1 encoding is not available.".into()));
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].code, "codec_unavailable");
    assert_eq!(shown[0].message, "AV1 encoding is not available.");
    note_codec_refusal(&warnings, None);
    assert!(warnings.snapshot().is_empty());
    // Clearing with nothing shown is harmless.
    note_codec_refusal(&warnings, None);
    assert!(warnings.snapshot().is_empty());
}

fn pyrowave_stream(bitrate_kbps: u32, configured_kbps: u32) -> Negotiated {
    Negotiated {
        codec: 3,
        width: 3840,
        height: 2160,
        fps: 120,
        rate_millihz: 120_000,
        bitrate_kbps,
        configured_bitrate_kbps: configured_kbps,
        ..Negotiated::default()
    }
}

#[test]
fn a_pyrowave_stream_below_the_test_scene_level_is_warned_about_not_refused() {
    let warnings = Warnings::default();
    note_pyrowave_minimum(&warnings, &pyrowave_stream(799_000, 800_000));
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].code, "pyrowave_minimum");
    assert!(
        shown[0].message.contains("1493 Mbps"),
        "{}",
        shown[0].message
    );
    assert!(
        shown[0].message.contains("799 Mbps"),
        "{}",
        shown[0].message
    );
    // At the floor the client asked for, the picture's few Mbps to sound and control do not
    // count as below it: the game runs, with no warning.
    note_pyrowave_minimum(&warnings, &pyrowave_stream(1_489_420, 1_492_992));
    assert!(warnings.snapshot().is_empty());
    // Another codec clears a warning left by an earlier negotiation.
    note_pyrowave_minimum(&warnings, &pyrowave_stream(799_000, 800_000));
    let mut hevc = pyrowave_stream(799_000, 800_000);
    hevc.codec = 1;
    note_pyrowave_minimum(&warnings, &hevc);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn the_floor_uses_the_whole_frame_rate_not_the_fractional_one() {
    // 59.94 fps must be judged at 60 fps, as the client's minimum is; a stream between the two
    // floors is warned about.
    let mut stream = Negotiated {
        codec: 3,
        width: 1920,
        height: 1080,
        fps: 60,
        rate_millihz: 59_940,
        bitrate_kbps: 186_500,
        configured_bitrate_kbps: 186_500,
        ..Negotiated::default()
    };
    let warnings = Warnings::default();
    note_pyrowave_minimum(&warnings, &stream);
    assert_eq!(warnings.snapshot().len(), 1);
    // The 60 fps floor itself passes.
    stream.bitrate_kbps = 186_624;
    stream.configured_bitrate_kbps = 186_624;
    note_pyrowave_minimum(&warnings, &stream);
    assert!(warnings.snapshot().is_empty());
}

#[tokio::test]
async fn rejected_rtsp_requests_sanitize_wire_errors_and_ignore_unoffered_mic_setup() {
    let f = crate::state::test_support::Fixture::new();
    let h = f.host.clone();
    let base_port = (25000u16..55000)
        .step_by(20)
        .find(|&base| std::net::UdpSocket::bind(("127.0.0.1", base)).is_ok())
        .unwrap();
    *h.config.write().unwrap() = butterpollo_config::config::Config::parse(&format!(
        "port = {base_port}\nstream_mic = false\n"
    ))
    .unwrap();
    let mut launch = f.launch(f.client(0), butterpollo_session::session::Role::Stream);
    launch.peer = "127.0.0.1".parse().unwrap();
    launch.rtsp_encrypted = false;
    let launch_id = launch.id.clone();
    let client_uuid = launch.client.uuid.clone();
    h.sessions.lock().unwrap().queue(launch).unwrap();

    let media = crate::stream::Media::new(h.clone(), "127.0.0.1".parse().unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (socket, peer) = tokio::join!(tokio::net::TcpStream::connect(addr), async {
        listener.accept().await.unwrap()
    });
    let mut client = socket.unwrap();
    let (server_sock, peer) = peer;
    let configs = Arc::new(Mutex::new(HashMap::new()));
    let mics = Arc::new(Mutex::new(HashSet::new()));

    let server_task = tokio::spawn(connection(
        server_sock,
        peer,
        tokio::time::Instant::now(),
        h.clone(),
        media,
        configs,
        mics,
    ));

    let req = format!(
        "SETUP rtsp://127.0.0.1:{}/streamid=mic RTSP/1.0\r\nCSeq: 1\r\n\r\n",
        addr.port()
    );
    client.write_all(req.as_bytes()).await.unwrap();

    let mut reply = vec![0u8; 1024];
    let n = client.read(&mut reply).await.unwrap();
    let text = String::from_utf8_lossy(&reply[..n]);
    assert!(text.starts_with("RTSP/1.0 400 Bad Request\r\n"), "{text}");
    assert!(
        !text.contains("unknown stream setup target"),
        "internal error message leaked on wire: {text}"
    );
    assert!(
        h.last_failure(&client_uuid).is_none(),
        "unoffered mic SETUP must not record a launch failure"
    );

    drop(client);
    let _ = server_task.await;
    let _ = h.sessions.lock().unwrap().pending.remove(&launch_id);
}

#[test]
fn link_notice_control_message_is_offered_only_when_host_bitrate_adaptation_is_enabled() {
    for (text, expected) in [
        ("", true),
        ("bitrate_adaptation = on\n", true),
        ("bitrate_adaptation = delay\n", true),
        ("bitrate_adaptation = off\n", false),
        ("link_rate_adaptation = false\n", false),
    ] {
        let config = butterpollo_config::config::Config::parse(text).unwrap();
        assert_eq!(offers_link_notice(&config), expected, "{text:?}");
        let sdp = rtsp::describe_with_link_notice(
            0,
            7,
            1,
            true,
            true,
            true,
            false,
            true,
            offers_link_notice(&config),
        );
        assert_eq!(sdp.contains("0x5538"), expected, "{text:?}: {sdp}");
        assert!(sdp.contains("0x5537"), "{text:?}: {sdp}");
    }
}
