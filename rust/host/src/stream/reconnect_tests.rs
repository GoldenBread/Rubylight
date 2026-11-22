//! GPU-free integration matrix through the real HTTP, RTSP, ENet and session
//! workers. Only native preparation/input and capture/encoder output are fake.
//! No service, driver, environment switch or production port is used.
use super::*;
use butterpollo_config::config::Ports;
use butterpollo_session::session::Transport;
use butterpollo_session::{
    mic,
    state::{App, Client as PairedClient},
};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod clipboard_sync_tests;
mod control_fuzz_tests;
mod extension_tests;
mod frame_warp_tests;
mod rate_change_tests;
mod roaming_tests;
mod session_end_tests;
mod webrtc_tests;

pub(super) use super::reconnect_harness::{
    CLAIM, CLOSED, Client, FAILED, Harness, LossySocket, PORT_SLOTS, WAIT, address, claim_slot,
    established, opened_rtsp, port_base, reserve, sealed_rtsp, udp, until, xml,
};

#[test]
fn port_bases_start_per_process_and_stay_in_range() {
    assert_eq!(port_base(0, 0), 22000);
    assert_eq!(port_base(7, 0), 22000 + 7 * 32);
    assert_eq!(port_base(PORT_SLOTS - 1, 1), 22000);
    let bases: std::collections::HashSet<_> = (0..PORT_SLOTS).map(|i| port_base(300, i)).collect();
    assert_eq!(bases.len(), usize::from(PORT_SLOTS));
    assert!(bases.iter().all(|b| (22000..40000 - 31).contains(b)));
}

/// While one harness holds a slot's claim, another run checking that slot skips it, even
/// before the host has bound any of the slot's ports.
#[test]
fn a_claimed_slot_is_skipped_until_its_claim_is_dropped() {
    let (ports, claim) = (0..PORT_SLOTS)
        .find_map(|i| claim_slot(port_base(150, i)))
        .expect("a free slot");
    let used = |p: Ports| {
        [
            p.http, p.https, p.web, p.rtsp, p.video, p.control, p.audio, p.mic,
        ]
    };
    let base = ports.http;
    assert!(!used(ports).contains(&(base + CLAIM)));
    assert!(!used(Ports::from_base(base + 32)).contains(&(base + CLAIM)));
    assert!(
        reserve(ports).is_ok(),
        "the claim holds none of the host's ports"
    );
    assert!(claim_slot(base).is_none());
    drop(claim);
    assert!(claim_slot(base).is_some());
}

