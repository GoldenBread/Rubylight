use super::*;
use butterpollo_session::config_ext::ConfigExt;
use butterpollo_stream::state::test_support::Fixture;
use serde_json::{Value, json};

#[test]
fn a_pages_connection_failure_reaches_a_default_log_once_per_session() {
    let mut throttle = Throttle::new();
    let mut reported = false;
    assert_eq!(
        client_line_is_loud("decoder started", &mut reported, &mut throttle),
        None
    );
    assert!(!reported);
    assert_eq!(
        client_line_is_loud(
            "connection failed: no-answer, offered 192.168.4.69:8001, Firefox/159.0",
            &mut reported,
            &mut throttle
        ),
        Some(0)
    );
    // The limit is past: the same session still says it once.
    throttle.rewind_for_test(Duration::from_secs(11));
    assert_eq!(
        client_line_is_loud(
            "connection failed: the same page again",
            &mut reported,
            &mut throttle
        ),
        None
    );
}

#[test]
fn a_page_reconnecting_in_a_loop_writes_one_warning_in_ten_seconds() {
    let mut throttle = Throttle::new();
    let failure = |throttle: &mut Throttle| {
        // Every reconnect is a new connection, whose flag is clear.
        client_line_is_loud("connection failed: no-answer", &mut false, throttle)
    };
    assert_eq!(failure(&mut throttle), Some(0));
    assert_eq!(failure(&mut throttle), None);
    assert_eq!(failure(&mut throttle), None);
    throttle.rewind_for_test(Duration::from_secs(11));
    assert_eq!(failure(&mut throttle), Some(2));
}

