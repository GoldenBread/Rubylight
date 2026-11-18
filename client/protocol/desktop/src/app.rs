//! What the app does: the methods of `App`, whose state and main loop are in main.rs.
//!
//! Here: saving, shutting down, pairing links and adding a PC. The rest by area: the launcher's events
//! and frames (`launcher`), what a page asks for (`command`), the window's renderer (`renderer`), starting
//! a stream (`launch`), a running stream (`session`) and what is drawn over the picture (`overlay`).

use super::*;

mod command;
mod launch;
mod launch_probe;
mod launcher;
mod overlay;
mod renderer;
mod session;

pub(crate) use launch::*;
pub(crate) use launch_probe::*;
pub(crate) use launcher::*;
pub(crate) use overlay::*;
pub(crate) use renderer::*;
pub(crate) use session::*;

/// The name of app `app` on `host`: its entry in the PC's list, else its game in the PC's library.
fn app_name_of(host: &hosts::Host, app: i32) -> Option<String> {
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(entry) = apps.iter().find(|a| a.id == app) {
            return Some(entry.name.clone());
        }
    }
    host.library()?.games.iter().find(|g| g.app_id as i32 == app).map(|g| g.title.clone())
}

impl App {
    fn save_edits(&mut self) {
        // Closing Nightjar is leaving it: a start within the grace time stays unlocked.
        if let Some(pin) = self.launcher.pin.as_mut() {
            pin.left();
            pin.flush();
        }
        if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
    }

    fn save_hosts_and_place(&mut self) {
        self.save_hosts();
        // Under gamescope, or straight to the display, the window was never placed from the saved
        // place, so the desktop's stays.
        if handheld::start_window(handheld::fills_screen()).restore_place {
            if let Some(placement) = window_place::current(self.window, self.store.window_placement().as_ref()) {
                if let Err(e) = self.store.save_window_placement(&placement) { eprintln!("nightjar: window place not saved: {e}"); }
            }
        }
    }

    /// Before an update's hand-over, with nothing streaming: the next Nightjar can read these files
    /// as soon as it starts (a new AppImage, or the old program after a setup that stopped), and
    /// would overwrite with what it read what shutdown writes only later.
    pub(crate) fn save_for_next_run(&mut self) {
        self.save_edits();
        self.save_hosts_and_place();
    }

    pub(crate) fn shutdown(&mut self) {
        self.save_edits();
        self.network_test = None;
        // Its decoders may run on the renderer's device, which goes with the process.
        self.launcher.latency_bench.finish();
        // Its input closes now; the process going closes it too.
        if let Some(companion) = self.second_screen.take() {
            companion.end();
            self.second_screen_gone();
        }
        // The PC's automatic bitrate goes off after the window has gone, not with the process.
        self.gui_back_from_overlay();
        let mut abr_stops: Vec<_> = self.abr_stops.drain(..).map(|(_, stop)| stop).collect();
        let (stop, teardown) = match self.streaming.take() {
            Some((streaming, _)) => streaming.finish(self.window, &mut self.pads),
            None => (None, stream::Teardown::none()),
        };
        abr_stops.extend(stop);
        self.push_teardown(teardown);
        // Sessions an attempt set aside to stop (a recovery that gave up, a previous stop that
        // timed out) are the app's to join too.
        for ended in self.attempts.drain_teardowns() {
            self.push_teardown(ended);
        }
        self.save_hosts_and_place();
        self.pads.close_all();
        self.gui.destroy();
        let teardowns_pending = self.teardowns_busy();
        if self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty() || teardowns_pending {
            // Ending a launch or its decoder checks can take seconds: the window goes now rather
            // than sitting frozen meanwhile.
            // SAFETY: the window run() created, still alive.
            unsafe { SDL_HideWindow(self.window) };
        }
        // The connect thread and the decoder checks may be using the renderer's GPU device (a
        // decoder being made on it), so every launch in progress or ending ends first, and a
        // session one started stops, before the renderer goes.
        let wait = stream::LEAVE_WAIT;
        let deadline = Instant::now() + wait;
        let mut checks_stopped = self.probe.take().is_none_or(|probe| probe.finish_until(deadline));
        for probe in std::mem::take(&mut self.ending_probes) {
            checks_stopped &= probe.finish_until(deadline);
        }
        let (renderer, window) = (self.renderer, self.window);
        if !checks_stopped {
            eprintln!("nightjar: a decoder check did not end within {} ms of leaving; the window and its GPU device stay until the process exits, as it may still be using them", wait.as_millis());
        }
        // A session stopping on its own thread joins the decoder and audio threads that used the
        // renderer: they end before it goes, within the same leaving deadline.
        let teardowns_joined = std::mem::take(&mut self.teardowns)
            .into_iter()
            .all(|mut teardown| teardown.join_until(deadline));
        if !teardowns_joined {
            eprintln!("nightjar: a stream's teardown did not end within {} ms of leaving; the window and its GPU device stay until the process exits, as it may still be using them", wait.as_millis());
        }
        // The launches are cancelled and waited for either way; the renderer goes only when no
        // decoder check is still inside a call that may use it.
        stream::end_all_before(self.attempts.take_all(), deadline.saturating_duration_since(Instant::now()), || {
            if checks_stopped && teardowns_joined {
                // SAFETY: tearing down what run() created, with nothing else using it any more.
                unsafe {
                    crate::sdl::destroy_renderer(renderer);
                    SDL_DestroyWindow(window);
                    SDL_Quit();
                }
            }
        });
        for stop in abr_stops { stop.wait(); }
    }

