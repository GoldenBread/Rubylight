//! Console API token permissions, metadata, device grants, host control, and sleep parity tests.
use super::*;

#[tokio::test]
async fn all_offered_token_permissions_can_be_created_persisted_used_and_revoked() {
    let f = Fixture::new();
    let offered = value(
        request(&f, "GET", "/api/token/routes", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let scopes = offered["routes"].clone();
    assert!(scopes.as_array().unwrap().len() > 64);
    let mut canonical: Vec<auth::Scope> = serde_json::from_value(scopes.clone()).unwrap();
    for scope in &mut canonical {
        scope.methods.sort();
    }
    let expected = serde_json::to_value(&canonical).unwrap();
    let issued = value(
        request(&f, "POST", "/api/token", json!({"scopes":scopes})).await,
        StatusCode::OK,
    )
    .await;
    let secret = issued["token"].as_str().unwrap();
    let saved = std::fs::read_to_string(&f.host.aliases_path).unwrap();
    assert!(!saved.contains(secret));
    let document: Value = serde_json::from_str(&saved).unwrap();
    let tokens = auth::read(&document).unwrap();
    assert_eq!(serde_json::to_value(&tokens[0].scopes).unwrap(), expected);
    // Exercise the saved representation as a newly loaded host would.
    *f.host.aliases.lock().unwrap() = document;
    let listed = value(
        request(&f, "GET", "/api/tokens", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["tokens"][0]["scopes"], expected);
    assert!(!listed.to_string().contains(secret));
    let hash = listed["tokens"][0]["hash"].as_str().unwrap();
    for scope in &tokens[0].scopes {
        let path = scope
            .path
            .replace("[^/]+", "example")
            .replace("[0-9]+", "123");
        for method in &scope.methods {
            assert!(auth::permits(&tokens, secret, "parity", &path, method));
        }
    }
    // An all-permissions token still cannot mint/list tokens or broaden a
    // listed route's methods. Only the administrator can manage API tokens.
    for (method, path, status) in [
        ("GET", "/api/apps", StatusCode::OK),
        ("GET", "/api/tokens", StatusCode::UNAUTHORIZED),
        ("POST", "/api/token", StatusCode::UNAUTHORIZED),
        ("PATCH", "/api/apps", StatusCode::UNAUTHORIZED),
    ] {
        let response = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
    }
    value(
        request(&f, "DELETE", &format!("/api/token/{hash}"), Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let revoked = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .uri("/api/apps")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    let saved = state::load_json(&f.host.aliases_path, Value::Null).unwrap();
    assert!(auth::read(&saved).unwrap().is_empty());
}

#[tokio::test]
async fn both_metadata_routes_require_authentication_and_preserve_gpu_inventory() {
    let f = Fixture::new();
    let expected = json!({
        "status": true,
        "gpus": [{"name": "AMD Radeon RX 7600M XT", "vendor": 4098,
            "dedicated_memory": 8589934592u64, "luid": [2, 0], "pnp_id": null}],
        "gpu_error": null,
        "capture_status": {"displays": [], "error": null}
    });
    // Warm the auth verification cache before stamping the 5s metadata cache.
    let _ = request(&f, "GET", "/api/config", Value::Null).await;
    for path in ["/api/metadata", "/api/meta"] {
        // Populate the cache without opening GPU devices or probing encoders.
        *f.host.metadata.lock().unwrap() = Some((Instant::now(), expected.clone()));
        let actual = value(request(&f, "GET", path, Value::Null).await, StatusCode::OK).await;
        assert_eq!(actual, expected);
        let anonymous = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let denied = value(anonymous, StatusCode::UNAUTHORIZED).await;
        assert!(denied.get("gpus").is_none());
    }
}

#[tokio::test]
async fn device_updates_persist_permissions_overrides_and_revoke_only_that_devices_sessions() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let mine = f.session(client.clone(), Role::Stream);
    let mut other = client.clone();
    other.uuid = "other".into();
    let theirs = f.session(other, Role::InputOnly);
    let body = json!({"uuid":client.uuid,"name":"Renamed","cert":"cannot replace the certificate","perm":u32::MAX,
        "config_overrides":{"max_bitrate":20000},"display_mode":"2560x1600x120","custom":"preserved"});
    let updated = value(
        request(&f, "POST", "/api/clients/update", body).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["disconnected"], false);
    assert!(!mine.stopping());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"], listed["named_certs"]);
    let device = &listed["clients"][0];
    assert_eq!(device["perm"], 0x071f1f00);
    assert_eq!(device["name"], "Renamed");
    assert_eq!(device["connected"], true);
    assert!(device.get("cert").is_none());
    assert_eq!(device["config_overrides"]["max_bitrate"], 20000);
    for body in [
        json!({"uuid":client.uuid,"perm":1<<25}),
        json!({"uuid":client.uuid,"enabled":false,"display_mode":null}),
    ] {
        let updated = value(
            request(&f, "POST", "/api/clients/update", body).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(updated["disconnected"], true);
        assert!(mine.stopping());
        assert!(!theirs.stopping());
    }
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].enabled);
    assert_eq!(saved.clients[0].perm, 1 << 25);
    assert_eq!(saved.clients[0].cert, client.cert);
    assert_eq!(saved.clients[0].extra["custom"], "preserved");
    assert!(!saved.clients[0].extra.contains_key("display_mode"));
    assert_eq!(
        saved.clients[0].extra["config_overrides"]["max_bitrate"],
        20000
    );
}

#[tokio::test]
async fn host_control_is_granted_only_as_a_boolean_and_revoking_keeps_the_stream() {
    let f = Fixture::new();
    // A full permission value must not switch it on.
    let client = f.client(u32::MAX);
    let mine = f.session(client.clone(), Role::Stream);
    assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
    for bad in [
        json!("yes"),
        json!("true"),
        json!(1),
        json!([true]),
        json!({}),
    ] {
        value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"name":"Renamed","host_control":bad}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let paired = f.host.paired.read().unwrap();
        assert!(!paired.clients[0].allows_host_control(), "{bad}");
        assert_eq!(paired.clients[0].name, client.name, "nothing else changed");
    }
    let granted = value(
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"host_control":true}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(granted["disconnected"], false);
    assert!(f.host.paired.read().unwrap().clients[0].allows_host_control());
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients[0].allows_host_control());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"][0]["host_control"], true);
    // Revoking, with false or null, takes effect now and keeps the session.
    for revoke in [json!(false), json!(null)] {
        let granted = value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"host_control":true}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(granted["disconnected"], false);
        let revoked = value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"host_control":revoke}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(revoked["disconnected"], false);
        assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
        assert!(!mine.stopping());
    }
}

