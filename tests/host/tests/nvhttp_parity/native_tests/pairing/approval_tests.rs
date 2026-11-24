//! Native API pairing approval, permission bounds, failure handling, and strike counting tests.
use super::*;

#[tokio::test]
async fn the_routes_need_a_paired_device_with_a_grant() {
    let f = Fixture::new();
    f.client(ALL);
    let (_, id) = asking(&f, "TV").await;
    let plain = other_device(&f, ALL, false);
    let stranger = Connection {
        certificate: None,
        ..f.connection(true)
    };
    for connection in [&plain, &stranger] {
        let code = if connection.certificate.is_some() {
            "permission_required"
        } else {
            "not_paired"
        };
        api(&f, connection, "GET", "/pairing/requests")
            .await
            .assert_error(403, code);
        approve(&f, connection, &id, "1234")
            .await
            .assert_error(403, code);
        send(
            &f,
            true,
            connection,
            "POST",
            &format!("{PREFIX}/pairing/deny"),
            None,
            serde_json::json!({"id": id}).to_string(),
        )
        .await
        .assert_error(403, code);
    }
    // No PIN reached the waiting device.
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn the_list_names_a_waiting_device_and_never_its_certificate() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let (client, id) = asking(&f, "LivingRoomTV").await;
    let reply = api(&f, &approver, "GET", "/pairing/requests").await;
    let body = reply.json();
    assert_eq!(body["requests"][0]["id"], id.as_str());
    assert_eq!(body["requests"][0]["name"], "LivingRoomTV");
    assert_eq!(body["requests"][0]["address"], PEER);
    assert!(body["requests"][0]["age_seconds"].is_u64());
    let text = reply.text();
    assert!(!text.contains("cert"), "{text}");
    assert!(!text.contains(&client.certificate), "{text}");
}

#[tokio::test]
async fn a_right_pin_pairs_with_the_consoles_default_and_answers_when_it_is_done() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["name"], "TV");
    let tv = saved(&f, "TV").unwrap();
    assert_eq!(tv.perm, PLAY_DEVICE_PERMISSION);
    assert!(tv.allows_coop_join());
    assert!(!tv.allows_host_control());
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn the_new_device_never_gets_more_than_its_approver_holds() {
    let f = Fixture::new();
    // Host control, but no input and no launching: the approver's to cap.
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(saved(&f, "TV").unwrap().perm, LATER_DEVICE_PERMISSION);
}

#[tokio::test]
async fn an_approver_with_nothing_of_play_is_refused_and_stores_no_dead_device() {
    let f = Fixture::new();
    // Host control and file transfer: it may reach the route, with no Play bit to give.
    let approver = other_device(&f, 0x0001_0000, true);
    let (_, id) = asking(&f, "TV").await;
    approve(&f, &approver, &id, "1234")
        .await
        .assert_error(403, "permission_required");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
    assert!(f.host.approvals.lock().unwrap().is_idle());
    let state = f.host.paired.read().unwrap();
    let refused = pairing::capped(
        &state,
        Some("other-device"),
        PLAY_DEVICE_PERMISSION,
        Default::default(),
    );
    // What it lost is named: it still may approve, and has nothing of Play to hand on.
    let refused = refused.err().unwrap();
    let revoked = refused.downcast_ref::<pairing::ApproverRevoked>();
    assert_eq!(revoked.unwrap().missing, pairing::LIST_AND_VIEW);
    let refused = pairing::capped(&state, Some("gone"), 7, Default::default()).err();
    let refused = refused.unwrap();
    let revoked = refused.downcast_ref::<pairing::ApproverRevoked>();
    assert_eq!(revoked.unwrap().missing, "Approve new devices");
}

#[tokio::test]
async fn a_device_the_pc_could_not_save_costs_its_address_nothing() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    // The saved state can't replace a directory.
    let _ = std::fs::remove_file(&f.host.paired_path);
    std::fs::create_dir_all(&f.host.paired_path).unwrap();
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    reply.assert_error(409, "pairing_unconfirmed");
    assert!(
        reply.text().contains("could not save TV"),
        "{}",
        reply.text()
    );
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
}

