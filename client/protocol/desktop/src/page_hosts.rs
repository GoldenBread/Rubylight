//! Bridges `nightjar-core::http::HostClient` to the pure UI page traits (`CoopSource`,
//! `FolderSource`, `NeuralSource`, `RemoteProbe`, `session_page::Api`) via a thin local newtype,
//! plus stream overlay `Session and graphics` host helpers.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nightjar_core::http::native_api::capabilities::Capabilities;
use nightjar_core::http::native_api::radeon::{Radeon, SettingsWrite};
use nightjar_core::http::native_api::session::{ActionId, Session};
use nightjar_core::http::{
    ClientIdentity, Coop, CoopJoined, Error, FileError, FileList, FolderEntries, HostCapabilities,
    HostClient, NativeCapabilities, Neural, NeuralWrite, RemoteCheck, SharedFolder, TransferCancel,
    Uploaded,
};
use nightjar_desktop_config::hosts_types::Waker;
use nightjar_desktop_config::strings::tr;
use nightjar_desktop_media::stream;
use nightjar_desktop_ui::coop_page::CoopSource;
use nightjar_desktop_ui::folders_page::FolderSource;
use nightjar_desktop_ui::host_page::{RemoteAccess, RemoteProbe};
use nightjar_desktop_ui::neural_page::NeuralSource;
use nightjar_desktop_ui::session_page::{self, Hints, HostExit};
use nightjar_desktop_ui::ui;
use nightjar_desktop_ui::ui::files::FileHost;

use crate::hosts::{client_for, Host};

/// Local orphan-rule newtype wrapping [`HostClient`] for UI page worker traits.
pub struct HostClientPage(pub Arc<HostClient>);

impl CoopSource for HostClientPage {
    fn coop(&self) -> Result<Coop, Error> {
        self.0.coop()
    }
    fn join(&self, session: u32) -> Result<CoopJoined, Error> {
        self.0.join_coop(session)
    }
    fn cancel(&self) {
        self.0.cancel_pending_requests();
    }
}

impl FolderSource for HostClientPage {
    fn folders(&self) -> Result<Vec<SharedFolder>, FileError> {
        self.0.shared_folders()
    }
    fn entries(&self, id: &str, path: &str) -> Result<FolderEntries, FileError> {
        self.0.folder_entries(id, path)
    }
    fn save(
        &self,
        id: &str,
        path: &str,
        listed: u64,
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<PathBuf, FileError> {
        self.0.save_folder_file(id, path, listed, dir, progress, stop)
    }
    fn upload(
        &self,
        id: &str,
        path: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        self.0.upload_folder_file(id, path, file, length, progress, stop)
    }
    fn cancel(&self) {
        self.0.cancel_pending_requests();
    }
}

impl NeuralSource for HostClientPage {
    fn neural(&self, app: &str) -> Result<Neural, Error> {
        self.0.neural(app)
    }
    fn write(&self, write: &NeuralWrite) -> Result<Neural, Error> {
        self.0.write_neural(write)
    }
    fn cancel(&self) {
        self.0.cancel_pending_requests();
    }
}

impl RemoteProbe for HostClientPage {
    fn check(&self, unique_id: &str) -> Result<RemoteCheck, Error> {
        self.0.remote_check(unique_id)
    }
    fn cancel(&self) {
        self.0.cancel_pending_requests();
    }
}

impl session_page::Api for HostClientPage {
    fn capabilities(&self) -> Result<Capabilities, Error> {
        self.0.native_capabilities()
    }
    fn session(&self) -> Result<Session, Error> {
        HostClient::session(&self.0)
    }
    fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        HostClient::session_action(&self.0, action)
    }
    fn quit_app(&self) -> Result<bool, Error> {
        self.0.cancel()
    }
    fn radeon(&self) -> Result<Radeon, Error> {
        HostClient::radeon(&self.0)
    }
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
        HostClient::write_radeon(&self.0, write)
    }
}

impl FileHost for HostClientPage {
    fn capabilities(&self) -> Result<NativeCapabilities, Error> {
        self.0.native_capabilities_with_files()
    }

    fn list(&self) -> Result<FileList, FileError> {
        self.0.list_files()
    }

    fn upload(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        self.0.upload_file(name, file, length, progress, stop)
    }

    fn download(
        &self,
        name: &str,
        listed: u64,
        dest: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        self.0.download_to_path(name, listed, dest, progress, stop)
    }
}

fn page_client(identity: Option<&Arc<ClientIdentity>>, host: &Host) -> Result<HostClientPage, String> {
    match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address())
            .map(|c| HostClientPage(Arc::new(c))),
        None => Err(tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned()),
    }
}

/// Command::RemoteCheck: checks `host` unless a check is running. One that cannot start says
/// why on the page.
pub fn start_check(
    remote: &mut RemoteAccess,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    if remote.running() {
        return;
    }
    match page_client(identity, host) {
        Ok(client) => remote.start(
            host.id,
            Arc::new(client),
            host.record.unique_id.clone(),
            waker,
        ),
        Err(error) => remote.fail(host.id, error),
    }
}