#[tokio::test]
async fn sleep_is_granted_only_as_a_boolean() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    assert!(!f.host.paired.read().unwrap().clients[0].allows_sleep());
    for bad in [json!("true"), json!(1), json!([true]), json!({})] {
        value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"name":"Renamed","allow_sleep":bad}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let paired = f.host.paired.read().unwrap();
        assert!(!paired.clients[0].allows_sleep(), "{bad}");
        assert!(!paired.clients[0].extra.contains_key("allow_sleep"));
        assert_eq!(paired.clients[0].name, client.name, "nothing else changed");
    }
    let update = |allow: Value| {
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"allow_sleep":allow}),
        )
    };
    value(update(json!(true)).await, StatusCode::OK).await;
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients[0].allows_sleep());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"][0]["allow_sleep"], true);
    value(update(json!(null)).await, StatusCode::OK).await;
    assert!(!f.host.paired.read().unwrap().clients[0].allows_sleep());
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("allow_sleep"));
}

/// A stream's launch copy of the device keeps what it was granted at launch;
/// Host control and Sleep PC are not read from it. Each request reads the
/// paired state, so turning one off in the console is not a revocation (the
/// stream stays) and the very next request is refused all the same.
#[tokio::test]
async fn host_control_and_sleep_turned_off_in_the_console_stop_working_for_a_live_session() {
    let f = Fixture::new();
    f.client(u32::MAX);
    let update = |data: Value| async {
        let data = data;
        let reply = value(
            request(&f, "POST", "/api/clients/update", data).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(reply["disconnected"], false);
    };
    let uuid = f.host.paired.read().unwrap().clients[0].uuid.clone();
    update(json!({"uuid":uuid,"host_control":true,"allow_sleep":true})).await;
    let at_launch = f.host.paired.read().unwrap().clients[0].clone();
    assert!(at_launch.allows_host_control() && at_launch.allows_sleep());
    let live = f.session(at_launch, Role::Stream);
    let status = |method: &'static str, path: &'static str| {
        let app = butterpollo_stream::nvhttp::router_with(
            f.host.clone(),
            true,
            Some(butterpollo_web::native_api::router()),
        )
        .layer(axum::Extension(f.connection(true)))
        .layer(axum::Extension(
            butterpollo_stream::tls::ConnectionEnd::default(),
        ));
        async move {
            app.oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };
    let close = || status("POST", "/api/native/v1/session/close-game");
    let sleep = || {
        status(
            "POST",
            "/power-actions/v1/sleep?token=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
    };
    // Sleep PC is past the permission check while it is on, whatever else it answers.
    assert_ne!(sleep().await, StatusCode::FORBIDDEN);
    update(json!({"uuid":uuid,"host_control":false})).await;
    assert_eq!(close().await, StatusCode::FORBIDDEN);
    assert_ne!(sleep().await, StatusCode::FORBIDDEN, "Sleep PC is still on");
    update(json!({"uuid":uuid,"allow_sleep":null})).await;
    assert_eq!(sleep().await, StatusCode::FORBIDDEN);
    assert!(!live.stopping(), "turning them off keeps the stream");
    // The refusal came from the permission: with it back, Close game ends the stream.
    update(json!({"uuid":uuid,"host_control":true})).await;
    assert_ne!(close().await, StatusCode::FORBIDDEN);
    assert!(live.stopping());
}

#[tokio::test]
async fn usb_and_folder_grants_are_saved_only_in_their_shape() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let update = |change: Value| {
        let mut body = json!({"uuid":client.uuid,"name":"Renamed"});
        body.as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        request(&f, "POST", "/api/clients/update", body)
    };
    for bad in [
        json!({"allow_usb": "true"}),
        json!({"allow_usb": 1}),
        json!({"shared_folders": ["games"]}),
        json!({"shared_folders": "games"}),
        json!({"shared_folders": {"games": "admin"}}),
        json!({"shared_folders": {"games": true}}),
        json!({"shared_folders": {"": "read"}}),
        json!({"shared_folders": {"../games": "read"}}),
    ] {
        value(update(bad.clone()).await, StatusCode::BAD_REQUEST).await;
        let paired = f.host.paired.read().unwrap();
        let device = &paired.clients[0];
        assert!(!device.allows_usb() && !device.has_folders(), "{bad}");
        assert!(!device.extra.contains_key("allow_usb"), "{bad}");
        assert!(!device.extra.contains_key("shared_folders"), "{bad}");
        assert_eq!(device.name, client.name, "nothing else changed");
    }
    value(
        update(json!({"allow_usb": true, "shared_folders": {"games": "read", "saves-2": "write"}}))
            .await,
        StatusCode::OK,
    )
    .await;
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    let device = &saved.clients[0];
    assert!(device.allows_usb() && device.has_folders());
    use rubylight_native_api::folders::FolderAccess;
    assert_eq!(
        (
            device.folder_access("games"),
            device.folder_access("saves-2")
        ),
        (Some(FolderAccess::Read), Some(FolderAccess::Write))
    );
    value(
        update(json!({"allow_usb": null, "shared_folders": null})).await,
        StatusCode::OK,
    )
    .await;
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("allow_usb"));
    assert!(!saved.clients[0].extra.contains_key("shared_folders"));
}

#[tokio::test]
async fn host_control_cannot_be_granted_without_the_update_route() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let issued = value(
        request(
            &f,
            "POST",
            "/api/token",
            json!({"scopes":[{"path":"/api/apps","methods":["GET"]}]}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let secret = issued["token"].as_str().unwrap();
    let body = json!({"uuid":client.uuid,"host_control":true}).to_string();
    for authorization in [Some(format!("Bearer {secret}")), None] {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/clients/update")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(authorization) = authorization {
            builder = builder.header(header::AUTHORIZATION, authorization);
        }
        let response = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(builder.body(Body::from(body.clone())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
    }
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("host_control"));
}