#[tokio::test]
async fn a_page_that_tries_again_on_its_socket_can_report_its_failure_again() {
    let f = Fixture::new();
    let h = f.host.clone();
    let clients = crate::WebRtcRuntime::for_host(&h).clients;
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let Registered {
        id,
        closed: _closed,
    } = clients.open(peer.address.ip()).unwrap();
    let mut state = Connection::new(h.clone(), clients.clone(), Limits::default(), peer, id);
    let device = clients.console_device(&h, "owner").unwrap();
    state.client = Some(Authenticated {
        client_id: device.webrtc().unwrap().0.to_owned(),
        name: device.name.clone(),
    });
    let mut out = Vec::new();
    let failure = json!({"version":2,"type":"client-log","lines":["connection failed: no-try"]});
    let settings = r#""appId":"0","video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},"audio":{"channels":2}"#;
    for kind in ["start-session", "switch-session"] {
        state.handle(&failure.to_string(), &mut out).await;
        assert!(state.reported_failure, "{out:?}");
        let again = format!(r#"{{"version":2,"type":"{kind}",{settings}}}"#);
        state.handle(&again, &mut out).await;
        assert!(!state.reported_failure, "{kind}: {out:?}");
    }
}

#[test]
fn a_refusal_log_is_one_line_in_ten_seconds_with_the_skipped_count() {
    let mut throttle = Throttle::new();
    assert_eq!(throttle.allow(), Some(0));
    assert_eq!(throttle.allow(), None);
    assert_eq!(throttle.allow(), None);
    throttle.rewind_for_test(Duration::from_secs(11));
    assert_eq!(throttle.allow(), Some(2));
    assert_eq!(throttle.allow(), None);
}

#[test]
fn a_pairing_name_may_not_spell_a_pin_in_any_digits() {
    for name in [
        "Living room TV - PIN 4821",
        "TV 4-8-2-1",
        "Xbox, PIN ④⑧②①",
        "TV \u{ff14}",
        "TV ²",
        "TV \u{1d7d2}",
        "TV \u{0664}\u{0668}",
    ] {
        assert!(name_problem("4821", name).is_some(), "{name}");
    }
    for name in [
        "Living room TV 2024",
        "Xbox",
        "Edge on Windows",
        "SM-F966B",
        "Fernseher Wohnzimmer 📺",
        "客厅电视",
        "Télé du salon",
    ] {
        assert_eq!(name_problem("4821", name), None, "{name}");
    }
}

#[test]
fn a_pairing_name_may_not_hide_reorder_or_break_its_text() {
    for c in [
        '\u{00ad}',
        '\u{061c}',
        '\u{180e}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202b}',
        '\u{202c}',
        '\u{202d}',
        '\u{202e}',
        '\u{2060}',
        '\u{2066}',
        '\u{2067}',
        '\u{2068}',
        '\u{2069}',
        '\u{feff}',
        '\u{e0001}',
        '\u{e0041}',
    ] {
        let name = format!("{c}Pairing request from 192.168.4.20");
        assert_eq!(
            name_problem("4821", &name),
            Some("clientName may not contain format or line separator characters"),
            "U+{:04X}",
            c as u32
        );
    }
}

#[tokio::test]
async fn a_connection_task_that_panics_or_is_aborted_frees_its_slots() {
    let f = Fixture::new();
    let h = f.host.clone();
    let clients = crate::WebRtcRuntime::for_host(&h).clients;
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let ip = peer.address.ip();
    // A connection with a pairing request, as a TV waiting for its PIN has.
    let claim = |h: &Shared| {
        let Registered { id, closed } = clients.open(ip).expect("a slot");
        let mut state = Connection::new(h.clone(), clients.clone(), Limits::default(), peer, id);
        let (request_id, entered) = clients.begin_pairing(h, "4321", "TV", ip);
        state.pairing = Some(Pairing {
            request_id,
            entered,
            deadline: tokio::time::Instant::now() + Duration::from_secs(60),
        });
        (state, closed)
    };
    let (state, closed) = claim(&h);
    assert_eq!(clients.slots(), 1);
    assert_eq!(h.pins.lock().unwrap().len(), 1);
    let panicked = tokio::spawn(async move {
        let _held = (state, closed);
        panic!("a connection task failed");
    });
    assert!(panicked.await.unwrap_err().is_panic());
    assert_eq!(clients.slots(), 0);
    assert_eq!(clients.connections(), 0);
    assert!(h.pins.lock().unwrap().is_empty());

    let (state, closed) = claim(&h);
    let waiting = tokio::spawn(async move {
        let _held = (state, closed);
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    assert_eq!(clients.slots(), 1);
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(clients.slots(), 0);
    assert_eq!(clients.connections(), 0);
    assert!(h.pins.lock().unwrap().is_empty());
    // The peer has its whole share again.
    let held: Vec<_> = (0..4).map(|_| clients.open(ip).unwrap()).collect();
    assert_eq!(held.len(), 4);
}

#[tokio::test]
async fn a_taken_port_is_reported_not_moved() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let f = Fixture::new();
    assert!(start_on(f.host.clone(), [127, 0, 0, 1].into(), port, 0, None).is_none());
    let status = crate::WebRtcRuntime::for_host(&f.host).clients.status();
    assert!(status.enabled);
    assert!(status.signalling.is_none());
    assert!(status.error.unwrap().contains(&port.to_string()));
    assert!(f.host.warnings.snapshot().iter().any(|w| w.code == WARNING));
}

#[tokio::test]
async fn with_the_setting_off_nothing_listens() {
    let f = Fixture::new();
    assert!(!f.host.config.read().unwrap().webrtc_server());
    assert!(start(f.host.clone(), [127, 0, 0, 1].into(), None).is_none());
    let status = crate::WebRtcRuntime::for_host(&f.host).clients.status();
    assert!(!status.enabled);
    assert!(status.signalling.is_none() && status.discovery.is_none());
    assert!(!f.host.warnings.snapshot().iter().any(|w| w.code == WARNING));
}

#[test]
fn only_the_local_network_may_connect() {
    for peer in [
        "127.0.0.1",
        "10.0.0.5",
        "172.16.1.1",
        "192.168.1.9",
        "169.254.1.1",
        "100.64.0.1",
        "::1",
        "fd12::1",
        "fe80::1",
        "::ffff:192.168.1.9",
    ] {
        assert!(local_network(peer.parse().unwrap()), "{peer}");
    }
    for peer in [
        "8.8.8.8",
        "172.32.0.1",
        "100.128.0.1",
        "2001:db8::1",
        "::ffff:8.8.8.8",
    ] {
        assert!(!local_network(peer.parse().unwrap()), "{peer}");
    }
}

/// A test host can stream rates that are not offered by default; nothing else changes them.
#[test]
fn the_frame_rate_setting_replaces_the_offer_for_every_mode() {
    let mut config = butterpollo_config::config::Config::default();
    assert_eq!(frame_rate_offer(&config), FrameRateOffer::default());
    let saved = serde_json::json!({"webrtc_frame_rates": "30, 60,120,x"});
    config.update(saved.as_object().unwrap()).unwrap();
    let offer = frame_rate_offer(&config);
    assert_eq!(offer, FrameRateOffer::uniform(&[30, 60, 120]));
    assert_ne!(offer, FrameRateOffer::default());
}

/// An authenticated connection hears `capabilities` again when the probe or the rate setting
/// changes what is on offer, and only then.
#[test]
fn capabilities_are_sent_again_when_the_probe_or_the_rate_setting_changes() {
    let f = Fixture::new();
    let h = f.host.clone();
    let clients = crate::WebRtcRuntime::for_host(&h).clients;
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let Registered {
        id,
        closed: _closed,
    } = clients.open(peer.address.ip()).unwrap();
    let mut state = Connection::new(h.clone(), clients, Limits::default(), peer, id);
    let refresh = |state: &mut Connection| {
        let mut out = Vec::new();
        state.refresh_capabilities(&mut out);
        out
    };
    // Nothing before authentication.
    assert!(refresh(&mut state).is_empty());
    state.client = Some(Authenticated {
        client_id: "client".into(),
        name: "TV".into(),
    });
    let mut out = Vec::new();
    state.startup(&mut out);
    assert!(matches!(out.last(), Some(ServerMessage::Capabilities(_))));
    assert!(refresh(&mut state).is_empty(), "nothing changed");
    let rates = |values: &str| {
        let saved = json!({ "webrtc_frame_rates": values });
        h.config
            .write()
            .unwrap()
            .update(saved.as_object().unwrap())
            .unwrap();
    };
    rates("30,60");
    assert_ne!(
        frame_rate_offer(&h.config.read().unwrap()),
        FrameRateOffer::default()
    );
    let again = refresh(&mut state);
    assert!(
        matches!(again.as_slice(), [ServerMessage::Capabilities(_)]),
        "{again:?}"
    );
    assert!(refresh(&mut state).is_empty(), "sent once");
    // A setting that parses to the same rates changes nothing on offer.
    rates("30, 60");
    assert!(refresh(&mut state).is_empty());
    // The probe finds AV1, or loses it.
    h.codecs
        .fetch_xor(0x10000, std::sync::atomic::Ordering::AcqRel);
    assert!(matches!(
        refresh(&mut state).as_slice(),
        [ServerMessage::Capabilities(_)]
    ));
    assert!(refresh(&mut state).is_empty());
}

/// The host's AMF throughput caps gate the frame rates it lists and accepts.
#[test]
fn the_offer_follows_the_amf_throughput() {
    let mbs = |w: u64, h: u64, fps: u64| w.div_ceil(16) * h.div_ceil(16) * fps;
    let rates = |probe: EncoderProbe, height: u32| {
        HostCapabilities::new(probe, FrameRateOffer::default())
            .capabilities()
            .video_modes
            .into_iter()
            .find(|m| m.height == height)
            .map(|m| m.frame_rates)
    };
    // No cap reported: 60 fps and below, as before the probe.
    let unknown = encoder_probe(0x10000 | 0x20000 | 0x200, [0; 3]);
    assert_eq!(rates(unknown, 2160), Some(vec![30, 60]));
    assert_eq!(rates(unknown, 720), Some(vec![30, 60]));
    let full = encoder_probe(0x10000 | 0x20000 | 0x200, [mbs(3840, 2160, 120); 3]);
    assert_eq!(rates(full, 2160), Some(vec![30, 60, 90, 120]));
    let slow = encoder_probe(0x10000 | 0x20000 | 0x200, [mbs(3840, 2160, 60); 3]);
    assert_eq!(slow.hevc_mb_rate, mbs(3840, 2160, 60));
    assert_eq!(rates(slow, 2160), Some(vec![30, 60]));
    assert_eq!(rates(slow, 1440), Some(vec![30, 60, 90, 120]));
}

/// The capabilities list follows the host's AMF probe bits, not the protocol table.
#[test]
fn capabilities_follow_the_amf_probe_bits() {
    let listed = |bits: u32| {
        let body = HostCapabilities::new(encoder_probe(bits, [0; 3]), FrameRateOffer::default())
            .capabilities();
        let value: Value =
            serde_json::from_str(&ServerMessage::Capabilities(body).to_json()).unwrap();
        let all = |field: &str| -> Vec<(String, Vec<String>)> {
            value["videoModes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    (
                        m["height"].to_string(),
                        m[field]
                            .as_array()
                            .map(|a| a.iter().map(|c| c.as_str().unwrap().into()).collect())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        };
        (all("codecs"), all("hdrCodecs"))
    };
    let has = |modes: &[(String, Vec<String>)], codec: &str| {
        modes
            .iter()
            .any(|(_, codecs)| codecs.iter().any(|c| c == codec))
    };
    // H.264 and HEVC only: no AV1 anywhere.
    let (codecs, hdr) = listed(0x101);
    assert!(has(&codecs, "hevc") && !has(&codecs, "av1"));
    assert!(!has(&hdr, "av1") && !has(&hdr, "hevc"), "no Main10, no HDR");
    // HEVC Main10 adds HEVC HDR only.
    let (_, hdr) = listed(0x301);
    assert!(has(&hdr, "hevc") && !has(&hdr, "av1"));
    // AV1 8-bit lists AV1 but not as HDR.
    let (codecs, hdr) = listed(0x10101);
    assert!(has(&codecs, "av1") && !has(&hdr, "av1"));
    // AV1 with 10-bit is HDR too.
    let (codecs, hdr) = listed(0x30101);
    assert!(has(&codecs, "av1") && has(&hdr, "av1"));
}
