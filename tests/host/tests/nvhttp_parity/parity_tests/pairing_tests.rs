//! NVHTTP OTP, QR/typed PIN pairing, replay refusal, and pending-request bound tests.
use super::*;

fn otp_request(f: &Fixture) -> Args {
    Args::from([
        ("uniqueid".into(), "otp-fixture".into()),
        ("phrase".into(), "getservercert".into()),
        ("salt".into(), hex::encode([0x12; 16])),
        (
            "clientcert".into(),
            hex::encode(f.host.identity.certificate.as_bytes()),
        ),
        ("devicename".into(), "Client name".into()),
        (
            "otpauth".into(),
            hex::encode(crypto::hash(
                format!("1234{}passphrase", hex::encode([0x12; 16])).as_bytes(),
            )),
        ),
    ])
}
fn set_otp(f: &Fixture, age: Duration) {
    f.host.otp.lock().unwrap().typed = Some(OneTimePin {
        pin: "1234".into(),
        passphrase: "passphrase".into(),
        device_name: "Console name".into(),
        created: Instant::now() - age,
        misses: Default::default(),
        permission: None,
    });
}

#[tokio::test]
async fn otp_handshake_persists_the_device_name_and_consumes_the_pin_once() {
    let f = Fixture::new();
    set_otp(&f, Duration::ZERO);
    let args = otp_request(&f);
    let peer = "127.0.0.1".parse().unwrap();
    let start = do_pair(f.host.clone(), &args, peer).await.unwrap();
    assert_eq!(start[0], ("paired".into(), "1".into()));
    assert!(f.host.otp.lock().unwrap().typed.is_none());
    let aes = crypto::pin_key(&[0x12; 16], "1234");
    let challenge = [0x34; 16];
    let step = |name: &str, value: Vec<u8>| {
        Args::from([
            ("uniqueid".into(), "otp-fixture".into()),
            (name.into(), hex::encode(value)),
        ])
    };
    let response = do_pair(
        f.host.clone(),
        &step(
            "clientchallenge",
            crypto::ecb(&aes, &challenge, true).unwrap(),
        ),
        peer,
    )
    .await
    .unwrap();
    let response = crypto::ecb(&aes, &hex::decode(&response[1].1).unwrap(), false).unwrap();
    let secret = [0x56; 16];
    let proof = crypto::hash(&[&response[32..], &f.host.identity.signature, &secret].concat());
    let server = do_pair(
        f.host.clone(),
        &step(
            "serverchallengeresp",
            crypto::ecb(&aes, &proof, true).unwrap(),
        ),
        peer,
    )
    .await
    .unwrap();
    let server = hex::decode(&server[1].1).unwrap();
    assert_eq!(
        &response[..32],
        crypto::hash(
            &[
                challenge.as_slice(),
                &f.host.identity.signature,
                &server[..16]
            ]
            .concat()
        )
    );
    assert!(crypto::verify(&f.host.identity.certificate, &server[..16], &server[16..]).unwrap());
    do_pair(
        f.host.clone(),
        &step(
            "clientpairingsecret",
            [&secret[..], &f.host.identity.sign(&secret)].concat(),
        ),
        peer,
    )
    .await
    .unwrap();
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert_eq!(saved.clients.len(), 1);
    assert_eq!(saved.clients[0].name, "Console name");
    assert_eq!(
        saved.clients[0].perm,
        butterpollo_session::state::FIRST_DEVICE_PERMISSION
    );
    assert!(saved.clients[0].enabled);
    // Pairing never grants Host control, even to the first device.
    assert!(!saved.clients[0].extra.contains_key("host_control"));
    assert!(!saved.clients[0].allows_host_control());
    assert!(f.host.pairings.lock().unwrap().sessions.is_empty());
    assert_eq!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string(),
        butterpollo_stream::state::ONE_TIME_PIN_SPENT
    );
}

#[tokio::test]
async fn otp_expiry_wrong_auth_and_pairing_disable_do_not_create_devices() {
    let f = Fixture::new();
    let mut args = otp_request(&f);
    let peer = "127.0.0.1".parse().unwrap();
    set_otp(&f, Duration::from_secs(181));
    assert!(do_pair(f.host.clone(), &args, peer).await.is_err());
    assert!(f.host.otp.lock().unwrap().typed.is_none());
    set_otp(&f, Duration::ZERO);
    args.insert("otpauth".into(), "wrong".into());
    assert!(do_pair(f.host.clone(), &args, peer).await.is_ok());
    assert!(f.host.otp.lock().unwrap().typed.is_some());
    assert!(f.host.paired.read().unwrap().clients.is_empty());
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "false".into());
    assert_eq!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string(),
        "pairing is disabled"
    );
    assert!(f.host.otp.lock().unwrap().typed.is_some());
}