/// A client can send its ENet connect before the host binds the control port. The port
/// unreachable that comes back loses that datagram; the client's worker goes on and connects on
/// ENet's retry.
#[test]
fn the_client_socket_outlives_a_datagram_to_a_closed_port() -> Result<()> {
    let closed = udp().local_addr()?;
    let mut socket = LossySocket {
        socket: udp(),
        blocked: Arc::default(),
        moved: Arc::default(),
    };
    rusty_enet::Socket::send(&mut socket, closed, b"connect")?;
    let mut buffer = [0; rusty_enet::MTU_MAX];
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        assert!(rusty_enet::Socket::receive(&mut socket, &mut buffer)?.is_none());
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_during_launch_or_resume_releases_unaccepted_preparation() -> Result<()> {
    for resume in [false, true] {
        let h = Harness::new(10_000, 1).await?;
        if !resume {
            let previous = h.h.current_app.lock().unwrap().take();
            drop(previous);
        }
        let undo_marker = h.directory.join("rejected-launch-prep.txt");
        if !resume {
            // with_environment runs the real prep/undo lifecycle. These are
            // hidden cmd.exe built-ins writing only inside this test's TMP
            // directory; no app window, input or user process is involved.
            h.h.apps.write().unwrap()[0]
                .prep
                .push(butterpollo_session::state::PrepCommand {
                    r#do: format!("echo prepared>\"{}\"", undo_marker.display()),
                    undo: format!("echo undone>>\"{}\"", undo_marker.display()),
                    // CI administrators may have no linked non-elevated
                    // token. Use the existing runner context, as the process
                    // lifecycle fixtures do; never request UAC elevation.
                    elevated: butterpollo_windows::process::user_environment().is_err(),
                    extra: Default::default(),
                });
        }
        let previous_generation =
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone());
        h.fixture
            .disconnect_during_preparation
            .store(true, Ordering::Release);
        let response = h.launch_response(0, resume).await?;
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(response.contains("disconnected or changing"), "{response}");
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"));
        assert_eq!(h.fixture.session_checks.load(Ordering::Acquire), 2);
        assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
        assert_eq!(
            h.fixture.apps_prepared.load(Ordering::Acquire),
            usize::from(!resume)
        );
        assert_eq!(
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone()),
            previous_generation
        );
        assert!(h.h.app_audio.lock().unwrap().is_none());
        if !resume {
            assert_eq!(
                std::fs::read_to_string(&undo_marker)?
                    .lines()
                    .collect::<Vec<_>>(),
                ["prepared", "undone"],
                "rejecting a newly prepared app must execute its real RAII undo"
            );
        }
        h.empty().await?;

        h.fixture
            .disconnect_during_preparation
            .store(false, Ordering::Release);
        h.fixture
            .session_disconnected
            .store(false, Ordering::Release);
        let retry = h.launch(0, resume).await?;
        let (session, client) = h.play(&retry).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_before_play_rejects_only_the_pending_attempt() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let live_launch = h.launch(0, false).await?;
    let (session, client) = h.play(&live_launch).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let pending = h.launch(1, false).await?;
    h.rtsp(
        &pending,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture
        .session_disconnected
        .store(true, Ordering::Release);
    let error = h.rtsp(&pending, 2, "PLAY", "").await.unwrap_err();
    assert!(
        error.to_string().contains("disconnected or changing"),
        "{error:#}"
    );
    // This test keeps an extra launch identity clone; real RTSP snapshots
    // deliberately do not own its preparation. Release the test's clone too.
    drop(pending);
    h.counts(1, 0).await?;
    assert!(!session.stopping());
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.fixture.audios.load(Ordering::Acquire), 1);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // HTTP already accepted this app; a later PLAY rejection must preserve
    // its normal resumable lifecycle and must not stop another client's stream.
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_rtsp_socket_releases_preparation_before_replacement() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    drop(launch);
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let abandoned = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("abandoned RTSP connection authorized", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let replacement = h.launch(0, true).await?;
    assert_eq!(
        h.fixture.displays.load(Ordering::Acquire),
        1,
        "canceled preparation still owned while the replacement prepares its display"
    );
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    let (session, client) = h.play(&replacement).await?;
    drop(abandoned);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_rtsp_cleanup_preserves_a_new_launch_announcement() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let older_launch = h.launch(0, false).await?;
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let older_connection = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("older RTSP connection reached cleanup", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let launch = h.launch(1, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    until("older RTSP cleanup resumed", WAIT, || {
        !h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;
    // The old connection can now reach its read only after pruning. Its EOF
    // response establishes that cleanup completed before this PLAY.
    let mut older_connection = older_connection;
    older_connection.shutdown().await?;
    let mut response = Vec::new();
    older_connection.read_to_end(&mut response).await?;

    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch)?;
    until("newly announced session is streaming", WAIT, || {
        client.frames.load(Ordering::Acquire) > 2
            && session.stats.idr_requests.load(Ordering::Acquire) > 0
    })
    .await;
    let canceled =
        h.h.sessions
            .lock()
            .unwrap()
            .request_stop(Some(&older_launch.id));
    drop((canceled, older_launch));
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_during_control_connect_preserves_the_host_termination_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.rtsp(&launch, 2, "PLAY", "").await?;
    h.fixture
        .fail_on_control_connect
        .store(true, Ordering::Release);
    let client = Client::new(h.ports, launch)?;
    until("early failure reason delivered", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    drop(client);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_media_pings_wake_idle_sockets() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    // A ping the client's worker sent after this one would take the video peer back.
    client.stop_video_pings().await;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    for port in [h.ports.video, h.ports.audio] {
        socket.send_to(b"PING", address(port))?;
    }
    let peer = socket.local_addr()?;
    until("legacy audio and video pings registered", WAIT, || {
        let peers = h.media.as_ref().unwrap().peers.lock().unwrap();
        [false, true]
            .into_iter()
            .all(|audio| peers.get(&launch.id, audio) == Some(peer))
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_rtsp_requests_receive_an_authenticated_error_response() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    for (sequence, method, target, body, message) in [
        (
            1,
            "PLAY",
            "streamid=video",
            "",
            "ANNOUNCE required before PLAY",
        ),
        (
            2,
            "SETUP",
            "streamid=unknown",
            "",
            "unknown stream setup target",
        ),
        (
            3,
            "ANNOUNCE",
            "streamid=video",
            "a=x-nv-video[0].clientViewportWd:1\r\n",
            "",
        ),
    ] {
        let response = h
            .rtsp_at(&launch, sequence, method, target, body)
            .await
            .unwrap_err()
            .to_string();
        assert!(response.starts_with("RTSP/1.0 400"), "{response}");
        assert!(response.contains(message), "{response}");
    }
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&launch.id));
    drop(canceled);
    drop(launch);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_resume_says_it_came_for_the_running_game() -> Result<()> {
    use crate::display_session::RESUMES_GAME;

    let h = Harness::new(10_000, 1).await?;
    // A client that sends the name itself is not asking for anything: a plain
    // launch keeps a display of its own.
    let launch = h
        .launch_with(0, false, None, &[(RESUMES_GAME, "1")])
        .await?;
    assert_eq!(launch.options.get(RESUMES_GAME), None);
    let resumed = h.launch(0, true).await?;
    assert_eq!(
        resumed.options.get(RESUMES_GAME).map(String::as_str),
        Some("1")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_within_ping_timeout() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    drop(client);
    assert!(!old.stopping());
    let resumed = h.launch(0, true).await?;
    assert_ne!(resumed.id, old.launch.id);
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    let (session, client) = h.play(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_after_ping_timeout() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play_short_timeout(&launch).await?;
    drop(client);
    until("vanished client times out", WAIT, || old.stopping()).await;
    assert_eq!(old.termination_reason(), FAILED);
    h.empty().await?;
    let resumed = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

async fn network_drop(seconds: u64, timeout_ms: u32, survives: bool) -> Result<()> {
    let h = Harness::new(timeout_ms, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let lost_at = Instant::now();
    client.network(true);
    if !survives {
        until(
            "outage ends the session within ping_timeout plus worker scheduling allowance",
            Duration::from_millis(u64::from(timeout_ms)) + WAIT,
            || session.stopping(),
        )
        .await;
        assert_eq!(session.termination_reason(), FAILED);
        h.empty().await?;
    }
    tokio::time::sleep(Duration::from_secs(seconds).saturating_sub(lost_at.elapsed())).await;
    assert!(h.fixture.input_releases.load(Ordering::Acquire) > 0);
    if survives {
        assert!(
            !session.stopping(),
            "{seconds}s outage ended the session within ping_timeout"
        );
        let before = session.stats.idr_requests.load(Ordering::Acquire);
        let frames = client.frames.load(Ordering::Acquire);
        client.network(false);
        until("control and video resume after packet loss", WAIT, || {
            session.stats.idr_requests.load(Ordering::Acquire) > before
                && client.frames.load(Ordering::Acquire) > frames
        })
        .await;
        h.counts(1, 0).await?;
        h.end(&session, client).await?;
    } else {
        assert!(
            session.stopping(),
            "{seconds}s outage exceeded ping_timeout without ending"
        );
        assert_eq!(session.termination_reason(), FAILED);
        drop(client);
        h.empty().await?;
        let resumed = h.launch(0, true).await?;
        let (session, client) = h.play(&resumed).await?;
        h.end(&session, client).await?;
    }
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_2s_survives_default_timeout() -> Result<()> {
    network_drop(2, 10_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_10s_survives_15s_timeout() -> Result<()> {
    network_drop(10, 15_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_40s_times_out_then_resumes() -> Result<()> {
    network_drop(40, 10_000, false).await
}

/// A client hears of a lowered bitrate (0x5538) only after it asks, at most once a
/// second, newest first, and hears the return to the requested rate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_notices_reach_only_a_client_that_asks() -> Result<()> {
    use butterpollo_media_policy::link_notice::{
        LINK_NOTICE_MESSAGE_TYPE, LinkNotice, LinkNoticeRequest, LinkReason, LinkState,
    };
    let limited = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 100_000,
        current_kbps,
    };
    let heard = |client: &Client| -> Vec<(Instant, LinkNotice)> {
        client
            .notices
            .lock()
            .unwrap()
            .iter()
            .map(|(at, payload)| (*at, LinkNotice::decode(payload).expect("a valid notice")))
            .collect()
    };
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;

    // Nobody asked: a lowered bitrate is the host's business.
    session.link_notices.note(limited(75_000));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        heard(&client).is_empty(),
        "a notice went to a client that did not ask"
    );

    // Asking brings the standing limit at once.
    let asked = Instant::now();
    client.send_control(
        LINK_NOTICE_MESSAGE_TYPE,
        &LinkNoticeRequest { enabled: true }.encode(),
    );
    until("the standing limit arrives", WAIT, || {
        heard(&client).len() == 1
    })
    .await;
    assert_eq!(heard(&client)[0].1, limited(75_000));

    // Three changes in a burst: the newest alone follows, a second after the first. The first
    // left the host after the request did; a busy client may hear it late, so measure from
    // the request.
    for kbps in [60_000, 50_000, 40_000] {
        session.link_notices.note(limited(kbps));
    }
    until("the newest arrives", WAIT * 2, || heard(&client).len() == 2).await;
    let notices = heard(&client);
    assert_eq!(notices[1].1, limited(40_000));
    assert!(
        notices[1].0.duration_since(asked) >= Duration::from_millis(900),
        "{:?} from the request to the second notice",
        notices[1].0.duration_since(asked)
    );

    // The return to the requested rate clears it, and nothing else follows.
    session.link_notices.note_full(100_000);
    until("the clear arrives", WAIT * 2, || heard(&client).len() == 3).await;
    let clear = heard(&client)[2].1;
    assert_eq!(clear.state, LinkState::Full);
    assert_eq!(clear.reason, LinkReason::None);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(heard(&client).len(), 3);
    h.end(&session, client).await?;
    h.finish().await
}

/// The text field a focus watcher would see.
fn text_field(
    input_type: butterpollo_input_policy::text_input::InputType,
) -> butterpollo_input_policy::text_input::Focused {
    use butterpollo_input_policy::text_input::{Field, Focused, Surrounding};
    Focused {
        field: Field {
            input_type,
            sensitive: false,
            no_autocorrect: false,
            no_autocapitalize: false,
            enter_action: false,
        },
        element: 1,
        surrounding: Some(Surrounding {
            text: "hello world".into(),
            selection: Some((5, 5)),
        }),
    }
}

/// The text input notices (0x553B) `client` heard.
fn text_notices(client: &Client) -> Vec<butterpollo_input_policy::text_input::TextInputNotice> {
    client
        .text_notices
        .lock()
        .unwrap()
        .iter()
        .map(|(_, payload)| {
            butterpollo_input_policy::text_input::TextInputNotice::decode(payload)
                .expect("a valid notice")
        })
        .collect()
}

/// Keyboard follows focus (0x553B): a client that never asks hears nothing, one that asks
/// hears the field that has the focus, a lower once it left text fields, and nothing after
/// it stops asking. Over plain control the text around the cursor stays on the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_notices_reach_only_a_client_that_asks() -> Result<()> {
    use butterpollo_input_policy::text_input::{
        InputType, NoticeKind, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    let h = Harness::configured(10_000, 1, "text_input_surrounding_text = enabled\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;

    // Stock Moonlight never asks: the focus stays the host's business.
    session
        .text_input
        .note(Some(text_field(InputType::Text)), Instant::now());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(text_notices(&client).is_empty());
    assert!(h.fixture.text_watches.lock().unwrap().is_empty());

    let ask = |notices: bool| TextInputRequest {
        notices,
        surrounding_text: true,
        element_ids: false,
        max_text: 64,
    };
    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &ask(true).encode());
    until("the watcher starts", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    // Plain control: no text, though the client asked and the host allows it.
    assert_eq!(
        *h.fixture.text_watches.lock().unwrap(),
        [Watch::On { max_text: 0 }]
    );
    session
        .text_input
        .note(Some(text_field(InputType::Email)), Instant::now());
    until("the raise arrives", WAIT, || {
        text_notices(&client).len() == 1
    })
    .await;
    let raise = &text_notices(&client)[0];
    assert_eq!(
        (raise.kind, raise.input_type),
        (NoticeKind::Raise, InputType::Email)
    );
    assert_eq!(raise.text, "");
    assert_eq!(raise.element, 0, "the client did not ask for element ids");

    session.text_input.note(None, Instant::now());
    until("the lower arrives", WAIT, || {
        text_notices(&client).len() == 2
    })
    .await;
    let lower = &text_notices(&client)[1];
    assert_eq!(lower.kind, NoticeKind::Lower);
    assert!(lower.serial > raise.serial);

    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &ask(false).encode());
    until("the watcher stops", WAIT, || {
        h.fixture.text_watches.lock().unwrap().len() == 2
    })
    .await;
    assert_eq!(h.fixture.text_watches.lock().unwrap()[1], Watch::Off);
    session
        .text_input
        .note(Some(text_field(InputType::Text)), Instant::now());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(text_notices(&client).len(), 2);
    h.end(&session, client).await?;
    h.finish().await
}

/// Over encrypted control, with `text_input_surrounding_text` on, the notice carries the text
/// around the cursor; never for a password field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_sends_surrounding_text_only_over_encrypted_control() -> Result<()> {
    use butterpollo_input_policy::text_input::{
        InputType, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    let h = Harness::configured(10_000, 1, "text_input_surrounding_text = enabled\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let request = TextInputRequest {
        notices: true,
        surrounding_text: true,
        element_ids: true,
        max_text: 64,
    };
    client
        .outbox
        .lock()
        .unwrap()
        .push(butterpollo_media_policy::packet::encrypted_control(
            &launch.key,
            1,
            TEXT_INPUT_MESSAGE_TYPE,
            &request.encode(),
            false,
        )?);
    until("the watcher starts", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *h.fixture.text_watches.lock().unwrap(),
        [Watch::On { max_text: 64 }]
    );
    session
        .text_input
        .note(Some(text_field(InputType::Text)), Instant::now());
    until("the raise arrives", WAIT, || {
        text_notices(&client).len() == 1
    })
    .await;
    let raise = &text_notices(&client)[0];
    assert_eq!(raise.text, "hello world");
    assert_eq!((raise.selection_start, raise.selection_end), (5, 5));
    assert_eq!(raise.element, 1, "the client asked for element ids");

    session
        .text_input
        .note(Some(text_field(InputType::Password)), Instant::now());
    until("the update arrives", WAIT, || {
        text_notices(&client).len() == 2
    })
    .await;
    let update = &text_notices(&client)[1];
    assert_eq!(update.input_type, InputType::Password);
    assert_eq!(update.text, "", "a password field's text left the host");
    h.end(&session, client).await?;
    h.finish().await
}

/// A device without the keyboard permission, or a host with `keyboard_follows_focus` off,
/// starts no watcher and hears nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_needs_the_keyboard_permission_and_the_setting() -> Result<()> {
    use butterpollo_input_policy::text_input::{
        InputType, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    for (config, perm) in [
        ("", !butterpollo_input_policy::input::KEYBOARD_PERMISSION),
        ("keyboard_follows_focus = disabled\n", u32::MAX),
    ] {
        let h = Harness::configured(10_000, 1, config).await?;
        h.h.paired.write().unwrap().clients[0].perm = perm;
        let launch = h.launch(0, false).await?;
        let (session, client) = h.play(&launch).await?;
        let request = TextInputRequest {
            notices: true,
            surrounding_text: false,
            element_ids: false,
            max_text: 0,
        };
        client.send_control(TEXT_INPUT_MESSAGE_TYPE, &request.encode());
        until("the request is handled", WAIT, || {
            !h.fixture.text_watches.lock().unwrap().is_empty()
        })
        .await;
        assert_eq!(
            *h.fixture.text_watches.lock().unwrap(),
            [Watch::Off],
            "{config}"
        );
        session
            .text_input
            .note(Some(text_field(InputType::Text)), Instant::now());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(text_notices(&client).is_empty(), "{config}");
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

/// The device's keyboard layout report (0x553E) is kept, the newest winning, while the host
/// leaves the layout to the device; with `keyboard_layout = us` it is not taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_layout_reports_keep_the_newest() -> Result<()> {
    use butterpollo_input_policy::keyboard_layout::{
        KEYBOARD_LAYOUT_MESSAGE_TYPE, KeyboardLayout, LayoutSource,
    };
    let report = |klid: u32, tag: &str| KeyboardLayout {
        source: LayoutSource::Klid,
        klid,
        tag: tag.into(),
    };
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    for layout in [report(0x0407, "de-DE"), report(0x040C, "fr-FR")] {
        client.send_control(KEYBOARD_LAYOUT_MESSAGE_TYPE, &layout.encode().unwrap());
    }
    // Kept as the layout id it resolved to on arrival.
    until("the newest report is kept", WAIT, || {
        *session.keyboard_layout.lock().unwrap() == Some(0x040C)
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await?;

    let h = Harness::configured(10_000, 1, "keyboard_layout = us\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    client.send_control(
        KEYBOARD_LAYOUT_MESSAGE_TYPE,
        &report(0x0407, "de-DE").encode().unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*session.keyboard_layout.lock().unwrap(), None);
    h.end(&session, client).await?;
    h.finish().await
}

/// A device without the keyboard permission does not pick the layout keys go with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_layout_reports_need_the_keyboard_permission() -> Result<()> {
    use butterpollo_input_policy::keyboard_layout::{
        KEYBOARD_LAYOUT_MESSAGE_TYPE, KeyboardLayout, LayoutSource,
    };
    use butterpollo_input_policy::text_input::{TEXT_INPUT_MESSAGE_TYPE, TextInputRequest};
    let h = Harness::new(10_000, 1).await?;
    h.h.paired.write().unwrap().clients[0].perm =
        !butterpollo_input_policy::input::KEYBOARD_PERMISSION;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let german = KeyboardLayout {
        source: LayoutSource::Klid,
        klid: 0x0407,
        tag: "de-DE".into(),
    };
    client.send_control(KEYBOARD_LAYOUT_MESSAGE_TYPE, &german.encode().unwrap());
    // Control messages are handled in order: once the request after it is, so is the report.
    let request = TextInputRequest {
        notices: true,
        surrounding_text: false,
        element_ids: false,
        max_text: 0,
    };
    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &request.encode());
    until("the request after the report is handled", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(*session.keyboard_layout.lock().unwrap(), None);
    h.end(&session, client).await?;
    h.finish().await
}

/// Reliable control messages the host sent during a 10 s outage reach the client right after
/// it: ENet resends them once the client is heard again, not when the timeouts that doubled
/// in the outage run out (client/protocol/vendor/rusty_enet).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reliable_messages_sent_in_an_outage_arrive_when_it_ends() -> Result<()> {
    use butterpollo_media_policy::link_notice::{
        LINK_NOTICE_MESSAGE_TYPE, LinkNotice, LinkNoticeRequest, LinkReason, LinkState,
    };
    let limited = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 100_000,
        current_kbps,
    };
    let arrival = |client: &Client, notice: LinkNotice| -> Option<Instant> {
        client
            .notices
            .lock()
            .unwrap()
            .iter()
            .find(|(_, payload)| LinkNotice::decode(payload).expect("a valid notice") == notice)
            .map(|(at, _)| *at)
    };
    let h = Harness::new(15_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    session.link_notices.note(limited(75_000));
    client.send_control(
        LINK_NOTICE_MESSAGE_TYPE,
        &LinkNoticeRequest { enabled: true }.encode(),
    );
    until("the standing limit arrives", WAIT, || {
        arrival(&client, limited(75_000)).is_some()
    })
    .await;

    let lost_at = Instant::now();
    client.network(true);
    // Over a second apart, so the host sends both in the outage.
    for (at_ms, kbps) in [(1_200, 60_000), (2_400, 50_000)] {
        tokio::time::sleep(Duration::from_millis(at_ms).saturating_sub(lost_at.elapsed())).await;
        session.link_notices.note(limited(kbps));
    }
    tokio::time::sleep(Duration::from_secs(10).saturating_sub(lost_at.elapsed())).await;
    assert!(
        arrival(&client, limited(60_000)).is_none(),
        "a notice crossed the outage"
    );
    let restored_at = Instant::now();
    client.network(false);
    until(
        "the notices sent in the outage arrive",
        Duration::from_secs(20),
        || arrival(&client, limited(50_000)).is_some(),
    )
    .await;
    let delay = arrival(&client, limited(50_000))
        .unwrap()
        .duration_since(restored_at);
    eprintln!("notice delay after the outage: {delay:?}");
    assert!(
        delay <= Duration::from_millis(500),
        "{delay:?} after the outage ended"
    );
    assert!(!session.stopping());
    h.end(&session, client).await?;
    h.finish().await
}

/// A device's `background_stream` launch of an app runs in Nucleus Co-op, a
/// private session of one player: the host's own app is neither replaced,
/// started nor kept, the stream is a Nucleus player's, its owner returns to
/// it and another device is refused, by the app and by its co-op entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_launches_run_in_nucleus_and_only_their_owner_returns() -> Result<()> {
    let (h, pc, entry) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, portal).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    assert_eq!(seat.slot, 0);
    assert_eq!((launch.background, launch.app_id), (Some(true), portal));
    let listed = h.h.nucleus.sessions();
    assert_eq!(
        listed[0].session.kind,
        butterpollo_session::nucleus::coop::Kind::Background
    );
    assert_eq!(
        (listed[0].session.slots.len(), listed[0].app_id),
        (1, portal)
    );
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    let config = crate::stream::effective_config(&h.h, &launch)?;
    assert!(crate::nucleus::player(&config));
    assert!(!butterpollo_session::background::enabled(&config));
    let refused = launch_app(&h, 1, portal).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains("Portal 2 can&apos;t stream in the background")
            || refused.contains("Portal 2 can't stream in the background"),
        "{refused}"
    );
    assert!(refused.contains("for a background stream"), "{refused}");
    assert!(pending_of(&h, 1).is_none());
    let by_entry = launch_app(&h, 1, entry).await?;
    assert!(by_entry.contains("status_code=\"409\""), "{by_entry}");
    // The owner's next launch returns to its own slot of the same session.
    let again = launch_app(&h, 0, portal).await?;
    assert!(again.contains("status_code=\"200\""), "{again}");
    let relaunch = pending_of(&h, 0).unwrap();
    assert_eq!(relaunch.nucleus, Some(seat));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    drop(launch);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&relaunch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &relaunch.id);
    drop(relaunch);
    // The harness's last launch is a normal one.
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
    }
    h.finish().await
}

/// A stream's launch for client `index` of the harness, under `uuid`.
fn parked_launch(h: &Harness, index: usize, uuid: &str) -> Launch {
    let mut client = h.h.paired.read().unwrap().clients[index].clone();
    client.uuid = uuid.into();
    crate::state::test_support::Fixture::new().launch(client, Role::Stream)
}

/// A stream's controllers carry whether its app starts through Steam, which Automatic needs
/// before it gives Nightjar's on-screen Steam Controller controls a Steam Controller. Every
/// path that gives a stream or a launch its controllers (the control loop and its retry, a
/// WebRTC stream and its retry, the launch's preparation) takes them through
/// `controller_sessions::acquire_for` with the stream's own launch and settings, as here: its
/// app with a Steam game's mark says yes, the Desktop no. The harness's control loop takes
/// none itself: it makes no injector before input comes (`ControlPeer::injector_tried`), and
/// input would reach this PC's desktop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streams_controllers_know_whether_its_app_starts_through_steam() -> Result<()> {
    use crate::controller_sessions::{acquire_for, key};
    let h = Harness::new(10_000, 1).await?;
    let pool = &h.h.controller_sessions;
    // The harness's app, already running, with or without a Steam game's mark: its launch
    // neither switches apps nor starts anything.
    let mark = |marked: bool| {
        let mut apps = h.h.apps.write().unwrap();
        if marked {
            apps[0].extra.insert("steam-id".into(), "570".into());
        } else {
            apps[0].extra.remove("steam-id");
        }
    };
    let steam_app = |session: &Session| -> Result<Option<bool>> {
        let config = effective_config(&h.h, &session.launch)?;
        drop(acquire_for(&h.h, &session.launch, &config)?);
        Ok(pool
            .policy(&key(&session.launch))
            .map(|policy| policy.steam_app))
    };
    for marked in [true, false] {
        mark(marked);
        let launch = h.launch(0, false).await?;
        let (session, client) = h.play(&launch).await?;
        assert_eq!(steam_app(&session)?, Some(marked), "marked: {marked}");
        h.end(&session, client).await?;
    }
    pool.release_parked(WAIT, &Default::default())?;
    h.finish().await
}

/// #56: a desktop stream that ended parks its controller, which keeps the
/// first XInput slot, and the background game started next read that pad
/// instead of the player's. A new background session releases parked
/// controllers before it plugs its player's pad, but not a pending launch's,
/// which Nightjar prepares before its stream takes them, nor a running
/// stream's, which its control stream takes at its first input. A launch
/// the registry refuses, and the owner returning to its session, plug
/// nothing and leave them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_background_stream_releases_parked_controllers_first() -> Result<()> {
    use crate::controller_sessions::key;
    use std::collections::BTreeSet;
    let (h, _pc, _) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    let none = BTreeSet::new();
    let config = butterpollo_config::config::Config::default();
    let pool = &h.h.controller_sessions;
    // Client 1 streams the desktop; its controllers wait for its first
    // input.
    let streaming = h.launch(1, false).await?;
    let (session, client) = h.play(&streaming).await?;
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    // A desktop stream that ended, waiting for its reconnect, and a launch
    // whose controllers Nightjar prepared.
    let (gap, prepared) = (
        parked_launch(&h, 1, "gap"),
        parked_launch(&h, 1, "nightjar"),
    );
    // Parked together: a client's acquire evicts the others' parked ones.
    let held: Vec<_> = [&streaming, &gap, &prepared]
        .into_iter()
        .map(|launch| pool.acquire(launch, &config, false))
        .collect::<Result<_>>()?;
    drop(held);
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .insert(prepared.id.clone(), prepared.clone());
    let kept = BTreeSet::from([key(&prepared), key(&streaming)]);
    assert!(pool.parked(&kept), "the gap's controller is parked");
    let first = launch_app(&h, 0, portal).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    assert!(!pool.parked(&kept), "the gap's controller is gone");
    assert!(
        pool.parked(&BTreeSet::from([key(&prepared)])),
        "the running stream's stays"
    );
    assert!(
        pool.parked(&BTreeSet::from([key(&streaming)])),
        "the prepared one stays"
    );
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&prepared.id);
    drop((canceled, streaming));
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    // Client 1 is refused: the session is client 0's. Its launch ends its
    // desktop stream first.
    let refused = launch_app(&h, 1, portal).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(pool.parked(&none));
    let again = launch_app(&h, 0, portal).await?;
    assert!(again.contains("status_code=\"200\""), "{again}");
    assert!(pool.parked(&none));
    let relaunch = pending_of(&h, 0).unwrap();
    drop(launch);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&relaunch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &relaunch.id);
    drop((relaunch, session, client));
    pool.release_parked(Duration::from_secs(5), &none)?;
    // The harness's last launch is a normal one.
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
    }
    h.finish().await
}

/// #56: a new co-op session plugs its players' pads too, so it releases the
/// controllers that streams which ended still park first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_co_op_session_releases_parked_controllers_first() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    let none = std::collections::BTreeSet::new();
    let gap = parked_launch(&h, 0, "gap");
    drop(h.h.controller_sessions.acquire(
        &gap,
        &butterpollo_config::config::Config::default(),
        false,
    )?);
    assert!(h.h.controller_sessions.parked(&none));
    let started = launch_app(&h, 1, id).await?;
    assert!(started.contains("status_code=\"200\""), "{started}");
    assert!(!h.h.controller_sessions.parked(&none));
    let launch = pending_of(&h, 1).unwrap();
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop((canceled, launch));
    h.finish().await
}

/// Moonlight and Nightjar end the stream first and then quit: a quit from
/// the device whose background stream already ended still ends that
/// session, by Moonlight's /cancel or Nightjar's Close game, instead of
/// leaving Nucleus Co-op running it for the two minutes its slot is kept
/// (#56). The host's own app stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quit_after_the_stream_ended_ends_the_background_stream() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
        client.extra.insert("host_control".into(), true.into());
    }
    let generation = || {
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .map(|app| app.generation.clone())
    };
    let before = generation();
    // The stream ends: its launch is gone, its slot kept for the device.
    let vacate = |h: &Harness| {
        let launch = pending_of(h, 0).unwrap();
        let seat = launch.nucleus.unwrap();
        let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop(canceled);
        h.h.nucleus.leave(seat, &launch.id);
        seat
    };
    let ended = |seat: butterpollo_session::session::Seat| {
        h.h.nucleus
            .sessions()
            .iter()
            .find(|listed| listed.session.id == seat.session)
            .is_none_or(|listed| listed.session.ended())
    };
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    assert!(!ended(seat));
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[0].stopped());
    assert_eq!(generation(), before);

    // Nightjar quits with Close game where the device has host control.
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        crate::native_api::session_action(&host, &client, crate::native_api::Action::CloseGame)
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[1].stopped());
    assert_eq!(generation(), before);

    // Native End too. A device that owns the host's app quits that app, as
    // Moonlight shows it running, and its background stream stays.
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    let owner = |who: &str| {
        let mut app = h.h.current_app.lock().unwrap();
        std::mem::replace(&mut app.as_mut().unwrap().owner, who.into())
    };
    let previous = owner("0");
    let host = h.h.clone();
    let quit =
        tokio::task::spawn_blocking(move || crate::nvhttp::quit_nucleus_stream(&host, "0", false))
            .await?
            .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(!quit);
    assert!(!ended(seat));
    owner(&previous);
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        crate::native_api::session_action(&host, &client, crate::native_api::Action::End)
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[2].stopped());
    assert_eq!(generation(), before);
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
        client.extra.remove("host_control");
    }
    h.finish().await
}