/// Command::SharedFolders: opens the shared-folders page for `host`, reporting why if it cannot.
pub fn start_folders(
    folders: &mut nightjar_desktop_ui::folders_page::SharedFolders,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    match page_client(identity, host) {
        Ok(client) => folders.open(host.id, Arc::new(client), waker),
        Err(error) => folders.fail(host.id, error),
    }
}

/// Command::Coop: opens the co-op sessions page for `host`, reporting why if it cannot.
pub fn start_coop(
    coop: &mut nightjar_desktop_ui::coop_page::CoopSessions,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    match page_client(identity, host) {
        Ok(client) => coop.open(host.id, Arc::new(client), waker),
        Err(error) => coop.fail(host.id, error),
    }
}

/// Command::Neural: opens the Neural settings panel for `(host, game, app)`.
pub fn start_neural(
    neural: &mut nightjar_desktop_ui::neural_page::NeuralPanel,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    game: u32,
    app: String,
    waker: Waker,
) {
    match page_client(identity, host) {
        Ok(client) => neural.open(host.id, game, app, Arc::new(client), waker),
        Err(error) => neural.fail(host.id, game, error),
    }
}

/// Command::OpenFiles: opens the Files page for `host` unless another host is still transferring.
pub fn open_files(
    launcher: &mut ui::Launcher,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    if launcher.files.page.as_ref().is_some_and(|p| p.host == host.id) {
        launcher.open(ui::Screen::Files(host.id));
        return;
    }
    if launcher.files.transferring() {
        launcher.message(
            tr("Files could not open"),
            tr("Files are still transferring with another PC. Wait for them, or cancel them on that PC’s Files page, then try again."),
        );
        return;
    }
    match page_client(identity, host) {
        Ok(client) => {
            let base = host
                .info
                .as_ref()
                .map_or(HostCapabilities::UNKNOWN, |i| i.capabilities.clone());
            launcher.files.page = Some(ui::files::Page::new(
                host.id,
                Arc::new(client),
                base,
                ui::files::downloads_folder(),
                waker,
            ));
            launcher.open(ui::Screen::Files(host.id));
        }
        Err(e) => launcher.message(tr("Files could not open"), e),
    }
}

/// Opens the in-stream `Session and graphics` page for a running [`stream::Streaming`].
pub fn open_session_page(
    streaming: &stream::Streaming,
    waker: Waker,
    pending_exit: Option<HostExit>,
) -> session_page::Page {
    let identity = streaming.plan.identity.clone();
    let record = streaming.plan.record.clone();
    let address = record.address.clone();
    let page = session_page::Page::open(
        move || {
            client_for(&identity, &record, &address)
                .map(|c| HostClientPage(Arc::new(c)))
        },
        waker,
    );
    if let Some(exit) = pending_exit {
        page.exit(exit);
    }
    page
}

/// The ways out beside Disconnect that end the PC's session or game.
pub fn game_exits(streaming: &stream::Streaming, hints: Hints) -> Vec<HostExit> {
    if streaming.facts.role != nightjar_core::http::AppRole::Stream {
        return Vec::new();
    }
    if !streaming.session_page_available() {
        return vec![HostExit::QuitApp];
    }
    session_page::actions(hints.launch, hints.host_control, None)
        .into_iter()
        .map(|entry| entry.exit)
        .collect()
}

/// The connection facts shown under the Session page's actions.
pub fn connection_lines(
    streaming: &stream::Streaming,
    session_link: &str,
) -> Vec<(&'static str, String)> {
    let network = streaming
        .overlay
        .lines
        .iter()
        .find(|(k, _)| k == "Network")
        .map(|(_, v)| v.as_str());
    session_page::connection_lines(
        &streaming.facts.host_name,
        stream::codec_name(streaming.facts.format),
        (streaming.facts.width, streaming.facts.height, streaming.facts.fps),
        streaming.facts.bitrate_kbps,
        session_link,
        network,
    )
}