    /// Drops the saved library of a PC that is unpaired or removed.
    pub(crate) fn forget_library(&self, host: u64) {
        if let Some(unique_id) = self.hosts.get(host).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
            self.store.forget_library(&unique_id);
        }
    }

    pub(crate) fn save_hosts(&mut self) {
        if save_hosts(&self.store, &self.hosts).is_err() {
            self.save_failed();
        }
    }

    /// Says once a run that a save failed, with the folder Nightjar could not write to: without
    /// it a read-only config folder drops every change at the next start without a word. The
    /// change itself stays for this run. The log keeps every failure.
    pub(crate) fn save_failed(&mut self) {
        if !first_save_failure(&mut self.save_failed_told) {
            return;
        }
        self.launcher.notify(tr("Changes could not be saved"), save_failed_text(self.store.dir()));
    }

    /// Keeps a session's teardown for the joins before a renderer switch or the exit, dropping the
    /// ones that already ended instead of letting the list grow with every stream.
    pub(crate) fn push_teardown(&mut self, teardown: stream::Teardown) {
        self.teardowns.retain(|kept| !kept.finished());
        self.teardowns.push(teardown);
    }

    /// The sessions still stopping, for a launch to wait on: its connect thread joins them before
    /// its first request, as a reconnect joins the session it replaces, and hands back the ones
    /// still running after its wait (`Attempts::drain_teardowns`). While it holds them the attempt
    /// itself is what keeps a renderer switch and the exit waiting.
    pub(crate) fn take_stopping(&mut self) -> Vec<stream::Teardown> {
        self.teardowns.retain(|kept| !kept.finished());
        std::mem::take(&mut self.teardowns)
    }

    /// Whether a session is still stopping on the renderer's GPU device: finished teardowns are
    /// dropped, and the rest block a renderer switch or the exit as a start that has ended does.
    pub(crate) fn teardowns_busy(&mut self) -> bool {
        stream::prune_teardowns(&mut self.teardowns)
    }

    /// Waits up to `wait` for the sessions still stopping to end. False says one is still using the
    /// renderer's device; its handle is kept for the next wait or the exit, never dropped.
    #[cfg(unix)]
    pub(crate) fn join_teardowns(&mut self, wait: std::time::Duration) -> bool {
        let deadline = Instant::now() + wait;
        let mut still = Vec::new();
        let mut stopped = true;
        for mut teardown in std::mem::take(&mut self.teardowns) {
            if !teardown.join_until(deadline) {
                stopped = false;
                still.push(teardown);
            }
        }
        self.teardowns = still;
        stopped
    }

    /// Adds the PC a pairing link names and pairs with its one-time PIN and certificate.
    pub(crate) fn pair_link(&mut self, link: rubylight_protocol::pairing_uri::PairingUri) {
        let id = self.hosts.add_manual(&link.host, Some(link.port));
        self.save_hosts();
        self.launcher.dialog = Dialog::Pairing { host: id, pin: String::new() };
        self.hosts.pair_link(id, link);
    }

    /// Opens an invite to play over the internet: the PC is looked for, paired as a guest and
    /// added; the answer closes the progress dialog.
    pub(crate) fn redeem_invite(&mut self, link: rubylight_protocol::invite_uri::InviteUri) {
        let name = link.shown_name();
        let name = if name.is_empty() {
            tr("the PC").to_owned()
        } else {
            name
        };
        self.launcher.dialog = Dialog::Inviting { name };
        self.hosts.redeem_invite(link);
    }

    /// Shows a link the system opened with Nightjar, once nothing else is in the way: a stream
    /// (this runs only between them), a stream being started, an open dialog or the PIN screen.
    fn open_waiting_link(&mut self) {
        if self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some() || !self.launcher.can_open_link() {
            return;
        }
        let Some(link) = self.links.take() else { return };
        // SAFETY: window calls on the main thread with a live window.
        unsafe {
            if SDL_GetWindowFlags(self.window) & SDL_WINDOW_MINIMIZED != 0 {
                SDL_RestoreWindow(self.window);
            }
            SDL_RaiseWindow(self.window);
        }
        self.launcher.open_link(&link);
    }

    /// Add PC: an address adds the PC; a QR pairing link adds it and pairs with the link's
    /// one-time PIN and certificate, at once: it was typed or pasted here on purpose.
    fn add_entry(&mut self, text: &str) {
        match hosts::parse_entry(text) {
            Ok(hosts::Entry::Address(address, port)) => {
                self.hosts.add_manual(&address, port);
                self.save_hosts();
            }
            Ok(hosts::Entry::Link(link)) => self.pair_link(*link),
            // Typed or pasted here on purpose: asked about all the same, as it can come from anyone.
            Ok(hosts::Entry::Invite(link)) => {
                self.launcher.dialog = Dialog::ConfirmInvite { link, shown: None }
            }
            // The link holds a PIN and passphrase: it is not shown back.
            Err(hosts::LINK_INVALID) => self
                .launcher
                .message(tr("Could not pair"), tr(hosts::LINK_INVALID)),
            Err(hosts::INVITE_INVALID) => self.launcher.message(
                tr("Could not join over the internet"),
                tr(hosts::INVITE_INVALID),
            ),
            Err(why) => self.launcher.message(tr(why), text.trim()),
        }
    }
}