/// A one-player game without a hub handler streams in the background with
/// Rubylight's own handler, written once at the launch: the stream is a
/// Nucleus player's like any other, with nothing added on its way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_without_a_hub_handler_runs_with_ours() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    let mut ice = h.h.apps.read().unwrap().last().unwrap().clone();
    ice.name = "Ice".into();
    ice.cmd = r"C:\NoSuchGames\Ice\Ice.exe".into();
    ice.extra.insert("background-stream".into(), true.into());
    let id = ice.id();
    h.h.apps.write().unwrap().push(ice);
    let started = launch_app(&h, 0, id).await?;
    assert!(started.contains("status_code=\"200\""), "{started}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].session.game, own[0].guid());
    assert_eq!((listed[0].session.slots.len(), listed[0].app_id), (1, id));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    let config = crate::stream::effective_config(&h.h, &launch)?;
    assert!(crate::nucleus::player(&config));
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &launch.id);
    drop(launch);
    h.finish().await
}

/// A background launch Nucleus Co-op can't run is refused with the reason
/// and streams nothing: no display, no app, nothing pending. An app's own
/// `background-stream` asks for one as the device's setting does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_nucleus_cannot_run_streams_nothing() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    // A launcher's link names no game for Nucleus Co-op to start.
    let mut store = h.h.apps.read().unwrap().last().unwrap().clone();
    store.name = "Store Game".into();
    store.cmd = "com.epicgames.launcher://apps/x?action=launch".into();
    store.extra.insert("background-stream".into(), true.into());
    let id = store.id();
    h.h.apps.write().unwrap().push(store);
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(
        missing.contains("Store Game does not name one"),
        "{missing}"
    );
    assert!(missing.contains("stream in the background"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    // Where Nucleus Co-op can't run at all, 503 and why.
    h.h.nucleus
        .use_pc(Arc::new(crate::nucleus::fake::Pc::unavailable()));
    let unavailable = launch_app(&h, 0, id).await?;
    assert!(unavailable.contains("status_code=\"503\""), "{unavailable}");
    assert!(unavailable.contains("not on this test PC"), "{unavailable}");
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert!(pc.started.lock().unwrap().is_empty());
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_race_to_resume_the_same_app() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let (a, b) = tokio::join!(h.launch(0, true), h.launch(1, true));
    let (a, b) = (a?, b?);
    assert_ne!(a.id, b.id);
    h.counts(0, 2).await?;
    let (a, b) = tokio::join!(h.play(&a), h.play(&b));
    let ((a, ac), (b, bc)) = (a?, b?);
    h.counts(2, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    assert_eq!(h.fixture.inputs.load(Ordering::Acquire), 2);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.rtsp(&a.launch, 3, "TEARDOWN", "").await?;
    until("first client's resources released", WAIT, || {
        h.fixture.displays.load(Ordering::Acquire) == 1
            && h.fixture.inputs.load(Ordering::Acquire) == 1
    })
    .await;
    assert_eq!(a.termination_reason(), CLOSED);
    assert!(!b.stopping());
    let before = bc.frames.load(Ordering::Acquire);
    until(
        "second client's video survives first client's teardown",
        WAIT,
        || bc.frames.load(Ordering::Acquire) > before,
    )
    .await;
    h.counts(1, 0).await?;
    drop(ac);
    h.end(&b, bc).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_waits_for_previous_teardown() -> Result<()> {
    for (client, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        reconnect_waits_for_previous_teardown(client, resume).await?;
    }
    Ok(())
}

async fn reconnect_waits_for_previous_teardown(client_index: usize, resume: bool) -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    h.fixture.hold_teardown.store(true, Ordering::Release);
    h.rtsp(&launch, 3, "TEARDOWN", "").await?;
    until("previous session enters resource teardown", WAIT, || {
        h.fixture.in_teardown.load(Ordering::Acquire)
    })
    .await;
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 0).await?;
    assert!(
        h.h.sessions
            .lock()
            .unwrap()
            .teardown
            .contains_key(&launch.id)
    );
    let (session, resumed_client, overlapped) = {
        let resume = h.launch(client_index, resume);
        tokio::pin!(resume);
        let launch = match tokio::time::timeout(Duration::from_millis(500), &mut resume).await {
            Ok(result) => result?,
            Err(_) => {
                h.counts(0, 0).await?;
                assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
                h.fixture.hold_teardown.store(false, Ordering::Release);
                resume.await?
            }
        };
        let (session, resumed_client) = h.play(&launch).await?;
        let overlapped = h.fixture.displays.load(Ordering::Acquire) > 1;
        h.fixture.hold_teardown.store(false, Ordering::Release);
        (session, resumed_client, overlapped)
    };
    drop(client);
    h.end(&session, resumed_client).await?;
    h.finish().await?;
    assert!(
        !overlapped,
        "RTSP PLAY started a replacement while the previous session still owned display resources in teardown"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_reports_busy_when_previous_teardown_times_out() -> Result<()> {
    for (client_index, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        let h = Harness::new(10_000, 2).await?;
        let launch = h.launch(0, false).await?;
        let (old, client) = h.play(&launch).await?;
        h.fixture.hold_teardown.store(true, Ordering::Release);
        h.rtsp(&launch, 3, "TEARDOWN", "").await?;
        until("previous session enters resource teardown", WAIT, || {
            h.fixture.in_teardown.load(Ordering::Acquire)
        })
        .await;

        let started = Instant::now();
        let response = h.launch_response(client_index, resume).await?;
        assert!(started.elapsed() >= Duration::from_secs(5));
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(
            response.contains(&format!(
                "status_message=\"{}\"",
                crate::nvhttp::STILL_CLOSING
            )),
            "{response}"
        );
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"), "{response}");
        h.counts(0, 0).await?;
        assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
        assert!(
            h.h.sessions
                .lock()
                .unwrap()
                .teardown
                .contains_key(&launch.id)
        );
        assert_eq!(old.termination_reason(), CLOSED);

        h.fixture.hold_teardown.store(false, Ordering::Release);
        drop(client);
        h.empty().await?;
        let launch = h.launch(client_index, resume).await?;
        let (session, client) = h.play(&launch).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

/// A control worker that runs late finds ENet's own timeout for a lost client before its ping
/// check. The stream ended as closed by the host (0x80030023); it was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_control_stream_found_late_by_enet_still_fails() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let lost = Instant::now();
    // From before the ping check is due, once ENet has resent its unanswered ping often enough,
    // to after ENet's own timeout.
    *h.fixture.control_held.lock().unwrap() = Some((
        lost + Duration::from_millis(1800),
        lost + Duration::from_millis(4000),
    ));
    client.control_down.store(true, Ordering::Release);
    until("the lost client's stream ends", WAIT, || session.stopping()).await;
    h.empty().await?;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.finish().await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_lost_while_video_continues_times_out() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let before = client.frames.load(Ordering::Acquire);
    let released = h.fixture.input_releases.load(Ordering::Acquire);
    client.control_down.store(true, Ordering::Release);
    assert!(!session.stopping());
    until(
        "video traffic cannot keep a lost control stream alive",
        WAIT,
        || session.stopping(),
    )
    .await;
    h.empty().await?;
    let released_after = h.fixture.input_releases.load(Ordering::Acquire);
    let video_after_release = h.fixture.video_after_input_release.load(Ordering::Acquire);
    let frames_after = client.frames.load(Ordering::Acquire);
    assert!(
        released_after > released,
        "control loss must release input: {released} -> {released_after}"
    );
    assert!(
        video_after_release > released,
        "video must continue after control loss and before timeout: input {released} -> {released_after}, video checkpoint {video_after_release}, frames {before} -> {frames_after}"
    );
    assert!(
        frames_after > before,
        "client must still receive video while control is blocked: {before} -> {frames_after}"
    );
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_enet_peer_reset_mid_stream_times_out_then_resumes() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    *h.fixture.reset.lock().unwrap() = Some(session.launch.id.clone());
    until("host ENet peer was reset", WAIT, || {
        h.fixture.reset_done.load(Ordering::Acquire)
    })
    .await;
    until("reset peer's session times out", WAIT, || {
        session.stopping()
    })
    .await;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.empty().await?;
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A microphone client as Apollo's: offered in DESCRIBE, set up and played
/// as `streamid=mic`, its encrypted packets taken for its own session only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn microphone_packets_reach_only_the_session_that_set_them_up() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(
        describe.contains(&format!(
            "m=audio {} RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\n",
            h.ports.mic
        )),
        "{describe}"
    );
    // Loopback is a LAN client: encryption optional, the microphone requested.
    assert!(
        describe.contains("encryptionSupported:13\r\n"),
        "{describe}"
    );
    assert!(describe.contains("encryptionRequested:9\r\n"), "{describe}");
    let setup = h
        .rtsp_at(&launch, 11, "SETUP", "streamid=mic/0/0", "")
        .await?;
    assert!(
        setup.contains(&format!("Transport: server_port={}\r\n", h.ports.mic)),
        "{setup}"
    );
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(session.config.mic);
    // A client that plays each stream on its own plays the microphone last.
    h.rtsp_at(&launch, 12, "PLAY", "streamid=mic", "").await?;
    let counters = h
        .media
        .as_ref()
        .unwrap()
        .mic
        .clone()
        .expect("microphone port bound");
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let send = |datagram: Vec<u8>| {
        socket
            .send_to(&datagram, address(h.ports.mic))
            .expect("microphone packet sent");
    };
    // A loaded PC loses the odd loopback datagram: what has not arrived by the next
    // attempt is sent again, under a new sequence number.
    let attempt = Duration::from_millis(200);
    let mut sequence = 0;
    let mut sent = Instant::now() - attempt;
    until("microphone packets accepted", WAIT, || {
        let accepted = counters.accepted.load(Ordering::Acquire);
        if accepted < 5 && sent.elapsed() >= attempt {
            for _ in accepted..5 {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    sequence,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sequence += 1;
            }
            sent = Instant::now();
        }
        accepted >= 5
    })
    .await;
    assert_eq!(counters.refused.load(Ordering::Acquire), 0);
    let mut damaged = mic::seal(&launch.key, launch.key_id, sequence, 0, &[0xf8, 0xff, 0xfe]);
    // CBC has no authentication tag: a wrong key can randomly produce valid
    // padding. Keep the microphone header but truncate the ciphertext so this
    // rejection is deterministic for every randomly generated launch key.
    damaged.pop();
    let mut sent = Instant::now() - attempt;
    until("a damaged microphone packet refused", WAIT, || {
        if sent.elapsed() >= attempt {
            send(damaged.clone());
            sent = Instant::now();
        }
        counters.refused.load(Ordering::Acquire) >= 1
    })
    .await;
    h.end(&session, client).await?;
    // No session takes microphone packets from here on.
    let accepted = counters.accepted.load(Ordering::Acquire);
    // Without a microphone SETUP, the next session's packets are refused.
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(!session.config.mic);
    let refused = counters.refused.load(Ordering::Acquire);
    let mut sent = Instant::now() - attempt;
    until(
        "microphone packet of a session without one refused",
        WAIT,
        || {
            if sent.elapsed() >= attempt {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    0,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sent = Instant::now();
            }
            counters.refused.load(Ordering::Acquire) > refused
        },
    )
    .await;
    assert_eq!(counters.accepted.load(Ordering::Acquire), accepted);
    h.end(&session, client).await?;
    // A client that would send it in the clear is not taken.
    let launch = h.launch(0, false).await?;
    h.rtsp_at(&launch, 11, "SETUP", "streamid=mic", "").await?;
    let (session, client) = h.play_encrypted(&launch, 0).await?;
    assert!(!session.config.mic);
    assert!(
        session
            .launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == "microphone"),
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Nonary's calibration and Nightjar read the host's UDP port from the
/// headers before the probe's result, so the host must send them at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_client_runs_the_pyrowave_udp_probe_with_a_handshake() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let token = "0123456789abcdef0123456789abcdef";
    let client = std::net::UdpSocket::bind("127.0.0.1:0")?;
    // Buffer the complete two-second probe if parallel tests deschedule this receiver.
    socket2::SockRef::from(&client).set_recv_buffer_size(8 * 1024 * 1024)?;
    let named = client.local_addr()?.port();
    let url = |handshake: u8| {
        format!(
            "https://127.0.0.1:{}/pyrowave-udp-probe?kbps=20000&port={named}&packetsize=1024&token={token}&handshake={handshake}",
            h.ports.https
        )
    };
    // Not over plain HTTP, where no client is identified.
    let plain = format!("http://127.0.0.1:{}/pyrowave-udp-probe", h.ports.http);
    assert_eq!(
        h.http.get(plain).send().await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let response = h.clients[0].get(url(1)).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let port: u16 = response.headers()["x-pyrowave-udp-port"]
        .to_str()?
        .parse()?;
    let counted = tokio::task::spawn_blocking(move || -> Result<usize> {
        client.send_to(token.as_bytes(), ("127.0.0.1", port))?;
        client.set_read_timeout(Some(Duration::from_millis(400)))?;
        let mut seen = std::collections::HashSet::new();
        let mut buffer = [0u8; 2048];
        while let Ok((bytes, _)) = client.recv_from(&mut buffer) {
            if bytes == 1024 + 48 && &buffer[..32] == token.as_bytes() {
                seen.insert(u32::from_be_bytes(buffer[32..36].try_into()?));
            }
        }
        Ok(seen.len())
    });
    let body = response.text().await?;
    let counted = counted.await??;
    let field = |name: &str| -> u32 {
        body.split(&format!("<{name}>"))
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    assert!(body.contains("status_code=\"200\""), "{body}");
    assert_eq!(
        field("expected"),
        butterpollo_media_policy::pyrowave_probe::expected(20_000, 1024)
    );
    // A busy machine (a CI runner) can fall behind the schedule and stop at
    // the overload limit (2128 of 4317 on a windows-2022 runner); what the
    // host reports sent must still arrive.
    assert!(field("sent") > field("expected") / 4, "{body}");
    assert!(field("sent") <= field("expected"), "{body}");
    assert_eq!(counted as u32, field("sent"));
    // A bad query is refused before anything is sent.
    let bad = h.clients[0].get(url(2)).send().await?;
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_state_follows_a_launch_a_resume_and_the_end_of_a_stream() -> Result<()> {
    use rubylight_native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h.directory.join(crate::session_control::JOURNAL);
    // A game runs from before and nobody streams.
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert!(!journal.exists());

    // /launch replies and waits for the client.
    let launch = h.launch(0, false).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Launching);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));
    h.h.session_flush();
    assert!(journal.exists());
    let (first, first_client) = h.play(&launch).await?;
    assert_eq!(h.h.session_snapshot().state, State::Streaming);

    // The same device's /resume supersedes its running stream.
    let resume = h.launch(0, true).await?;
    assert!(first.stopping());
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    drop(first_client);
    until("the superseded stream ended", WAIT, || {
        !h.h.sessions
            .lock()
            .unwrap()
            .active
            .contains_key(&first.launch.id)
    })
    .await;
    let (second, client) = h.play(&resume).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Streaming);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));

    // The client ends the stream: no display is kept, so the session is
    // idle and the game keeps running.
    h.end(&second, client).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_while_a_launch_waits_for_its_client_ends_the_session() -> Result<()> {
    use rubylight_native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h.directory.join(crate::session_control::JOURNAL);
    // The copy holds the launch's resources: keep only the id.
    let id = h.launch(0, false).await?.id;
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    h.h.session_flush();
    assert!(journal.exists());

    // Stock /cancel, from the launching device, stops the pending launch and
    // the game.
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(!h.h.sessions.lock().unwrap().pending.contains_key(&id));
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

/// A launch whose app fails to start after its display was prepared answers
/// with an error, so the client never connects. Its pending slot and display
/// lease go at once rather than at the 30 s expiry: /serverinfo must not keep
/// counting a client that is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_whose_app_fails_to_start_is_not_left_pending() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .fail_app_preparation
        .store(true, Ordering::Release);
    let response = h.launch_response(0, false).await?;
    assert!(!response.contains("status_code=\"200\""), "{response}");
    assert_eq!(xml(&response, "gamesession"), "0", "{response}");
    assert!(h.h.sessions.lock().unwrap().pending.is_empty());
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    h.fixture
        .fail_app_preparation
        .store(false, Ordering::Release);
    h.finish().await
}

/// A client that negotiates a ten-bit format (`dynamicRangeMode:1`) after a
/// launch or resume: it is ten-bit SDR only when the query said `hdrMode=0`,
/// and HDR otherwise (no `hdrMode`, or `hdrMode=1`). SDR keeps `hdr` off, the
/// part the prepared display compares, so the display prepared for the launch
/// is kept, and the client is told HDR is off. The three launches run first,
/// each ended by TEARDOWN, which leaves the app for the three resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ten_bit_format_streams_sdr_only_with_hdr_mode_zero() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let sdp = "a=x-nv-vqos[0].bitStreamFormat:1\r\na=x-nv-video[0].dynamicRangeMode:1\r\n";
    // (resume, hdrMode, streams ten-bit SDR rather than HDR)
    for (resume, hdr_mode, sdr) in [
        (false, None, false),
        (false, Some("0"), true),
        (false, Some("1"), false),
        (true, None, false),
        (true, Some("0"), true),
        (true, Some("1"), false),
    ] {
        let case = format!("resume={resume} hdrMode={hdr_mode:?}");
        let launch = h.launch_hdr(0, resume, hdr_mode).await?;
        let (session, client) = h.connect_announcing(&launch, 0, sdp).await?;
        let config = &session.config;
        assert!(config.ten_bit(), "{case}");
        assert_eq!((config.hdr, config.sdr_10bit), (!sdr, sdr), "{case}");
        // The message needs the control connection only.
        until("HDR mode message", WAIT, || {
            !client.hdr_modes.lock().unwrap().is_empty()
        })
        .await;
        assert_eq!(
            client.hdr_modes.lock().unwrap()[0].1[0],
            u8::from(!sdr),
            "{case}"
        );
        h.end(&session, client).await?;
    }
    h.finish().await
}