/// Closes Nightjar's menu first, then asks the host to open its own menu on a worker thread.
pub fn open_host_menu(
    streaming: &mut stream::Streaming,
    window: *mut sdl3_sys::everything::SDL_Window,
    pads: &mut nightjar_desktop_media::gamepad::Gamepads,
) {
    if streaming.menu_visible {
        streaming.toggle_menu(window, pads);
    }
    if streaming.menu_visible {
        return;
    }
    let Some(client) = streaming.host_client() else {
        streaming.show_toast(tr("Cannot reach the PC."));
        return;
    };
    let shared = streaming.shared.clone();
    std::thread::spawn(move || {
        shared.notify(stream::pipeline::Notice::HostMenu(
            session_page::host_menu_result(client.couch_menu(true)),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::NetworkPath;
    use nightjar_desktop_config::config::HostRecord;
    use nightjar_desktop_ui::host_page::CheckState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    use crate::hosts::{Hosts, Reach};

    struct FakeProbe {
        answer: Mutex<Option<Result<RemoteCheck, Error>>>,
        gate: (Mutex<bool>, Condvar),
        cancels: AtomicUsize,
    }

    impl FakeProbe {
        fn answering(answer: Result<RemoteCheck, Error>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(Some(answer)),
                gate: (Mutex::new(false), Condvar::new()),
                cancels: AtomicUsize::new(0),
            })
        }
        fn release(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }

    impl RemoteProbe for FakeProbe {
        fn check(&self, _: &str) -> Result<RemoteCheck, Error> {
            let mut go = self.gate.0.lock().unwrap();
            while !*go {
                go = self.gate.1.wait(go).unwrap();
            }
            self.answer.lock().unwrap().take().unwrap()
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.release();
        }
    }

    fn host(address: &str) -> Host {
        let record = HostRecord {
            unique_id: "uid".into(),
            name: "PC".into(),
            address: address.into(),
            ..Default::default()
        };
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        let mut host = hosts.list.remove(0);
        host.reach = Reach::Online;
        host
    }

    fn until(label: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {label}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_check_command_runs_one_check_and_says_why_one_cannot_start() {
        let pc = host("192.0.2.7");
        let mut remote = RemoteAccess::default();
        let info = nightjar_core::http::ServerInfo::parse(
            "<root status_code=\"200\"><uniqueid>uid</uniqueid><PairStatus>1</PairStatus><state>SUNSHINE_SERVER_FREE</state></root>",
            47989,
        )
        .unwrap();
        let probe = FakeProbe::answering(Ok(RemoteCheck {
            outcome: nightjar_core::http::RemoteOutcome::Verified {
                server_info: Box::new(info),
                path: Ok(NetworkPath::Direct),
            },
            stats: Default::default(),
        }));
        remote.start(pc.id, probe.clone(), "uid".into(), Arc::new(|| {}));
        start_check(&mut remote, None, &pc, Arc::new(|| {}));
        assert!(remote.running() && remote.state_for(pc.id) == Some(&CheckState::Running));
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 0);
        probe.release();
        until("remote check", || {
            remote.poll(Some(pc.id));
            matches!(remote.state_for(pc.id), Some(CheckState::Done(_)))
        });

        start_check(&mut remote, None, &pc, Arc::new(|| {}));
        let Some(CheckState::Done(r)) = remote.state_for(pc.id) else {
            panic!("{:?}", remote.state);
        };
        assert_eq!(
            r.error.as_deref(),
            Some("Nightjar has no client identity yet. Restart Nightjar and try again.")
        );

        let identity = Arc::new(ClientIdentity::generate().expect("an identity"));
        let mut broken = host("192.0.2.7");
        broken.record.server_certificate = Some("not a certificate".into());
        let Err(why) = client_for(&identity, &broken.record, &broken.record.address) else {
            panic!("the client was made");
        };
        start_check(&mut remote, Some(&identity), &broken, Arc::new(|| {}));
        let Some(CheckState::Done(r)) = remote.state_for(broken.id) else {
            panic!("{:?}", remote.state);
        };
        assert_eq!(r.error.as_deref(), Some(why.as_str()));
    }

    #[test]
    #[ignore = "needs a paired Rubylight host; see the doc comment"]
    fn live_host_answers_the_page() {
        let Some(dir) = nightjar_desktop_media::sdl::test_input(
            "NIGHTJAR_LIVE_CONFIG",
            "the paired host's config directory",
        ) else {
            return;
        };
        let Some(address) =
            nightjar_desktop_media::sdl::test_input("NIGHTJAR_LIVE_ADDRESS", "the host's address")
        else {
            return;
        };
        let (dir, address) = (
            dir.to_string_lossy().into_owned(),
            address.to_string_lossy().into_owned(),
        );
        let saved = nightjar_core::hosts::IdentityStore::open(&dir)
            .identity()
            .expect("a readable store")
            .expect("a saved identity");
        let identity = Arc::new(
            ClientIdentity::from_pem(
                saved.unique_id,
                &saved.certificate_pem,
                &saved.private_key_pem,
            )
            .unwrap(),
        );
        let record: nightjar_desktop_config::config::HostRecord =
            nightjar_core::hosts::HostStore::open(&dir)
                .load_records(nightjar_desktop_config::config::keep_readable_settings_value)
                .into_iter()
                .find(|h: &nightjar_desktop_config::config::HostRecord| {
                    h.address == address && h.server_certificate.is_some()
                })
                .expect("a paired host at that address");
        let page = session_page::Page::open(
            move || client_for(&identity, &record, &address).map(|c| HostClientPage(Arc::new(c))),
            Arc::new(|| {}),
        );
        until("the first answers", || {
            let s = page.state();
            s.session.is_some()
                && (s.radeon.panel.is_some() || s.radeon.hidden || s.radeon.load_failure.is_some())
        });
        let state = page.state();
        println!("{state:#?}");
        println!(
            "actions: {:?}",
            state.actions(Hints { launch: true, host_control: false })
        );
    }
}