#[tokio::test]
async fn one_time_pin_refuses_an_address_after_five_wrong_attempts() {
    let f = Fixture::new();
    set_otp(&f, Duration::ZERO);
    let right = otp_request(&f);
    let mut wrong = right.clone();
    wrong.insert("otpauth".into(), "wrong".into());
    let guesser: std::net::IpAddr = "192.0.2.9".parse().unwrap();
    for _ in 0..5 {
        assert!(do_pair(f.host.clone(), &wrong, guesser).await.is_ok());
    }
    // That address can't guess again, even with the right answer...
    let refused = do_pair(f.host.clone(), &right, guesser).await.unwrap_err();
    assert!(refused.to_string().contains("this address"), "{refused}");
    assert!(f.host.paired.read().unwrap().clients.is_empty());
    // ...and has not used up the invitation for the device it was made for.
    let device = "192.0.2.10".parse().unwrap();
    let start = do_pair(f.host.clone(), &right, device).await.unwrap();
    assert_eq!(start[0], ("paired".into(), "1".into()));
    assert!(f.host.otp.lock().unwrap().typed.is_none());
}

#[tokio::test]
async fn one_time_pin_is_spent_after_wrong_attempts_from_many_addresses() {
    use butterpollo_stream::state::OtpMisses;
    let f = Fixture::new();
    set_otp(&f, Duration::ZERO);
    let mut wrong = otp_request(&f);
    wrong.insert("otpauth".into(), hex::encode([0u8; 32]));
    for i in 0..OtpMisses::TOTAL {
        let peer = std::net::IpAddr::from([192, 0, 2, 100 + i]);
        assert!(do_pair(f.host.clone(), &wrong, peer).await.is_ok());
    }
    assert!(f.host.otp.lock().unwrap().typed.is_none());
    assert!(
        do_pair(
            f.host.clone(),
            &otp_request(&f),
            "192.0.2.10".parse().unwrap()
        )
        .await
        .is_err()
    );
    assert!(f.host.paired.read().unwrap().clients.is_empty());
}

fn permission_of(f: &Fixture, name: &str) -> u32 {
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    saved.clients.iter().find(|c| c.name == name).unwrap().perm
}

#[tokio::test]
async fn the_qr_code_and_the_typed_pin_each_pair_once_with_their_own_access() {
    use butterpollo_session::state::{LATER_DEVICE_PERMISSION, PLAY_DEVICE_PERMISSION};
    let f = Fixture::new();
    // The first device, so the next ones get the access chosen for them.
    f.client(butterpollo_session::state::FIRST_DEVICE_PERMISSION);
    let peer = "127.0.0.1".parse().unwrap();
    {
        let mut otp = f.host.otp.lock().unwrap();
        otp.qr = Some(one_time_pin(
            "5678",
            "qr phrase",
            Some(PLAY_DEVICE_PERMISSION),
        ));
        otp.typed = Some(one_time_pin("1234", "typed phrase", None));
    }
    let (scanned, typed) = (
        crypto::Identity::generate().unwrap(),
        crypto::Identity::generate().unwrap(),
    );

    let request = device_request(&scanned, "scanned", Some(("5678", "qr phrase")));
    do_pair(f.host.clone(), &request, peer).await.unwrap();
    // Consuming the QR code's PIN keeps the typed one.
    assert!(f.host.otp.lock().unwrap().qr.is_none());
    assert!(f.host.otp.lock().unwrap().typed.is_some());
    finish_pairing(&f, &scanned, "scanned", "5678")
        .await
        .unwrap();
    assert_eq!(permission_of(&f, "scanned"), PLAY_DEVICE_PERMISSION);

    let request = device_request(&typed, "typed", Some(("1234", "typed phrase")));
    do_pair(f.host.clone(), &request, peer).await.unwrap();
    finish_pairing(&f, &typed, "typed", "1234").await.unwrap();
    // No access chosen: view only, as before.
    assert_eq!(permission_of(&f, "typed"), LATER_DEVICE_PERMISSION);
    // Play games also joins co-op sessions; view only does not.
    let joins = |name: &str| {
        let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
        saved
            .clients
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .allows_coop_join()
    };
    assert!(joins("scanned"));
    assert!(!joins("typed"));

    // Each worked once.
    let again = device_request(&scanned, "scanned", Some(("5678", "qr phrase")));
    let error = do_pair(f.host.clone(), &again, peer).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        butterpollo_stream::state::ONE_TIME_PIN_SPENT
    );
}