const VIEWPORT: &str =
    "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n";

/// Moves `launch`'s reply `age` into the past, as a client that took that long would see it.
fn age_launch(h: &Harness, launch: &Launch, age: Duration) {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .get_mut(&launch.id)
        .expect("launch pending")
        .created = Instant::now() - age;
}

/// A client may not use its launch before the host has prepared it and
/// replied: an RTSP request then gets no answer, and the launch, once
/// prepared, streams as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_still_being_prepared_takes_no_rtsp_request() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .hold_app_preparation
        .store(true, Ordering::Release);
    let early = async {
        until("launch preparing its app", WAIT, || {
            h.fixture.app_preparation_waiting.load(Ordering::Acquire)
        })
        .await;
        let early =
            h.h.sessions
                .lock()
                .unwrap()
                .pending
                .values()
                .next()
                .cloned();
        let early = early.expect("a launch is pending while it prepares");
        let options = h.rtsp(&early, 1, "OPTIONS", "").await;
        let announce = h.rtsp(&early, 2, "ANNOUNCE", VIEWPORT).await;
        h.fixture
            .resume_app_preparation
            .store(true, Ordering::Release);
        (early, options, announce)
    };
    let (launch, (early, options, announce)) = tokio::join!(h.launch(0, false), early);
    let launch = launch?;
    assert_eq!(launch.id, early.id);
    for refused in [options, announce] {
        let refused = refused.unwrap_err().to_string();
        assert!(refused.contains("closed without a response"), "{refused}");
    }
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A handshake under way when the launch's 30 s pass keeps it for its lease;
/// a launch nobody claimed in its 30 s is refused and pruned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claimed_launch_outlives_its_30_s_and_an_unclaimed_one_expires() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    age_launch(&h, &launch, Duration::from_secs(31));
    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "claimed launch did not stream"
    );
    h.end(&session, client).await?;

    let stale = h.launch(0, false).await?;
    age_launch(&h, &stale, Duration::from_secs(31));
    let refused = h
        .rtsp(&stale, 1, "OPTIONS", "")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    h.counts(0, 0).await?;
    // The pruned launch holds its display until the last copy goes.
    drop(stale);
    h.finish().await
}