/// Saves the PC list, logging a failure (the caller tells the player once: [`App::save_failed`]).
pub(crate) fn save_hosts(store: &config::Store, hosts: &Hosts) -> std::io::Result<()> {
    let saved = store.save_hosts(&hosts.records());
    if let Err(e) = &saved {
        eprintln!("nightjar: saving hosts failed: {e}");
    }
    saved
}

/// The words of a failed save: what happened, that the change lasts only this run, and what to
/// check. The folder is the config folder Nightjar could not write to.
pub(crate) fn save_failed_text(dir: &std::path::Path) -> String {
    trf("Nightjar could not write to {}. Changes apply until Nightjar closes, then go back to how they were; check that the folder is writable.", &[&dir.display()])
}

/// Whether this is the run's first failed save, the one the player is told: `told` then stays
/// true, so a read-only folder is said once instead of at every change.
pub(crate) fn first_save_failure(told: &mut bool) -> bool {
    !std::mem::replace(told, true)
}

/// Keeps the name chosen for a PC on this device, and saves it.
pub(crate) fn rename_host(hosts: &mut Hosts, store: &config::Store, id: u64, alias: &str) -> std::io::Result<()> {
    if hosts.rename(id, alias) {
        save_hosts(store, hosts)?;
    }
    Ok(())
}

/// Forgets a PC: its saved library, covers, recent games and favourites go with it.
pub(crate) fn remove_host(
    hosts: &mut Hosts,
    store: &config::Store,
    recents: &mut Vec<config::Recent>,
    favourites: &mut Vec<config::Favourite>,
    open_game: &mut Option<config::OpenGame>,
    id: u64,
) -> std::io::Result<()> {
    if let Some(unique_id) = hosts.get(id).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
        (*recents, *favourites) = store.forget_host(&unique_id, recents, favourites);
        *open_game = store.open_play().game;
    }
    hosts.remove(id);
    save_hosts(store, hosts)
}