#[tokio::test]
async fn a_save_that_fails_logs_why_and_moonlight_hears_only_that_it_failed() {
    let f = Fixture::new();
    // The saved state can't replace a directory.
    let _ = std::fs::remove_file(&f.host.paired_path);
    std::fs::create_dir_all(&f.host.paired_path).unwrap();
    let path = f.host.directory.join("not-saved.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    let guard = butterpollo_stream::test_log::capture(subscriber);
    // No approval runs: a PIN typed in the console, the case nothing else logs.
    let (client, id) = asking(&f, "TV").await;
    let waiting = f.host.pins.lock().unwrap().remove(&id).unwrap();
    assert!(
        waiting
            .sender
            .send(("1234".into(), "TV".into(), None, None))
            .is_ok()
    );
    until("the handshake to start", || {
        f.host.pairings.lock().unwrap().sessions.contains_key(&id)
    })
    .await;
    let refused = finish_pairing(&f, &client, "TV", "1234").await.unwrap_err();
    drop(guard);
    assert_eq!(refused.to_string(), pairing::NotSaved.to_string());
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(
        log.contains("a Moonlight device could not be saved"),
        "{log}"
    );
    assert!(log.contains("os error"), "the save's own error: {log}");
}

#[tokio::test]
async fn a_step_a_device_fails_on_purpose_still_counts() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let (client, id) = asking(&f, "TV").await;
    let (reply, _) = tokio::join!(approve(&f, &approver, &id, "1234"), async {
        until("the handshake to start", || {
            f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        // The secret before any challenge: out of order.
        let mut request = device_request(&client, "TV", None);
        request.remove("phrase");
        request.insert("clientpairingsecret".into(), "00".repeat(272));
        let _ = do_pair(f.host.clone(), &request, PEER.parse().unwrap()).await;
    });
    reply.assert_error(422, "wrong_pin");
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 1);
}

#[tokio::test]
async fn a_pairing_from_the_console_clears_the_address_count() {
    let f = Fixture::new();
    let peer: std::net::IpAddr = PEER.parse().unwrap();
    let strike = |f: &Fixture| {
        let mut approvals = f.host.approvals.lock().unwrap();
        for _ in 0..3 {
            approvals.strike_at(peer);
        }
    };
    strike(&f);
    let (client, id) = asking(&f, "TV").await;
    let waiting = f.host.pins.lock().unwrap().remove(&id).unwrap();
    assert!(
        waiting
            .sender
            .send(("1234".into(), "TV".into(), None, None))
            .is_ok()
    );
    until("the handshake to start", || {
        f.host.pairings.lock().unwrap().sessions.contains_key(&id)
    })
    .await;
    finish_pairing(&f, &client, "TV", "1234").await.unwrap();
    assert!(saved(&f, "TV").is_some());
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);

    // A browser's pairing from the console does too.
    strike(&f);
    let webrtc = butterpollo_webrtc::WebRtcRuntime::for_host(&f.host);
    let (request, _) = webrtc
        .clients
        .begin_pairing(&f.host, "1234", "Browser", peer);
    let finished = webrtc
        .clients
        .finish_pairing(&f.host, &request, "1234", "Browser", None, peer);
    assert!(finished.is_ok());
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
}

#[tokio::test]
async fn an_approver_kept_out_of_coop_cannot_hand_it_on() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    {
        let mut state = f.host.paired.write().unwrap();
        let other = state
            .clients
            .iter_mut()
            .find(|c| c.name == "Other device")
            .unwrap();
        other.extra.insert("coop_join".into(), false.into());
    }
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let tv = saved(&f, "TV").unwrap();
    assert_eq!(tv.perm, PLAY_DEVICE_PERMISSION);
    assert!(!tv.allows_coop_join());
}