/// The device launches again while an RTSP connection still holds its older
/// launch: that connection's request is refused, and only the new launch streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_racing_a_new_launch_streams_only_the_new_one() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let first = h.launch(0, false).await?;
    h.rtsp(&first, 1, "OPTIONS", "").await?;
    // An RTSP connection authorized for the first launch, before it reads.
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let mut held = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until(
        "RTSP connection authorized for the first launch",
        WAIT,
        || h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire),
    )
    .await;
    let second = h.launch(0, false).await?;
    assert_ne!(first.id, second.id);
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    held.write_all(&sealed_rtsp(
        &first,
        2,
        "ANNOUNCE",
        "rtsp://127.0.0.1/stream",
        VIEWPORT,
    )?)
    .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, held.read_to_end(&mut response)).await??;
    let refused = opened_rtsp(&first, "ANNOUNCE", &response)
        .unwrap_err()
        .to_string();
    assert!(refused.starts_with("RTSP/1.0 400"), "{refused}");
    assert!(refused.contains("replaced"), "{refused}");
    let refused = h.rtsp(&first, 3, "PLAY", "").await.unwrap_err().to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    // The replaced launch holds its display until the last copy goes.
    drop(first);
    h.counts(0, 1).await?;
    let (session, client) = h.play(&second).await?;
    assert_eq!(
        h.h.sessions
            .lock()
            .unwrap()
            .active
            .keys()
            .collect::<Vec<_>>(),
        [&second.id]
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Two devices launch at once. Two PLAYs of one launch start one session,
/// and the other device's launch stays its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_launching_at_once_each_keep_their_launch_and_play_once() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let (other, launch) = tokio::join!(h.launch(0, false), h.launch(1, false));
    let (other, launch) = (other?, launch?);
    h.counts(0, 2).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    let (a, b) = tokio::join!(
        h.rtsp(&launch, 2, "PLAY", ""),
        // Not 3: that is the TEARDOWN's.
        h.rtsp(&launch, 4, "PLAY", "")
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "{a:?} {b:?}"
    );
    let session = {
        let sessions = h.h.sessions.lock().unwrap();
        assert_eq!(sessions.active.len(), 1);
        assert!(sessions.pending.contains_key(&other.id));
        sessions.active[&launch.id].clone()
    };
    h.counts(1, 1).await?;
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "the one session did not stream"
    );
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&other.id));
    drop((canceled, other));
    h.end(&session, client).await?;
    h.finish().await
}

