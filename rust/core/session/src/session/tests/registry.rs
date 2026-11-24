//! Tests for session launch queueing, expiry, RTSP claims, teardown, roles, and transports.
use super::*;

#[test]
fn a_queued_launch_and_a_started_stream_wake_the_waker_and_nothing_else_does() {
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut sessions = Sessions::default().with_waker({
        let woken = woken.clone();
        move || {
            woken.fetch_add(1, Ordering::AcqRel);
        }
    });
    let count = || woken.load(Ordering::Acquire);
    drop(sessions.expire());
    assert_eq!(count(), 0);
    let fresh = launch("woken", Role::Stream);
    drop(sessions.queue(fresh.clone()).unwrap());
    assert_eq!(count(), 1, "a queued launch");
    let refused = launch("refused", Role::Stream);
    assert!(sessions.start(refused, Negotiated::default()).is_err());
    assert_eq!(count(), 1, "a start that fails changes nothing");
    let s = sessions.start(fresh, Negotiated::default()).unwrap();
    assert_eq!(count(), 2, "a started stream");
    sessions.begin_teardown(&s);
    drop(sessions.request_stop(None));
    assert_eq!(count(), 2);
    // Without one, nothing is told.
    let mut plain = Sessions::default();
    drop(plain.queue(launch("plain", Role::Stream)).unwrap());
    assert_eq!(count(), 2);
}
#[test]
fn a_launch_still_being_prepared_does_not_expire() {
    let mut sessions = Sessions::default();
    let mut slow = launch("slow", Role::Stream);
    slow.created = Instant::now() - Duration::from_secs(90);
    slow.preparing.store(true, Ordering::Release);
    sessions.pending.insert(slow.id.clone(), slow.clone());
    drop(sessions.expire());
    assert!(sessions.pending.contains_key("slow"));
    // Prepared: the client's 30 s count from then.
    slow.preparing.store(false, Ordering::Release);
    drop(sessions.expire());
    assert!(!sessions.pending.contains_key("slow"));
}
#[test]
fn starting_rechecks_launch_expiry_after_rtsp_authorization() {
    let mut sessions = Sessions::default();
    let fresh = launch("expired-before-play", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    let authorized = sessions.pending_for_peer(fresh.peer).unwrap();
    // The RTSP connection can wait for input after taking its authorized
    // snapshot. PLAY must check the current launch, even before housekeeping.
    sessions.pending.get_mut(&fresh.id).unwrap().created = Instant::now() - Duration::from_secs(31);
    assert!(sessions.start(authorized, Negotiated::default()).is_err());
    assert!(sessions.active.is_empty());
    // Keep preparation ownership for expire() to return to its caller;
    // restoring display/audio resources must happen outside the session lock.
    assert!(sessions.pending.contains_key(&fresh.id));
    assert_eq!(sessions.expire().len(), 1);

    let fresh = launch("valid-play", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    assert!(sessions.start(fresh.clone(), Negotiated::default()).is_ok());
    assert!(sessions.start(fresh, Negotiated::default()).is_err());
}
#[test]
fn a_launch_takes_rtsp_only_once_prepared_and_starts_one_session() {
    let mut sessions = Sessions::default();
    let early = launch("early", Role::Stream);
    early.preparing.store(true, Ordering::Release);
    sessions.pending.insert(early.id.clone(), early.clone());
    // Its client connects only after the reply: nothing may claim or
    // start it while the display and app are still being prepared.
    assert!(sessions.rtsp_for_peer(early.peer).is_empty());
    assert!(sessions.pending_for_peer(early.peer).is_err());
    let refused = sessions.claim("early").unwrap_err().to_string();
    assert!(refused.contains("still being prepared"), "{refused}");
    assert!(
        sessions
            .start(early.clone(), Negotiated::default())
            .is_err()
    );
    assert!(sessions.active.is_empty() && sessions.pending.contains_key("early"));
    drop(sessions.expire());
    assert!(sessions.pending.contains_key("early"));

    early.preparing.store(false, Ordering::Release);
    assert_eq!(sessions.rtsp_for_peer(early.peer).len(), 1);
    sessions.claim("early").unwrap();
    let session = sessions
        .start(early.clone(), Negotiated::default())
        .unwrap();
    // A second PLAY, or a claim of the consumed launch, starts nothing.
    assert!(sessions.start(early, Negotiated::default()).is_err());
    assert_eq!(sessions.active.len(), 1);
    // The running stream's own requests (TEARDOWN) still pass.
    sessions.claim("early").unwrap();
    sessions.begin_teardown(&session);
    assert!(sessions.claim("early").is_err());
}
#[test]
fn a_claimed_handshake_outlives_the_30_s_by_its_lease_up_to_the_limit() {
    let mut sessions = Sessions::default();
    let claimed = launch("claimed", Role::Stream);
    let unclaimed = launch("unclaimed", Role::Stream);
    sessions.pending.insert(claimed.id.clone(), claimed.clone());
    sessions
        .pending
        .insert(unclaimed.id.clone(), unclaimed.clone());
    sessions.claim("claimed").unwrap();
    let until = sessions.pending["claimed"].claimed_until.unwrap();
    assert!(until > Instant::now() + RTSP_LEASE - Duration::from_secs(1));
    // 31 s after the reply: the claimed handshake goes on, the other expired.
    for id in ["claimed", "unclaimed"] {
        sessions.pending.get_mut(id).unwrap().created = Instant::now() - Duration::from_secs(31);
    }
    let expired = sessions.expire();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, "unclaimed");
    assert!(sessions.claim("unclaimed").is_err());
    sessions.claim("claimed").unwrap();
    // A lease never reaches past the limit, and an ended lease expires.
    let at = sessions.pending["claimed"].created;
    assert_eq!(
        sessions.pending["claimed"].lease(at + CLAIM_LIMIT - Duration::from_secs(1)),
        at + CLAIM_LIMIT
    );
    sessions.pending.get_mut("claimed").unwrap().claimed_until =
        Some(Instant::now() - Duration::from_millis(1));
    let ended = sessions.claim("claimed").unwrap_err().to_string();
    assert!(ended.contains("expired"), "{ended}");
    sessions.pending.get_mut("claimed").unwrap().claimed_until = Some(Instant::now() + RTSP_LEASE);
    sessions.pending.get_mut("claimed").unwrap().created = Instant::now() - CLAIM_LIMIT;
    assert!(sessions.start(claimed, Negotiated::default()).is_err());
    assert_eq!(sessions.expire().len(), 1);
    assert!(sessions.pending.is_empty() && sessions.active.is_empty());
}
#[test]
fn a_reconnect_replaces_a_claimed_launch_which_then_starts_nothing() {
    let mut sessions = Sessions::default();
    let first = launch("first", Role::Stream);
    drop(sessions.queue(first.clone()).unwrap());
    sessions.claim("first").unwrap();
    // The same device launches again while the first handshake is under way.
    let replaced = sessions.queue(launch("second", Role::Stream)).unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].id, "first");
    assert!(sessions.claim("first").is_err());
    assert!(sessions.start(first, Negotiated::default()).is_err());
    sessions.claim("second").unwrap();
    let second = sessions.pending["second"].clone();
    sessions.start(second, Negotiated::default()).unwrap();
    assert_eq!(sessions.active.keys().collect::<Vec<_>>(), ["second"]);
}
#[test]
fn rtsp_authorization_does_not_keep_canceled_preparations_alive() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicU32>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl AudioPreparation<()> for Restore {
        fn route(&self) -> Arc<()> {
            Arc::new(())
        }
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.fetch_add(1, Ordering::Release);
        }
    }
    for expires in [false, true] {
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let restored = Arc::new(AtomicU32::new(0));
        let pending = launch("pending", Role::Stream);
        *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        *pending.audio_preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        let (authorized, canceled) = {
            let mut locked = sessions.lock().unwrap();
            let peer = pending.peer;
            locked.queue(pending).unwrap();
            let authorized = locked.rtsp_for_peer(peer);
            let canceled = if expires {
                locked.pending.get_mut("pending").unwrap().created =
                    Instant::now() - Duration::from_secs(31);
                locked.expire()
            } else {
                locked.request_stop(Some("pending"))
            };
            (authorized, canceled)
        };
        drop(canceled);
        assert_eq!(
            restored.load(Ordering::Acquire),
            2,
            "an RTSP socket must not retain canceled display and audio preparation"
        );
        assert_eq!(authorized[0].id, "pending");
    }
}
#[test]
fn play_consumes_the_current_pending_launch_with_its_preparation() {
    struct Prepared;
    impl Preparation<()> for Prepared {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    let mut sessions = Sessions::default();
    let pending = launch("prepared", Role::Stream);
    let peer = pending.peer;
    *pending.preparation.lock().unwrap() = Some(Box::new(Prepared));
    sessions.queue(pending).unwrap();
    let authorized = sessions.rtsp_for_peer(peer).pop().unwrap();
    sessions
        .pending
        .get_mut("prepared")
        .unwrap()
        .options
        .insert("server_state".into(), "current".into());
    let session = sessions.start(authorized, Negotiated::default()).unwrap();
    assert_eq!(
        session
            .launch
            .options
            .get("server_state")
            .map(String::as_str),
        Some("current")
    );
    assert!(session.launch.preparation.lock().unwrap().is_some());
    assert!(sessions.pending.is_empty());
}
#[test]
fn expired_launches_restore_resources_after_unlocking_sessions() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicBool>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.store(true, Ordering::Release);
        }
    }
    let sessions = Arc::new(Mutex::new(Sessions::default()));
    let restored = Arc::new(AtomicBool::new(false));
    let mut expired = launch("expired", Role::Stream);
    expired.created = Instant::now() - Duration::from_secs(31);
    *expired.preparation.lock().unwrap() = Some(Box::new(Restore {
        sessions: Arc::downgrade(&sessions),
        restored: restored.clone(),
    }));
    let expired = {
        let mut locked = sessions.lock().unwrap();
        locked.pending.insert(expired.id.clone(), expired);
        let live = launch("live", Role::InputOnly);
        locked.queue(live.clone()).unwrap();
        assert_eq!(locked.pending_for_peer(live.peer).unwrap().id, "live");
        assert_eq!(locked.rtsp_for_peer(live.peer).len(), 1);
        let expired = locked.expire();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "expired");
        assert!(locked.pending.contains_key("live"));
        assert!(!restored.load(Ordering::Acquire));
        expired
    };
    assert!(!restored.load(Ordering::Acquire));
    drop(expired);
    assert!(restored.load(Ordering::Acquire));
}
#[test]
fn canceled_launches_restore_resources_after_unlocking_sessions() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicBool>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.store(true, Ordering::Release);
        }
    }
    let cancellations: [fn(&mut Sessions) -> Vec<Launch>; 7] = [
        |s| s.supersede("client", Role::Stream),
        |s| s.request_stop(None),
        |s| s.request_stop(Some("pending")),
        |s| s.request_stop(Some("client")),
        |s| s.stop_role(Role::Stream, None),
        |s| s.stop_role(Role::Stream, Some("client")),
        |s| s.queue(launch("replacement", Role::Stream)).unwrap(),
    ];
    for cancel in cancellations {
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let restored = Arc::new(AtomicBool::new(false));
        let pending = launch("pending", Role::Stream);
        *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        let canceled = {
            let mut locked = sessions.lock().unwrap();
            locked.pending.insert(pending.id.clone(), pending);
            let canceled = cancel(&mut locked);
            assert_eq!(canceled.len(), 1);
            assert!(!locked.pending.contains_key("pending"));
            assert!(!restored.load(Ordering::Acquire));
            canceled
        };
        assert!(!restored.load(Ordering::Acquire));
        drop(canceled);
        assert!(restored.load(Ordering::Acquire));
    }
}
#[test]
fn pending_launches_and_streams_keep_sessions_busy_until_they_expire_or_end() {
    let mut sessions = Sessions::default();
    assert!(sessions.idle());
    let mut pending = launch("pending", Role::Stream);
    sessions.pending.insert(pending.id.clone(), pending.clone());
    assert!(!sessions.idle());
    pending.created = Instant::now() - Duration::from_secs(31);
    sessions.pending.insert(pending.id.clone(), pending.clone());
    assert!(sessions.idle());
    assert!(sessions.owns_capture());
    pending.preparing.store(true, Ordering::Release);
    assert!(!sessions.idle());
    pending.preparing.store(false, Ordering::Release);
    let fresh = launch("fresh", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    sessions.start(fresh, Negotiated::default()).unwrap();
    assert!(!sessions.idle());
    sessions.active.clear();
    sessions.teardown.insert("ended".into(), Instant::now());
    assert!(!sessions.idle());
    sessions.teardown.clear();
    assert!(sessions.idle());
}
#[test]
fn a_worker_that_cannot_start_fails_without_leaving_an_active_session() {
    let mut sessions = Sessions::default();
    let failed = launch("failed", Role::Stream);
    sessions.queue(failed.clone()).unwrap();
    let failed = sessions.start(failed, Negotiated::default()).unwrap();
    let other = launch("other", Role::InputOnly);
    sessions.queue(other.clone()).unwrap();
    let other = sessions.start(other, Negotiated::default()).unwrap();
    sessions.fail_start(&failed, "the stream worker did not start");
    assert!(failed.failed());
    assert!(failed.stopping());
    assert_eq!(failed.termination_reason(), 0x8000_4005);
    assert!(!sessions.active.contains_key("failed"));
    assert!(sessions.active.contains_key("other"));
    assert!(!other.stopping());
    assert!(sessions.teardown.is_empty());
    let retry = launch("retry", Role::Stream);
    sessions.queue(retry.clone()).unwrap();
    sessions.start(retry, Negotiated::default()).unwrap();
}
#[test]
fn teardown_leaves_active_while_resources_remain_owned() {
    use std::sync::{Mutex, Weak};
    struct Resource {
        sessions: Weak<Mutex<Sessions>>,
        released: Arc<AtomicBool>,
    }
    impl Preparation<()> for Resource {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            let sessions = self.sessions.upgrade().unwrap();
            let sessions = sessions.try_lock().unwrap();
            assert!(!sessions.active.contains_key("old"));
            assert!(sessions.teardown.contains_key("old"));
            self.released.store(true, Ordering::Release);
        }
    }
    let sessions = Arc::new(Mutex::new(Sessions::default()));
    let released = Arc::new(AtomicBool::new(false));
    let old = launch("old", Role::Stream);
    *old.preparation.lock().unwrap() = Some(Box::new(Resource {
        sessions: Arc::downgrade(&sessions),
        released: released.clone(),
    }));
    let old = {
        let mut sessions = sessions.lock().unwrap();
        sessions.queue(old.clone()).unwrap();
        sessions.start(old, Negotiated::default()).unwrap()
    };
    old.stop();
    {
        let mut sessions = sessions.lock().unwrap();
        sessions.begin_teardown(&old);
        assert!(sessions.active.is_empty());
        assert!(sessions.owns_capture());
        assert!(!sessions.idle());
        let mut next = launch("next", Role::Stream);
        next.app_id = 2;
        sessions.queue(next.clone()).unwrap();
        sessions.start(next, Negotiated::default()).unwrap();
        assert!(!released.load(Ordering::Acquire));
    }
    drop(old);
    assert!(released.load(Ordering::Acquire));
    let mut sessions = sessions.lock().unwrap();
    sessions.teardown.remove("old");
    sessions.active.clear();
    assert!(sessions.idle());
    assert!(!sessions.owns_capture());
}
#[test]
fn independent_roles_and_targeted_teardown_preserve_other_sessions() {
    let mut sessions = Sessions::default();
    sessions.queue(launch("game", Role::Stream)).unwrap();
    sessions
        .queue(launch("monitor", Role::RemoteMonitor))
        .unwrap();
    sessions.queue(launch("input", Role::InputOnly)).unwrap();
    // A client launching again replaces its own earlier launch in that role.
    sessions
        .queue(launch("duplicate", Role::RemoteMonitor))
        .unwrap();
    assert!(!sessions.pending.contains_key("monitor"));
    assert!(sessions.pending.contains_key("duplicate"));
    let game = sessions
        .start(launch("game", Role::Stream), Negotiated::default())
        .unwrap();
    let input = sessions
        .start(launch("input", Role::InputOnly), Negotiated::default())
        .unwrap();
    drop(sessions.stop_role(Role::RemoteMonitor, Some("client")));
    assert!(sessions.pending.is_empty());
    assert!(!game.stopping());
    assert!(!input.stopping());
    drop(sessions.stop_role(Role::Stream, None));
    assert!(game.stopping());
    assert!(!input.stopping());
    drop(sessions.request_stop(Some("client")));
    assert!(input.stopping());
}
#[test]
fn a_new_launch_replaces_an_unconnected_launch_and_stops_an_abandoned_stream() {
    let mut sessions = Sessions::default();
    // A launch whose RTSP never arrived must not block the next attempt.
    sessions.queue(launch("first", Role::Stream)).unwrap();
    sessions.queue(launch("retry", Role::Stream)).unwrap();
    assert_eq!(sessions.pending.keys().collect::<Vec<_>>(), ["retry"]);
    // A stream the client abandoned is stopped when it launches again.
    let old = sessions
        .start(launch("retry", Role::Stream), Negotiated::default())
        .unwrap();
    sessions.queue(launch("resume", Role::Stream)).unwrap();
    assert!(old.stopping());
    assert!(sessions.pending.contains_key("resume"));
    // Another client's stream is untouched.
    let mut other = launch("other", Role::Stream);
    other.client.uuid = "other-client".into();
    sessions.queue(other.clone()).unwrap();
    let other = sessions.start(other, Negotiated::default()).unwrap();
    sessions.queue(launch("again", Role::Stream)).unwrap();
    assert!(!other.stopping());
}
#[test]
fn a_session_of_either_transport_ends_the_other_one() {
    // A WebRTC session replaces a Moonlight stream and a pending Moonlight launch.
    let mut sessions = Sessions::default();
    sessions.queue(launch("moonlight", Role::Stream)).unwrap();
    let moonlight = sessions
        .start(launch("moonlight", Role::Stream), Negotiated::default())
        .unwrap();
    sessions.queue(launch("pending", Role::Stream)).unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
    assert_eq!(canceled.len(), 1);
    assert!(moonlight.stopping());
    assert!(sessions.pending.is_empty());
    // And a Moonlight launch replaces a WebRTC stream, from any device.
    sessions.queue(webrtc_launch("web", "tv")).unwrap();
    let web = sessions
        .start(webrtc_launch("web", "tv"), Negotiated::default())
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(web.stopping());
    // Sessions of the transport asking are left alone.
    sessions.queue(webrtc_launch("web2", "tv")).unwrap();
    let web2 = sessions
        .start(webrtc_launch("web2", "tv"), Negotiated::default())
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
    assert!(canceled.is_empty());
    assert!(!web2.stopping());
    // Another role is untouched too.
    sessions
        .queue(launch("monitor", Role::RemoteMonitor))
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(sessions.pending.contains_key("monitor"));
}
#[test]
fn a_nucleus_co_op_stream_is_never_ended_by_the_other_transport() {
    let seat = Seat {
        session: 1,
        slot: 1,
    };
    let mut sessions = Sessions::default();
    let mut player = webrtc_launch("player", "tv");
    player.nucleus = Some(seat);
    sessions.queue(player.clone()).unwrap();
    let player = sessions.start(player, Negotiated::default()).unwrap();
    let mut pending = launch("pending", Role::Stream);
    pending.nucleus = Some(seat);
    sessions.queue(pending).unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(!player.stopping());
    assert!(sessions.pending.contains_key("pending"));
}
#[test]
fn a_webrtc_stream_is_held_by_another_device_only_while_it_lives() {
    let mut sessions = Sessions::default();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
    sessions.queue(webrtc_launch("web", "tv")).unwrap();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
    assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
    // A Moonlight launch is not a WebRTC one.
    assert!(!sessions.held_by_other(Role::Stream, Transport::GameStream, "phone"));
    let web = sessions
        .start(webrtc_launch("web", "tv"), Negotiated::default())
        .unwrap();
    assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
    web.stop();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
}