#[tokio::test]
async fn host_control_lost_during_the_handshake_refuses_the_new_device() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let (client, id) = asking(&f, "TV").await;
    let (reply, finished) = tokio::join!(approve(&f, &approver, &id, "1234"), async {
        until("the handshake to start", || {
            f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        f.host
            .paired
            .write()
            .unwrap()
            .clients
            .iter_mut()
            .find(|c| c.name == "Other device")
            .unwrap()
            .extra
            .remove("host_control");
        finish_pairing(&f, &client, "TV", "1234").await
    });
    assert!(finished.is_err());
    // The approver's doing, and said so; the device takes no strike for it.
    reply.assert_error(403, "permission_required");
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
    assert!(saved(&f, "TV").is_none());
}

#[tokio::test]
async fn a_browser_devices_approver_losing_the_grant_is_no_strike_either() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let (id, entered) =
        clients(&f.host).begin_pairing(&f.host, "4321", "Browser TV", PEER.parse().unwrap());
    let host = f.host.clone();
    let finishing = {
        let id = id.clone();
        tokio::spawn(async move {
            let (pin, name, _, approver) = entered.await.unwrap();
            host.paired
                .write()
                .unwrap()
                .clients
                .iter_mut()
                .find(|c| c.name == "Other device")
                .unwrap()
                .extra
                .remove("host_control");
            clients(&host)
                .finish_pairing(
                    &host,
                    &id,
                    &pin,
                    &name,
                    approver.as_deref(),
                    PEER.parse().unwrap(),
                )
                .map(drop)
        })
    };
    approve(&f, &approver, &id, "4321")
        .await
        .assert_error(403, "permission_required");
    assert!(finishing.await.unwrap().is_err());
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
    assert!(saved(&f, "Browser TV").is_none());
}

#[tokio::test]
async fn three_wrong_pins_end_the_devices_requests_here_and_the_console_still_can() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    for _ in 0..3 {
        run(&f, &approver, "TV", "1111", "2222")
            .await
            .assert_error(422, "wrong_pin");
        assert!(saved(&f, "TV").is_none());
        f.host.approvals.lock().unwrap().cool_down();
    }
    // The device asks again: not offered, refused, and left for the console.
    let (_, id) = asking(&f, "TV").await;
    assert!(listed(&f, &approver).await.is_empty());
    approve(&f, &approver, &id, "2222")
        .await
        .assert_error(409, "pairing_ended");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
    assert!(saved(&f, "TV").is_none());
}

#[tokio::test]
async fn a_new_name_at_the_same_address_does_not_escape_the_count() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    for name in ["TV", "TV2", "LivingRoom"] {
        run(&f, &approver, name, "1111", "2222")
            .await
            .assert_error(422, "wrong_pin");
        f.host.approvals.lock().unwrap().cool_down();
    }
    let (_, id) = asking(&f, "Console").await;
    assert!(listed(&f, &approver).await.is_empty());
    approve(&f, &approver, &id, "2222")
        .await
        .assert_error(409, "pairing_ended");
}

#[tokio::test]
async fn a_right_pin_clears_the_wrong_ones() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    run(&f, &approver, "TV", "1111", "2222")
        .await
        .assert_error(422, "wrong_pin");
    f.host.approvals.lock().unwrap().cool_down();
    let reply = run(&f, &approver, "TV", "2222", "2222").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn moonlights_unpair_neither_counts_nor_drops_an_approval() {
    let f = Fixture::new();
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let (client, id) = asking(&f, "TV").await;
    let plain = Connection {
        certificate: None,
        tls: false,
        ..f.connection(false)
    };
    let (reply, _) = tokio::join!(approve(&f, &approver, &id, "1111"), async {
        until("the handshake to start", || {
            f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        // Without a certificate it proves nothing, however often it comes.
        for _ in 0..5 {
            let reply = send(
                &f,
                false,
                &plain,
                "GET",
                "/unpair?uniqueid=TV",
                None,
                vec![],
            )
            .await;
            assert!(reply.text().contains("<unpaired>0</unpaired>"));
        }
        assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
        assert!(!f.host.approvals.lock().unwrap().is_idle());
        // The device finishes after it: approved, told so and capped.
        finish_pairing(&f, &client, "TV", "1111").await.unwrap();
    });
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let tv = saved(&f, "TV").unwrap();
    assert_eq!(tv.perm, LATER_DEVICE_PERMISSION);
    assert!(!tv.extra.contains_key("pairing_approve"));
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
}

#[tokio::test]
async fn moonlights_real_wrong_pin_counts_once_when_the_wait_ends() {
    let f = Fixture::new();
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let (_client, id) = asking(&f, "TV").await;
    let plain = Connection {
        certificate: None,
        tls: false,
        ..f.connection(false)
    };
    let (reply, _) = tokio::join!(approve(&f, &approver, &id, "1111"), async {
        until("the handshake to start", || {
            f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        send(
            &f,
            false,
            &plain,
            "GET",
            "/unpair?uniqueid=TV",
            None,
            vec![],
        )
        .await
    });
    reply.assert_error(409, "pairing_unconfirmed");
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 1);
}