/// A client that connects and stalls is cut off at the handshake deadline,
/// without costing its launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_rtsp_connection_is_closed_at_its_deadline() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture.rtsp_deadline_ms.store(300, Ordering::Release);
    let launch = h.launch(0, false).await?;
    let started = Instant::now();
    let mut stalled = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    // Half of an encrypted request's header, then nothing.
    stalled
        .write_all(&sealed_rtsp(&launch, 1, "OPTIONS", "rtsp://127.0.0.1/stream", "")?[..12])
        .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stalled.read_to_end(&mut response)).await??;
    assert!(response.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300));
    h.counts(0, 1).await?;
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A launch of app `id` from `client`, with the query a Moonlight launch sends.
async fn launch_app(h: &Harness, client: usize, id: u32) -> Result<String> {
    Ok(h.clients[client]
        .get(format!("https://127.0.0.1:{}/launch", h.ports.https))
        .query(&[
            ("appid", id.to_string()),
            ("rikey", hex::encode(crypto::random::<16>())),
            ("rikeyid", "1".into()),
            ("corever", "1".into()),
            ("mode", "1281x721x60".into()),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}
/// #56: the launch that reaches a background stream's session no longer asks
/// to play on the PC, whatever the device does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_never_plays_its_game_on_the_pc() -> Result<()> {
    let (h, _pc, _entry) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    let launch = || {
        h.clients[0]
            .get(format!("https://127.0.0.1:{}/launch", h.ports.https))
            .query(&[
                ("appid", portal.to_string()),
                ("rikey", hex::encode(crypto::random::<16>())),
                ("rikeyid", "1".into()),
                ("corever", "1".into()),
                ("mode", "1281x721x60".into()),
                ("localAudioPlayMode", "1".into()),
            ])
            .send()
    };
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("background_stream".into(), true.into());
    let reply = launch().await?.text().await?;
    assert!(reply.contains("status_code=\"200\""), "{reply}");
    let pending = pending_of(&h, 0).unwrap();
    assert_eq!(pending.background, Some(true));
    assert!(!pending.host_audio);
    Ok(())
}
/// A harness whose host has Nucleus Co-op (the fake) and Portal 2, which it
/// has a handler for; client 1 may join co-op and client 0 has it turned off.
/// Returns the co-op entry's id.
async fn coop_harness() -> Result<(Harness, Arc<crate::nucleus::fake::Pc>, u32)> {
    let h = Harness::new(10_000, 2).await?;
    let pc = Arc::new(crate::nucleus::fake::Pc::installed());
    h.h.nucleus.use_pc(pc.clone());
    h.h.apps.write().unwrap().push(App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", crate::nucleus::fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    });
    h.h.paired.write().unwrap().clients[1]
        .extra
        .insert("coop_join".into(), true.into());
    // Never set, a device that launches apps may join: client 0 has it off.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), false.into());
    let apps = h.h.apps.read().unwrap().clone();
    let entries = h.h.nucleus.entries(&apps, "");
    assert_eq!(entries.len(), 1);
    let id = entries[0].id();
    Ok((h, pc, id))
}
/// A copy of the launch `client` has pending; it holds the launch's display lease.
fn pending_of(h: &Harness, client: usize) -> Option<Launch> {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .values()
        .find(|l| l.client.uuid == client.to_string())
        .cloned()
}

/// Two devices launching the co-op entry share one Nucleus session, each in
/// its own slot. Neither replaces, starts or keeps the host's app: Nucleus
/// started the game. The pending launches carry the seats their streams use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_launches_share_one_session_beside_the_running_app() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, id).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    assert_eq!(xml(&first, "gamesession"), "1");
    let second = launch_app(&h, 1, id).await?;
    assert!(second.contains("status_code=\"200\""), "{second}");
    h.counts(0, 2).await?;
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    assert_eq!(
        a.nucleus,
        Some(butterpollo_session::session::Seat { session, slot: 0 })
    );
    assert_eq!(
        b.nucleus,
        Some(butterpollo_session::session::Seat { session, slot: 1 })
    );
    assert_eq!((a.app_id, b.app_id), (id, id));
    assert_eq!(a.background, Some(false));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // The first window was parked at the stream's even size.
    assert_eq!(pc.last().sizes.lock().unwrap().first(), Some(&(1280, 720)));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, true]);
    assert_eq!(listed[0].app_id, id);
    // The launches' last copies give their slots back.
    for launch in [a, b] {
        let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop(canceled);
    }
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    h.finish().await
}

