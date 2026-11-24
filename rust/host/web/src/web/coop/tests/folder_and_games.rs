//! Nucleus Co-op folder validation, service availability, per-game status, and token isolation tests.
use super::*;

#[tokio::test]
async fn the_folder_stays_while_a_session_runs_from_it() {
    let c = coop();
    let body = value(
        request(
            &c.f,
            "POST",
            "/api/coop/folder",
            json!({"path": r"D:\Nucleus"}),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        body["error"],
        "Nucleus Co-op is running Portal 2: end that session first, then choose the folder"
    );
    assert_eq!(kept(&c.f), "");
    assert!(c.pc.put_back.lock().unwrap().is_empty());
}

#[tokio::test]
async fn another_release_kept_runs_the_earlier_install_with_a_notice_and_is_never_chosen() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    let why = r"The NucleusCoop.exe in D:\Old isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder";
    let notice = format!(
        r"{why}. Until then Rubylight runs the Nucleus Co-op 2.4.2 an earlier Rubylight installed, in C:\Nucleus"
    );
    *pc.instead.lock().unwrap() = Some((
        Unusable {
            problem: Problem::Version,
            message: why.into(),
        },
        notice.clone(),
    ));
    f.host.nucleus.use_pc(pc.clone());
    // Kept by an earlier Rubylight: the earlier install runs, and says why.
    kept_by_hand(&f, r"D:\Old");
    let install = panel(&f).await["install"].clone();
    assert_eq!(install["state"], "configured");
    assert_eq!(install["path"], r"C:\Nucleus");
    assert_eq!(install["chosen"], r"D:\Old");
    assert_eq!(install["notice"], notice);
    // Chosen in the console: refused, as without the earlier install.
    let body = value(
        request(&f, "POST", "/api/coop/folder", json!({"path": r"D:\Other"})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(body["error"], why);
    assert_eq!(kept(&f), r"D:\Old");
}

#[tokio::test]
async fn a_refused_folder_is_never_kept_and_says_why() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    f.host.nucleus.use_pc(pc.clone());
    for (problem, why) in [
        (Problem::Missing, r"D:\Downloads holds no NucleusCoop.exe"),
        (
            Problem::Version,
            r"The NucleusCoop.exe in D:\Downloads isn't Nucleus Co-op 2.4.2's",
        ),
        (
            Problem::Untrusted,
            r"Rubylight doesn't run Nucleus Co-op from D:\Downloads",
        ),
    ] {
        pc.unusable.lock().unwrap().insert(
            r"D:\Downloads".into(),
            Unusable {
                problem,
                message: why.into(),
            },
        );
        let body = value(
            request(
                &f,
                "POST",
                "/api/coop/folder",
                json!({"path": r"D:\Downloads"}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(body["error"], why);
        // The earlier install still runs; nothing was saved.
        assert_eq!(kept(&f), "");
        assert_eq!(panel(&f).await["install"]["state"], "installed");
    }
    value(
        request(&f, "POST", "/api/coop/folder", json!({})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
}

#[tokio::test]
async fn a_kept_folder_nucleus_cant_run_from_shows_and_refuses_open() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    let why = r"The NucleusCoop.exe in D:\Old isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder";
    pc.unusable.lock().unwrap().insert(
        r"D:\Old".into(),
        Unusable {
            problem: Problem::Version,
            message: why.into(),
        },
    );
    f.host.nucleus.use_pc(pc.clone());
    // Set where the console doesn't check it, or replaced since.
    kept_by_hand(&f, r"D:\Old");
    let body = panel(&f).await;
    assert_eq!(body["install"]["state"], "failed");
    assert_eq!(body["install"]["problem"], "version");
    assert_eq!(body["install"]["error"], why);
    assert_eq!(body["install"]["chosen"], r"D:\Old");
    // The earlier install doesn't stand in for it.
    assert_eq!(body["install"]["path"], Value::Null);
    let refused = value(
        request(&f, "POST", "/api/coop/open", json!({})).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(
        refused["error"],
        format!("Nucleus Co-op is not ready: {why}")
    );
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn open_answers_503_when_the_folder_could_not_be_checked() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    let why = r"reading the owner of C:\Nucleus: access denied";
    *pc.installed_error.lock().unwrap() = Some(why.into());
    f.host.nucleus.use_pc(pc.clone());
    let body = value(
        request(&f, "POST", "/api/coop/open", json!({})).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(body["error"], format!("Nucleus Co-op is not ready: {why}"));
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
    let body = panel(&f).await;
    assert_eq!(body["install"]["state"], "failed");
    assert_eq!(body["install"]["problem"], Value::Null);
}

#[tokio::test]
async fn choosing_a_folder_and_open_are_refused_without_the_service() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    let why = "Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user";
    *pc.unavailable.lock().unwrap() = Some(why.into());
    f.host.nucleus.use_pc(pc.clone());
    for (path, body) in [
        ("/api/coop/open", json!({})),
        ("/api/coop/folder", json!({"path": r"D:\Nucleus"})),
    ] {
        let body = value(
            request(&f, "POST", path, body).await,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        assert_eq!(body["error"], why, "{path}");
    }
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(kept(&f), "");
    // With the service it opens.
    *pc.unavailable.lock().unwrap() = None;
    value(
        request(&f, "POST", "/api/coop/open", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["opened"], true);
    // Opening it again is refused; Close closes the one the console opened.
    value(
        request(&f, "POST", "/api/coop/open", json!({})).await,
        StatusCode::CONFLICT,
    )
    .await;
    let body = value(
        request(&f, "POST", "/api/coop/close", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body, json!({"closed": true}));
    assert!(!pc.window().open.load(std::sync::atomic::Ordering::Relaxed));
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["opened"], false);
}

#[tokio::test]
async fn each_game_with_an_exe_shows_its_coop_status() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    f.host.nucleus.use_pc(pc.clone());
    let app = |name: &str, cmd: &str, uuid: &str| App {
        name: name.into(),
        cmd: cmd.into(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
    };
    *f.host.apps.write().unwrap() = vec![
        app("Desktop", "", "desktop"),
        app("Portal 2", &format!("\"{}\"", fake::EXE), "portal"),
        app("Puzzle", r"C:\Games\Puzzle.exe", "puzzle"),
        app(
            "Counter-Strike 2",
            "cmd /c start \"\" steam://rungameid/730",
            "cs2",
        ),
        app(
            "Portal",
            r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 400"#,
            "portal1",
        ),
    ];
    let dir = std::env::temp_dir().join(format!("coop-runs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ice, fort) = (dir.join("Ice.exe"), dir.join("Fort.exe"));
    std::fs::write(&ice, b"").unwrap();
    std::fs::write(&fort, b"").unwrap();
    pc.needs.lock().unwrap().insert(
        fort.to_string_lossy().into_owned(),
        butterpollo_session::nucleus::elevation::Need::Administrator(
            butterpollo_session::nucleus::elevation::Why::Manifest,
        ),
    );
    f.host.apps.write().unwrap().extend([
        app("Ice", &format!("\"{}\"", ice.display()), "ice"),
        app("Fort", &format!("\"{}\"", fort.display()), "fort"),
    ]);
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let no_handler = |name: &str| {
        format!("No Nucleus Co-op handler for {name}.exe: add one in Nucleus Co-op's handler hub")
    };
    assert_eq!(
        body["games"],
        json!([
            {"app": "portal", "name": "Portal 2", "status": "ready", "reason": null,
             "handler": "Portal 2", "players": 2, "runs": null, "entry": "Portal 2 (Co-op)"},
            // Without a handler, as a background launch would answer.
            {"app": "puzzle", "name": "Puzzle", "status": "refused",
             "reason": r"C:\Games\Puzzle.exe is not on this PC: set the app's command to the game's .exe",
             "handler": null, "players": null, "runs": null, "entry": "Puzzle (Co-op)"},
            {"app": "cs2", "name": "Counter-Strike 2", "status": "launcher", "reason": null,
             "handler": null, "players": null, "runs": null, "entry": "Counter-Strike 2 (Co-op)"},
            {"app": "portal1", "name": "Portal", "status": "launcher", "reason": null,
             "handler": null, "players": null, "runs": null, "entry": "Portal (Co-op)"},
            // Rubylight's own handler: as the user, or elevated as the
            // game asks.
            {"app": "ice", "name": "Ice", "status": "no_handler", "reason": no_handler("Ice"),
             "handler": null, "players": null, "runs": "user", "entry": "Ice (Co-op)"},
            {"app": "fort", "name": "Fort", "status": "no_handler", "reason": no_handler("Fort"),
             "handler": null, "players": null, "runs": "elevated", "entry": "Fort (Co-op)"},
        ])
    );
    assert_eq!(body["catalog_error"], Value::Null);
    assert_eq!(body["elevates"], true);
    // On an account Windows doesn't elevate: the hub handler's game says
    // so before a launch, in the launch's words; ours as the user stays.
    *pc.account.lock().unwrap() = Account::Standard;
    f.host.nucleus.use_pc(pc.clone());
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(body["elevates"], false);
    assert_eq!(body["admin_protection"], false);
    // Open Nucleus Co-op on this PC says why before it is pressed.
    assert_eq!(
        body["open_refusal"],
        "Nucleus Co-op needs administrator rights, and Windows doesn't give this account any: sign in to Windows on this PC as an administrator"
    );
    let games = body["games"].as_array().unwrap();
    assert_eq!(
        (&games[0]["status"], &games[0]["reason"]),
        (
            &json!("ready"),
            &json!(format!(
                "Nucleus Co-op needs administrator rights for a hub handler, and {}",
                elevation::STANDARD
            ))
        )
    );
    assert_eq!(
        (&games[4]["status"], &games[4]["reason"], &games[4]["runs"]),
        (
            &json!("no_handler"),
            &json!(no_handler("Ice")),
            &json!("user")
        )
    );
    assert_eq!(games[5]["status"], "refused");
    assert!(
        games[5]["reason"]
            .as_str()
            .unwrap()
            .ends_with(elevation::STANDARD),
        "{}",
        games[5]
    );
    // An administrator under Windows' Administrator protection is told
    // what can't run and what still does, never to turn it off.
    *pc.account.lock().unwrap() = Account::Protected;
    f.host.nucleus.use_pc(pc.clone());
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["elevates"], false);
    assert_eq!(body["admin_protection"], true);
    assert_eq!(
        body["open_refusal"],
        "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't open on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
    assert_eq!(
        body["games"][0]["reason"],
        "Nucleus Co-op needs administrator rights for a hub handler, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
    // Before Nucleus Co-op is set up nothing is known yet.
    *pc.installed.lock().unwrap() = None;
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["games"][0]["status"], "unknown");
    assert_eq!(body["games"][1]["status"], "unknown");
}

#[tokio::test]
async fn a_second_app_of_the_same_name_is_not_ready_as_the_library_lists_one_entry() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    f.host.nucleus.use_pc(pc.clone());
    let app = |uuid: &str| App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
    };
    let apps = vec![app("first"), app("second")];
    *f.host.apps.write().unwrap() = apps.clone();
    assert_eq!(f.host.nucleus.entries(&apps, "").len(), 1);
    let body = value(
        request(&f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["games"][0]["status"], "ready");
    assert_eq!(body["games"][1]["status"], "taken");
    assert_eq!(
        body["games"][1]["reason"],
        "Another app already lists as \"Portal 2 (Co-op)\", so the library can't show this one: rename one of them"
    );
}

#[test]
fn api_tokens_cannot_reach_the_coop_panel() {
    assert!(
        !crate::web::token_catalog()
            .iter()
            .any(|scope| scope.path.contains("coop"))
    );
}