/// What a client reads from `/pair` over plain HTTP, as Moonlight asks for the
/// server certificate: the HTTP status and the XML body.
async fn pair_over_http(f: &Fixture, args: &Args) -> (StatusCode, String) {
    let query = args
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let plain = Connection {
        peer: "127.0.0.1:50000".parse().unwrap(),
        local: "127.0.0.1:47989".parse().unwrap(),
        certificate: None,
        tls: false,
        second_screen: false,
    };
    let request = Request::builder()
        .uri(format!("/pair?{query}"))
        .body(Body::empty())
        .unwrap();
    let response = router(f.host.clone(), false)
        .layer(Extension(plain))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_replayed_pairing_link_with_no_live_code_is_refused_with_400() {
    let f = Fixture::new();
    let scanned = crypto::Identity::generate().unwrap();
    let link = device_request(&scanned, "scanned", Some(("5678", "qr phrase")));
    let refused = |body: &str| {
        // A refusal in the XML status, as every other pairing refusal: Nightjar
        // shows "show a new code" for a 4xx and network advice for anything else.
        body.contains("status_code=\"400\"")
            && body.contains("<paired>0</paired>")
            && body.contains("show a new one on the PC")
    };
    // No code was ever shown.
    let (status, body) = pair_over_http(&f, &link).await;
    assert_eq!(status, StatusCode::OK);
    assert!(refused(&body), "{body}");

    // The link pairs once...
    f.host.otp.lock().unwrap().qr = Some(one_time_pin("5678", "qr phrase", None));
    let (status, body) = pair_over_http(&f, &link).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<paired>1</paired>"), "{body}");
    finish_pairing(&f, &scanned, "scanned", "5678")
        .await
        .unwrap();
    // ...and replaying it, with no code live, is refused rather than dropped.
    let (status, body) = pair_over_http(&f, &link).await;
    assert_eq!(status, StatusCode::OK);
    assert!(refused(&body), "{body}");

    // So is a link whose code expired.
    let mut expired = one_time_pin("5678", "qr phrase", None);
    expired.created -= butterpollo_stream::state::ONE_TIME_PIN_LIFETIME;
    f.host.otp.lock().unwrap().qr = Some(expired);
    let (status, body) = pair_over_http(&f, &link).await;
    assert_eq!(status, StatusCode::OK);
    assert!(refused(&body), "{body}");
    assert!(f.host.otp.lock().unwrap().qr.is_none());
}

#[tokio::test]
async fn a_wrong_one_time_pin_counts_against_each_live_slot_and_expiry_closes_one() {
    let f = Fixture::new();
    let peer = "127.0.0.1".parse().unwrap();
    let client = crypto::Identity::generate().unwrap();
    {
        let mut otp = f.host.otp.lock().unwrap();
        otp.qr = Some(one_time_pin("5678", "qr phrase", None));
        otp.typed = Some(one_time_pin("1234", "typed phrase", None));
        otp.qr.as_mut().unwrap().created -= Duration::from_secs(181);
    }
    // The expired QR code's PIN no longer matches; the typed one counts a miss.
    let expired = device_request(&client, "device", Some(("5678", "qr phrase")));
    do_pair(f.host.clone(), &expired, peer).await.unwrap();
    assert!(f.host.otp.lock().unwrap().qr.is_none());
    assert_eq!(
        f.host
            .otp
            .lock()
            .unwrap()
            .typed
            .as_ref()
            .unwrap()
            .misses
            .count(peer),
        1
    );
    assert!(finish_pairing(&f, &client, "device", "5678").await.is_err());

    f.host.otp.lock().unwrap().qr = Some(one_time_pin("5678", "qr phrase", None));
    let wrong = device_request(&client, "device", Some(("0000", "guess")));
    for _ in 0..4 {
        do_pair(f.host.clone(), &wrong, peer).await.unwrap();
    }
    // The typed PIN's fifth miss refuses this address; the QR code has four.
    {
        let otp = f.host.otp.lock().unwrap();
        assert!(otp.typed.as_ref().unwrap().misses.refuses(peer));
        assert_eq!(otp.qr.as_ref().unwrap().misses.count(peer), 4);
    }
    do_pair(f.host.clone(), &wrong, peer).await.unwrap();
    assert!(
        f.host
            .otp
            .lock()
            .unwrap()
            .qr
            .as_ref()
            .unwrap()
            .misses
            .refuses(peer)
    );
    let refused = do_pair(f.host.clone(), &wrong, peer).await.unwrap_err();
    assert!(refused.to_string().contains("this address"), "{refused}");
    assert!(f.host.paired.read().unwrap().clients.is_empty());
}

#[tokio::test]
async fn the_access_typed_with_a_console_pin_is_applied_and_the_first_device_gets_everything() {
    use butterpollo_session::state::{
        FIRST_DEVICE_PERMISSION, LATER_DEVICE_PERMISSION, PLAY_DEVICE_PERMISSION,
    };
    let f = Fixture::new();
    let peer = "127.0.0.1".parse().unwrap();
    for (name, access, granted) in [
        // The first device gets everything whatever was chosen.
        (
            "first",
            Some(LATER_DEVICE_PERMISSION),
            FIRST_DEVICE_PERMISSION,
        ),
        (
            "second",
            Some(PLAY_DEVICE_PERMISSION),
            PLAY_DEVICE_PERMISSION,
        ),
        ("third", None, LATER_DEVICE_PERMISSION),
    ] {
        let client = crypto::Identity::generate().unwrap();
        let request = device_request(&client, name, None);
        let host = f.host.clone();
        let task = tokio::spawn(async move { do_pair(host, &request, peer).await });
        let key = pairing_id(name, peer);
        let pending = loop {
            if let Some(pending) = f.host.pins.lock().unwrap().remove(&key) {
                break pending;
            }
            tokio::task::yield_now().await;
        };
        pending
            .sender
            .send(("4321".into(), name.into(), access, None))
            .unwrap();
        task.await.unwrap().unwrap();
        finish_pairing(&f, &client, name, "4321").await.unwrap();
        assert_eq!(permission_of(&f, name), granted, "{name}");
    }
}

#[tokio::test]
async fn pairing_ids_are_scoped_to_the_peer_and_pending_requests_are_bounded() {
    let f = Fixture::new();
    let mut args = otp_request(&f);
    args.remove("otpauth");
    let peer = "192.0.2.1".parse().unwrap();
    let other = "192.0.2.2".parse().unwrap();
    let id = pairing_id(&args["uniqueid"], peer);
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    f.host.pins.lock().unwrap().insert(
        id,
        PendingPin {
            name: "Existing".into(),
            certificate: "different certificate".into(),
            address: peer,
            created: Instant::now(),
            sender,
        },
    );
    assert!(do_pair(f.host.clone(), &args, peer).await.is_err());
    let host = f.host.clone();
    let request = args.clone();
    let task = tokio::spawn(async move { do_pair(host, &request, other).await });
    tokio::task::yield_now().await;
    let key = pairing_id(&args["uniqueid"], other);
    assert!(f.host.pins.lock().unwrap().contains_key(&key));
    task.abort();
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    f.host.pins.lock().unwrap().insert(
        pairing_id("second", peer),
        PendingPin {
            name: "Second".into(),
            certificate: "different certificate".into(),
            address: peer,
            created: Instant::now(),
            sender,
        },
    );
    args.insert("uniqueid".into(), "third".into());
    assert!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit reached")
    );
}

#[tokio::test]
async fn a_device_asking_again_replaces_its_own_waiting_pin_request() {
    let f = Fixture::new();
    let mut args = otp_request(&f);
    args.remove("otpauth");
    let peer = "192.0.2.1".parse().unwrap();
    let id = pairing_id(&args["uniqueid"], peer);
    let (sender, mut old) = tokio::sync::oneshot::channel();
    f.host.pins.lock().unwrap().insert(
        id.clone(),
        PendingPin {
            name: "Earlier".into(),
            certificate: f.host.identity.certificate.clone(),
            address: peer,
            created: Instant::now(),
            sender,
        },
    );
    let host = f.host.clone();
    let request = args.clone();
    let task = tokio::spawn(async move { do_pair(host, &request, peer).await });
    tokio::task::yield_now().await;
    assert!(!task.is_finished(), "the same device must not be refused");
    assert_eq!(f.host.pins.lock().unwrap()[&id].name, "Client name");
    assert!(matches!(
        old.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    ));
    task.abort();
}