/// A co-op launch Nucleus refuses answers with the refusal's status and text
/// and leaves nothing pending: a device without Join co-op (409), a start
/// that fails (503) and a game whose handler went away (404).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_co_op_launches_answer_their_status_and_leave_nothing_pending() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op did not start".into());
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(failed.contains("Nucleus Co-op did not start"), "{failed}");
    assert_eq!(xml(&failed, "gamesession"), "0");
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seated = pending_of(&h, 1).unwrap();
    // Device 0 may not join the running session.
    let refused = launch_app(&h, 0, id).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(refused.contains("Can join co-op games"), "{refused}");
    assert!(pending_of(&h, 0).is_none());
    // The handler is gone by the time the next device launches.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    pc.catalog.lock().unwrap().handlers.clear();
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(missing.contains("portal2.exe"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&seated.id);
    drop(canceled);
    // The copy holds the launch's display lease and its slot.
    drop(seated);
    h.finish().await
}

/// A co-op launch that fails after Nucleus seated it gives the slot back: the
/// next launch of that device is not held off by a slot nobody streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_launch_that_fails_later_gives_its_slot_back() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    h.fixture
        .disconnect_during_preparation
        .store(true, Ordering::Release);
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(failed.contains("disconnected or changing"), "{failed}");
    assert!(pending_of(&h, 0).is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.empty().await?;
    h.fixture
        .disconnect_during_preparation
        .store(false, Ordering::Release);
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    // The device gets its own slot back in the same session.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let launch = pending_of(&h, 0).unwrap();
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_session::session::Seat {
            session: listed[0].session.id,
            slot: 0
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    h.finish().await
}

/// A WebRTC launch of the co-op entry takes a seat as a Moonlight launch
/// does. Nucleus's refusals become start-session errors with their text:
/// busy or refused is SessionActive, anything else a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webrtc_co_op_launches_take_seats_and_answer_refusals() -> Result<()> {
    use crate::nvhttp::{WebRtcLaunch, WebRtcLaunchError};
    let (h, pc, id) = coop_harness().await?;
    let webrtc = |client: usize| {
        let request = WebRtcLaunch {
            client: h.h.paired.read().unwrap().clients[client].clone(),
            peer: std::net::Ipv4Addr::LOCALHOST.into(),
            this_pc: false,
            app_id: id,
            width: 1280,
            height: 720,
            fps: 60,
            hdr: false,
        };
        let host = h.h.clone();
        tokio::task::spawn_blocking(move || crate::nvhttp::launch_webrtc(&host, request))
    };
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op did not start".into());
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::Failed(message)) => {
            assert!(message.contains("did not start"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seat = pending_of(&h, 1).unwrap().nucleus.unwrap();
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::SessionActive(message)) => {
            assert!(message.contains("Can join co-op games"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    let Ok(launch) = webrtc(0).await? else {
        panic!("the co-op WebRTC launch failed");
    };
    assert_eq!(
        launch.transport,
        butterpollo_session::session::Transport::WebRtc
    );
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_session::session::Seat {
            session: seat.session,
            slot: 1
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    // A co-op player's WebRTC launch leaves the other player's Moonlight launch alone.
    let other = pending_of(&h, 1).expect("the Moonlight launch stays").id;
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other);
    drop(canceled);
    h.finish().await
}

/// A co-op launch never takes the host's session from its owner, and a co-op
/// player's stock /cancel or native End session ends only its own launch: the
/// other player's launch and the host's app stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_player_neither_owns_the_session_nor_cancels_the_others() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    let before = h.h.session_snapshot();
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let after = h.h.session_snapshot();
    assert_eq!((after.state, after.owner), (before.state, before.owner));
    let response = h.clients[1]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(pending_of(&h, 1).is_none());
    assert!(pending_of(&h, 0).is_some());
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // Nightjar's End session from the player that is left ends its own launch
    // too, though it owns no host session; the host's app stays.
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        crate::native_api::session_action(&host, &client, crate::native_api::Action::End)
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(pending_of(&h, 0).is_none());
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.finish().await
}

/// A co-op launch replaced by the device's next launch gives its slot back:
/// the player shows as away and keeps the slot for `coop::GRACE`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, false]);
    assert!(!listed[0].session.players()[0].away);
    let other = h.launch(0, false).await?;
    assert!(other.nucleus.is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other.id);
    drop(canceled);
    drop(other);
    h.finish().await
}

/// Moonlight's multi-controller packet: controller `id` of the `active` ones.
fn pad_packet(id: u16, active: u16, buttons: u16) -> Vec<u8> {
    let mut b = vec![0u8; 34];
    b[..4].copy_from_slice(&30u32.to_be_bytes());
    b[4..8].copy_from_slice(&12u32.to_le_bytes());
    b[10..12].copy_from_slice(&id.to_le_bytes());
    b[12..14].copy_from_slice(&active.to_le_bytes());
    b[16..18].copy_from_slice(&buttons.to_le_bytes());
    b
}
/// A key press of A.
fn key_packet() -> Vec<u8> {
    let mut b = vec![0u8; 14];
    b[..4].copy_from_slice(&10u32.to_be_bytes());
    b[4..8].copy_from_slice(&3u32.to_le_bytes());
    b[9..11].copy_from_slice(&0x8041u16.to_le_bytes());
    b
}

/// Two players' streams drive their own slots' pads and never the desktop:
/// a keyboard is dropped and said once. When the game crashes both streams
/// end with the reason, the session is stopped and nothing stays active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_drive_their_slots_and_end_with_the_session() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    ca.send_encrypted(&a.key, 0, 0x0206, &pad_packet(0, 1, 0x1000));
    ca.send_encrypted(&a.key, 1, 0x0206, &key_packet());
    ca.send_encrypted(&a.key, 2, 0x0206, &key_packet());
    cb.send_encrypted(&b.key, 0, 0x0206, &pad_packet(0, 1, 0x2000));
    until("both players' pads moved", WAIT, || {
        pc.last().inputs.lock().unwrap().len() == 2
    })
    .await;
    let mut inputs = pc.last().inputs.lock().unwrap().clone();
    inputs.sort_by_key(|(slot, _)| *slot);
    assert!(matches!(
        inputs[..],
        [
            (
                0,
                butterpollo_input_policy::input::Input::Controller {
                    buttons: 0x1000,
                    ..
                }
            ),
            (
                1,
                butterpollo_input_policy::input::Input::Controller {
                    buttons: 0x2000,
                    ..
                }
            )
        ]
    ));
    let codes = |s: &Session| -> Vec<String> {
        s.launch
            .warnings
            .snapshot()
            .into_iter()
            .map(|w| w.code)
            .filter(|code| code.starts_with("coop_"))
            .collect()
    };
    until("the keyboard was said to be dropped", WAIT, || {
        codes(&sa) == ["coop_keyboard_mouse"]
    })
    .await;
    assert!(codes(&sb).is_empty());
    pc.last()
        .session
        .lock()
        .unwrap()
        .end(butterpollo_session::nucleus::coop::End::Crashed(
            "portal2.exe exited".into(),
        ));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    for s in [&sa, &sb] {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(
            ended.message.contains("portal2.exe exited"),
            "{}",
            ended.message
        );
    }
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The streams' last copies of their launches gave the slots back.
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// The console's End and every player gone (`End::Empty`, which Nucleus
/// Co-op's session reaches after `coop::GRACE`) close each player's stream
/// quietly with the reason, never as a failure, and stop Nucleus Co-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_close_quietly_when_the_session_is_ended_or_empty() -> Result<()> {
    use butterpollo_session::nucleus::coop::End;
    let (h, pc, id) = coop_harness().await?;
    let closed_with = |s: &Session, text: &str| {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(ended.message.contains(text), "{}", ended.message);
        assert_eq!(s.termination_reason(), CLOSED);
    };
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    assert!(h.h.nucleus.end(session));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    closed_with(&sa, "The Portal 2 co-op session was ended on the PC");
    closed_with(&sb, "The Portal 2 co-op session was ended on the PC");
    assert!(pc.last().stopped());
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The next launch starts a new session; everybody leaves it.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let a = pending_of(&h, 0).unwrap();
    assert_ne!(a.nucleus.unwrap().session, session);
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    pc.last().session.lock().unwrap().end(End::Empty);
    until("the stream ended", WAIT, || sa.stopping()).await;
    closed_with(&sa, "every player left");
    drop((ca, a, sa));
    h.empty().await?;
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// A co-op launch whose device never connects gives its slot back when it
/// expires, once the expired launch is dropped outside the sessions lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let expired = {
        let mut sessions = h.h.sessions.lock().unwrap();
        let long_ago = Instant::now()
            .checked_sub(butterpollo_session::session::LAUNCH_WAIT + Duration::from_secs(1))
            .unwrap();
        for launch in sessions.pending.values_mut() {
            launch.created = long_ago;
        }
        sessions.expire()
    };
    assert_eq!(expired.len(), 1);
    // The expired launch still holds the slot until it is dropped.
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    drop(expired);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.finish().await
}

