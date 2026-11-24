//! Link notices, text input notices, keyboard layout reports, and outage delivery tests.
use super::*;

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
