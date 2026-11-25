//! The tray on a session bus of its own, with a stand-in tray and notification server:
//! `dbus-run-session -- cargo test -p butterpollo-linux-io --lib tray::live_tests -- --ignored`.
//! It refuses a bus that has a tray, where the icon would show on the desktop.
use super::{Action, Tray, notify_pairing, pairing_request_ended, show_app};
use butterpollo_session::tray::Icon;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, connection::Builder},
    zvariant::{OwnedValue, Value},
};

type Pixmap = super::pixmap::Pixmap;
type ToolTip = (String, Vec<Pixmap>, String, String);
type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
/// Each notification's id, title and actions.
type Notifications = Arc<Mutex<Vec<(u32, String, Vec<String>)>>>;

#[derive(Default)]
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}
#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String) {
        self.items.lock().unwrap().push(service);
    }
}

#[derive(Default)]
struct Server {
    shown: Notifications,
    closed: Arc<Mutex<Vec<u32>>>,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app: String,
        _replaces: u32,
        _icon: String,
        summary: String,
        _body: String,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _timeout: i32,
    ) -> u32 {
        let mut shown = self.shown.lock().unwrap();
        let id = shown.len() as u32 + 1;
        shown.push((id, summary, actions));
        id
    }
    fn close_notification(&self, id: u32) {
        self.closed.lock().unwrap().push(id);
    }
}

fn until<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = found() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn property<T: TryFrom<OwnedValue>>(bus: &Connection, item: &str, name: &str) -> T
where
    T::Error: std::fmt::Debug,
{
    let reply = bus
        .call_method(
            Some(item),
            "/StatusNotifierItem",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.kde.StatusNotifierItem", name),
        )
        .unwrap();
    T::try_from(reply.body().deserialize::<OwnedValue>().unwrap()).unwrap()
}

fn layout(bus: &Connection, item: &str) -> (u32, Layout) {
    bus.call_method(
        Some(item),
        "/MenuBar",
        Some("com.canonical.dbusmenu"),
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    )
    .unwrap()
    .body()
    .deserialize()
    .unwrap()
}

fn labels(bus: &Connection, item: &str) -> Vec<String> {
    layout(bus, item)
        .1
        .2
        .into_iter()
        .map(|child| {
            let (_, properties, _): Layout = child.try_into().unwrap();
            properties
                .get("label")
                .and_then(|label| String::try_from(label.try_clone().unwrap()).ok())
                .unwrap_or_else(|| "-".into())
        })
        .collect()
}

#[test]
#[ignore = "needs a session bus of its own: dbus-run-session"]
fn the_tray_works_over_its_own_session_bus() {
    let bus = Connection::session().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).unwrap();
    assert!(
        !dbus
            .name_has_owner("org.kde.StatusNotifierWatcher".try_into().unwrap())
            .unwrap(),
        "this bus has a tray: run the test under dbus-run-session"
    );
    let ico = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/package/apollo.ico"
    );
    let (tray, actions) = Tray::new_options(ico.into(), 47990, false).unwrap();
    let item = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());

    // The tray starts after the host, as at sign-in.
    let watcher = Watcher::default();
    let items = watcher.items.clone();
    let server = Server::default();
    let (shown, closed) = (server.shown.clone(), server.closed.clone());
    let desktop = Builder::session()
        .unwrap()
        .name("org.kde.StatusNotifierWatcher")
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at("/StatusNotifierWatcher", watcher)
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", server)
        .unwrap()
        .build()
        .unwrap();
    until("the icon to register", || {
        items.lock().unwrap().contains(&item).then_some(())
    });

    let pixmaps: Vec<Pixmap> = property(&bus, &item, "IconPixmap");
    let sizes: Vec<i32> = pixmaps.iter().map(|(width, _, _)| *width).collect();
    assert_eq!(sizes, [16, 24, 32, 48, 64, 128]);
    assert!(
        pixmaps
            .iter()
            .all(|(w, h, argb)| argb.len() == (w * h * 4) as usize)
    );
    let tip: ToolTip = property(&bus, &item, "ToolTip");
    assert_eq!(
        (tip.2.as_str(), tip.3.as_str()),
        ("Rubylight", "Web port 47990")
    );

    // A click opens the console; the menu's items do what they say.
    bus.call_method(
        Some(item.as_str()),
        "/StatusNotifierItem",
        Some("org.kde.StatusNotifierItem"),
        "Activate",
        &(0i32, 0i32),
    )
    .unwrap();
    let next = |what| actions.recv_timeout(Duration::from_secs(5)).expect(what);
    assert_eq!(next("the click"), Action::Open);
    assert_eq!(labels(&bus, &item)[4], "No app running");
    bus.call_method(
        Some(item.as_str()),
        "/MenuBar",
        Some("com.canonical.dbusmenu"),
        "Event",
        &(4i32, "clicked", Value::from(0i32), 0u32),
    )
    .unwrap();
    assert_eq!(next("the menu's Quit"), Action::Quit);

    // The running app shows in the icon, the tooltip and the menu.
    let revision = layout(&bus, &item).0;
    show_app(Icon::Streaming, Some("Game"), Some("Streaming Game"));
    until("the menu to change", || {
        (layout(&bus, &item).0 > revision).then_some(())
    });
    assert_eq!(labels(&bus, &item)[4], "Quit Game");
    let tip: ToolTip = property(&bus, &item, "ToolTip");
    assert_eq!(tip.3, "Streaming Game");
    let streaming: Vec<Pixmap> = property(&bus, &item, "IconPixmap");
    assert_ne!(streaming, pixmaps);

    // A click on a pairing request opens the PIN; an ended request's notification goes.
    notify_pairing("first", "Pairing request", "A device asks to pair");
    let (id, title, buttons) = until("the notification", || {
        shown.lock().unwrap().first().cloned()
    });
    assert_eq!(title, "Pairing request");
    assert_eq!(buttons, ["default", "Enter PIN"]);
    desktop
        .emit_signal(
            None::<&str>,
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "ActionInvoked",
            &(id, "default"),
        )
        .unwrap();
    assert_eq!(next("the notification's click"), Action::EnterPin);
    notify_pairing("second", "Pairing request", "Another device");
    let (second, _, _) = until("the second notification", || {
        shown.lock().unwrap().get(1).cloned()
    });
    pairing_request_ended("second");
    until("the ended request's notification to go", || {
        closed.lock().unwrap().contains(&second).then_some(())
    });

    drop(tray);
    until("the icon to leave", || {
        (!dbus
            .name_has_owner(item.as_str().try_into().unwrap())
            .unwrap())
        .then_some(())
    });
}