/// "Stream here too" on a device that does not own the game: `/resume` joins it beside the owner,
/// and takes nothing from it. With no game there is nothing to join, and nothing starts one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_by_a_device_that_does_not_own_the_game_joins_and_never_starts_one() -> Result<()>
{
    let h = Harness::new(10_000, 2).await?;
    let owner = "0".to_owned();
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = owner.clone();
    let launch = h.launch(1, true).await?;
    assert_eq!(launch.client.uuid, "1");
    h.counts(0, 1).await?;
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    assert!(!h.h.owns_game("1"));
    let (session, client) = h.play(&launch).await?;
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    h.end(&session, client).await?;

    *h.h.current_app.lock().unwrap() = None;
    let refused = h.launch_response(1, true).await;
    assert!(
        refused
            .as_ref()
            .map_or(true, |reply| !reply.contains("status_code=\"200\"")),
        "{refused:?}"
    );
    assert!(h.h.current_app.lock().unwrap().is_none(), "nothing started");
    h.finish().await
}

/// The stream of `client`'s launch, running over `transport` as far as the registry can tell.
async fn running_over(h: &Harness, client: usize, transport: Transport) -> Result<Arc<Session>> {
    let mut launch = h.launch(client, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(queued);
    launch.transport = transport;
    let session = Session::new(launch, Default::default());
    h.h.sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    Ok(session)
}

/// Ends `session` a moment after it is told to stop, as its worker would: a launch waits for that.
fn ends_when_stopped(h: &Harness, session: &Arc<Session>) -> thread::JoinHandle<()> {
    let (host, session) = (h.h.clone(), session.clone());
    thread::spawn(move || {
        let deadline = Instant::now() + WAIT;
        while !session.stopping() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        if session.stopping() {
            thread::sleep(Duration::from_millis(100));
            let removed = host
                .sessions
                .lock()
                .unwrap()
                .active
                .remove(&session.launch.id);
            drop(removed);
        }
    })
}

fn forget_stream(h: &Harness, session: Arc<Session>) {
    let removed =
        h.h.sessions
            .lock()
            .unwrap()
            .active
            .remove(&session.launch.id);
    drop((removed, session));
}

/// "Stream here too" promises a stream beside the owner, and a Moonlight stream ends a WebRTC one:
/// so another device's WebRTC stream refuses the join and keeps running, while a Moonlight stream
/// has the join beside it. The owner's own resume moves its stream between transports as ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_join_never_ends_the_owners_webrtc_stream_and_runs_beside_its_moonlight_one() -> Result<()>
{
    let h = Harness::new(10_000, 2).await?;
    let owner = "0".to_owned();
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = owner.clone();

    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(crate::nvhttp::WEBRTC_HELD_BY_OTHER),
        "{refused}"
    );
    assert!(!webrtc.stopping(), "the owner's stream goes on");
    assert!(pending_of(&h, 1).is_none());
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );

    // The same join as stock Moonlight makes it: a `/launch` of the Running Game tile.
    let game = h.h.current_app.lock().unwrap().as_ref().unwrap().id;
    let tile = butterpollo_session::remote::running_game_id(game).to_string();
    let refused = h
        .launch_response_with(1, false, None, &[("appid", &tile)])
        .await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(crate::nvhttp::WEBRTC_HELD_BY_OTHER),
        "{refused}"
    );
    assert!(!webrtc.stopping(), "the owner's stream goes on");
    assert!(pending_of(&h, 1).is_none());

    // The same refusal for a launch of the owner's on its way.
    forget_stream(&h, webrtc);
    let mut launch = h.launch(0, true).await?;
    launch.transport = Transport::WebRtc;
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .insert(launch.id.clone(), launch.clone());
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(pending_of(&h, 0).is_some(), "the owner's launch stays");
    let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop((queued, launch));

    // A stream that is ending, as after a press, holds nothing.
    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    webrtc.stop();
    let ended = ends_when_stopped(&h, &webrtc);
    let joined = h.launch(1, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&joined.id);
    drop((queued, joined));
    ended.join().unwrap();
    forget_stream(&h, webrtc);

    // A background stream is no plain one: it has a Nucleus seat, and a Moonlight stream never ends it.
    let mut background = h.launch(0, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&background.id);
    drop(queued);
    background.transport = Transport::WebRtc;
    background.background = Some(true);
    background.nucleus = Some(butterpollo_session::session::Seat {
        session: 1,
        slot: 0,
    });
    let session = Session::new(background, Default::default());
    h.h.sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    let joined = h.launch(1, true).await?;
    assert!(!session.stopping());
    let queued = h.h.sessions.lock().unwrap().pending.remove(&joined.id);
    drop((queued, joined));
    forget_stream(&h, session);

    // The owner moves its own stream, as before.
    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    let ended = ends_when_stopped(&h, &webrtc);
    let moved = h.launch(0, true).await?;
    assert!(webrtc.stopping(), "its Moonlight stream takes over");
    let queued = h.h.sessions.lock().unwrap().pending.remove(&moved.id);
    drop((queued, moved));
    ended.join().unwrap();
    forget_stream(&h, webrtc);

    // Beside a Moonlight stream the join runs, and both go on.
    let first = h.launch(0, true).await?;
    let (stream, client) = h.play(&first).await?;
    let second = h.launch(1, true).await?;
    assert!(!stream.stopping());
    let (joined, joined_client) = h.play(&second).await?;
    h.counts(2, 0).await?;
    assert!(!stream.stopping());
    assert!(!joined.stopping());
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    // The join leaving takes nothing from the owner either.
    h.rtsp(&joined.launch, 3, "TEARDOWN", "").await?;
    until("the join ended", WAIT * 3, || {
        joined_client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert!(!stream.stopping());
    drop((joined, joined_client));
    h.end(&stream, client).await?;
    h.finish().await
}

/// A resume is of the running game, whatever it names: a prompt left open while the game changed
/// names the old one. Another device's stale name is refused, since the user did not choose this
/// game; the owner's resume, and one that names nothing as stock Moonlight does, go to the game.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_naming_another_game_is_refused_for_a_non_owner_and_gets_the_game_for_its_owner()
-> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let mut running = App::desktop();
    running.extra.insert("uuid".into(), "game-y".into());
    let mut stale = running.clone();
    stale.name = "Game X".into();
    stale.extra.insert("uuid".into(), "game-x".into());
    let (stale_id, running_id) = (stale.id(), running.id());
    assert_ne!(stale_id, running_id);
    // Earlier in the list: the query's id and uuid both name it.
    *h.h.apps.write().unwrap() = vec![stale, running.clone()];
    *h.h.current_app.lock().unwrap() = Some(crate::process::RunningApp::with_environment(
        &running,
        Default::default(),
    )?);
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = "0".into();
    let resume = |client: usize, uuid: &'static str| {
        let h = &h;
        async move {
            let extra = [("appuuid", uuid)];
            let extra: &[(&str, &str)] = if uuid.is_empty() { &[] } else { &extra };
            h.launch_response_with(client, true, None, extra).await
        }
    };
    let joins_the_game = |reply: String, client: usize| {
        assert!(reply.contains("status_code=\"200\""), "{reply}");
        let launch = pending_of(&h, client).expect("a launch");
        assert_eq!(launch.app_id, running_id);
        let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop((queued, launch));
    };

    let refused = resume(1, "game-x").await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(crate::nvhttp::RESUME_GAME_CHANGED),
        "{refused}"
    );
    assert!(pending_of(&h, 1).is_none());
    // The owner's name for its own game does not decide which game it is.
    joins_the_game(resume(0, "game-x").await?, 0);
    // Nothing named, the game's own name, and a Resume tile's.
    joins_the_game(resume(1, "").await?, 1);
    joins_the_game(resume(1, "game-y").await?, 1);
    joins_the_game(resume(1, "9a1c5a25-58fe-40e0-b9aa-7d3f00000001").await?, 1);
    // A game listed with a new id since it started is still found, and is the same game.
    h.h.current_app.lock().unwrap().as_mut().unwrap().id = 7777;
    h.h.apps.write().unwrap()[1].aliases.push(7777);
    joins_the_game(resume(1, "game-y").await?, 1);

    *h.h.apps.write().unwrap() = vec![running.clone()];
    h.h.current_app.lock().unwrap().as_mut().unwrap().id = running_id;
    h.finish().await
}

/// A game closed while a resume prepares its display is not started by that resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_game_closed_while_a_resume_prepares_stays_closed() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = "0".into();
    let host = h.h.clone();
    *h.fixture.during_display_preparation.lock().unwrap() = Some(Box::new(move || {
        *host.current_app.lock().unwrap() = None;
    }));
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"503\""), "{refused}");
    assert!(
        refused.contains(crate::nvhttp::NOTHING_TO_RESUME),
        "{refused}"
    );
    assert!(h.h.current_app.lock().unwrap().is_none(), "nothing started");
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert!(pending_of(&h, 1).is_none());
    h.finish().await
}
