//! Unit tests for WebRTC session bridge, monotonic timestamps and audio layout selection.

use super::live::audio_layout;
use super::*;
use butterpollo_media_policy::{audio::OpusLayout, rtsp::webrtc_audio_kbps};
use butterpollo_session::session::Role;
use butterpollo_stream::state::test_support::Fixture;
use tokio::sync::mpsc::UnboundedReceiver;

#[test]
fn the_audio_layout_is_the_answered_one_at_the_bitrate_the_video_budget_paid_for() {
    let answered = |channels| {
        let (streams, coupled, mapping): (i32, i32, &[u8]) = match channels {
            6 => (4, 2, &[0, 4, 1, 2, 3, 5]),
            _ => (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
        };
        OpusLayout {
            channels: usize::from(channels),
            streams,
            coupled,
            mapping: mapping.to_vec(),
            bitrate: 1,
        }
    };
    // Stereo ignores the answer; the surround layouts keep theirs.
    let stereo = audio_layout(2, None).unwrap();
    assert_eq!((stereo.channels, stereo.bitrate), (2, 128_000));
    for channels in [6u8, 8] {
        let layout = audio_layout(channels, Some(answered(channels))).unwrap();
        let want = answered(channels);
        assert_eq!(
            (layout.streams, layout.coupled, &layout.mapping),
            (want.streams, want.coupled, &want.mapping)
        );
        // GameStream's surround bitrates, and what `Negotiated::webrtc` subtracts.
        assert_eq!(
            layout.bitrate,
            [256_000, 450_000][usize::from(channels / 8)]
        );
        assert_eq!(layout.bitrate, webrtc_audio_kbps(channels) as i32 * 1000);
        assert_eq!(
            layout.bitrate,
            OpusLayout::select(usize::from(channels), false, None)
                .unwrap()
                .bitrate
        );
        // No answered layout, or one for another channel count, is refused.
        assert!(audio_layout(channels, None).is_err());
        assert!(audio_layout(channels, Some(answered(if channels == 6 { 8 } else { 6 }))).is_err());
    }
    assert!(audio_layout(4, None).is_err());
}

fn bridge() -> (Arc<Bridge>, UnboundedReceiver<Event>) {
    let (events, received) = unbounded_channel();
    (Bridge::new(events), received)
}

fn session(f: &Fixture) -> Arc<Session> {
    let session = f.session(f.client(u32::MAX), Role::Stream);
    // A new session starts with its first keyframe pending.
    assert!(session.idr.swap(false, Ordering::AcqRel));
    session
}

fn requested(session: &Session) -> (bool, u64) {
    (
        session.idr.load(Ordering::Acquire),
        session.stats.idr_requests.load(Ordering::Acquire),
    )
}

#[test]
fn session_ids_are_positive_and_never_repeat() {
    let links = butterpollo_stream::webrtc_bridge::WebRtcLinks::default();
    let ids: Vec<u64> = (0..4).map(|_| links.next_session_id()).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    // A launch nobody put a transport for has nothing to take or discard.
    assert!(links.take("nothing").is_none());
    links.discard("nothing");
    assert_eq!(links.len(), 0);
}

#[test]
fn capture_instants_never_repeat_or_step_back() {
    let last = Cell::new(None);
    let base = Instant::now();
    let first = monotonic(&last, base);
    assert_eq!(first, base);
    // The same instant, and an earlier one, each move on by at least a tick.
    let repeated = monotonic(&last, base);
    assert!(repeated >= first + MIN_STEP);
    let earlier = monotonic(&last, base - Duration::from_millis(5));
    assert!(earlier >= repeated + MIN_STEP);
    // A later one is kept as it is.
    let later = base + Duration::from_secs(1);
    assert_eq!(monotonic(&last, later), later);
}

#[test]
fn a_pli_or_fir_is_the_clients_keyframe_request_and_the_rest_are_the_hosts() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    assert_eq!(requested(&session), (false, 0));
    for (n, reason) in [IdrReason::Pli, IdrReason::Fir].into_iter().enumerate() {
        session.idr.store(false, Ordering::Release);
        bridge.idr(reason);
        assert_eq!(requested(&session), (true, n as u64 + 1), "{reason:?}");
    }
    let before = requested(&session).1;
    for reason in [
        IdrReason::Connected,
        IdrReason::Reconnected,
        IdrReason::QueueFull,
        IdrReason::PacerBacklog,
        IdrReason::WaitingForKeyframe,
        IdrReason::WriteFailed,
    ] {
        session.idr.store(false, Ordering::Release);
        bridge.idr(reason);
        assert!(session.idr.load(Ordering::Acquire), "{reason:?}");
    }
    assert_eq!(requested(&session).1, before + 6);
}

#[test]
fn a_request_before_the_session_exists_is_ignored() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.idr(IdrReason::Pli);
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: false,
    });
    assert_eq!(requested(&session), (false, 0));
}

#[test]
fn the_control_channels_request_keyframe_asks_for_one_and_other_text_does_not() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    for text in [&br#"{"type":"idr"}"#[..], b"not json", b"{}"] {
        bridge.event(TransportEvent::Control {
            data: text.to_vec(),
            binary: false,
        });
    }
    // Binary frames on the control channel are not the JSON protocol.
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: true,
    });
    assert_eq!(requested(&session), (false, 0));
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: false,
    });
    assert_eq!(requested(&session), (true, 1));
}

#[test]
fn pad_announcements_on_the_control_channel_reach_the_gamepad_worker_and_state_does_not() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    let (input, mut received) = InputHandle::probe();
    bridge.attach_input(input);
    let connected = br#"{"v":2,"type":"gamepad-connected","controllerId":0}"#;
    let gone = br#"{"v":2,"type":"gamepad-disconnected","controllerId":0}"#;
    let state = br#"{"v":2,"type":"gamepad-state","controllerId":0}"#;
    for (data, binary) in [
        (&connected[..], false),
        (&state[..], false),
        (&gone[..], false),
        (&connected[..], true),
    ] {
        bridge.event(TransportEvent::Control {
            data: data.to_vec(),
            binary,
        });
    }
    let texts = received();
    assert_eq!(
        texts,
        [
            String::from_utf8_lossy(connected).into_owned(),
            String::from_utf8_lossy(gone).into_owned()
        ]
    );
    assert_eq!(requested(&session), (false, 0));
}

#[test]
fn a_transport_that_closes_ends_the_session_even_before_it_is_bound() {
    let f = Fixture::new();
    let (bound, _events) = bridge();
    let session = session(&f);
    bound.bind(&session);
    bound.event(TransportEvent::Closed);
    assert!(session.stopping());

    // Closed first: the session is stopped the moment it is bound.
    let (early, _events) = bridge();
    let late = Fixture::new();
    let session = self::session(&late);
    early.event(TransportEvent::Closed);
    assert!(!session.stopping());
    early.bind(&session);
    assert!(session.stopping());
}

#[test]
fn a_transport_that_connected_before_the_session_gets_its_keyframe_when_bound() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, mut events) = bridge();
    bridge.event(TransportEvent::Connected);
    assert!(matches!(events.try_recv(), Ok(Event::Connected)));
    assert_eq!(requested(&session), (false, 0));
    bridge.bind(&session);
    assert_eq!(requested(&session), (true, 1));
    // A path that drops and returns changes nothing by itself.
    bridge.event(TransportEvent::Disconnected);
    bridge.event(TransportEvent::Reconnected);
    assert!(!session.stopping());
}
