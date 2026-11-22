//! Mid-stream bitrate and frame-rate changes over 0x553C, on a stream whose DESCRIBE lists
//! them: the stream menu's requests, the client-hint bitrate controller's
//! (Lower bitrate's Lower it sooner, ticked once a second), and what the host answered.
//! Time is session-relative milliseconds.

use nightjar_core::http::AppRole;
use nightjar_policy::choices::LowerBitrate;
use nightjar_policy::client_hint;
use nightjar_policy::stats::Stats;
use rubylight_protocol::rate_change::{Reason, RateChangeRequest, RateChangeResult, Status, RATE_CHANGE_MESSAGE_TYPE};

use crate::strings::{tr, trf};

/// Time between two controller ticks.
pub const TICK_MS: u64 = 1000;
/// A request the host has not answered in this long is given up on, loudly.
pub const ANSWER_MS: u64 = 5000;

/// Whether the launch asks for rate changes (`rlRateChange=1`): full streams only.
pub fn asked_at_launch(role: AppRole) -> bool {
    role == AppRole::Stream
}

/// Whether the stream takes rate changes: the host listed 0x553C, which it does only for a
/// launch with `rlRateChange=1`, and the stream is a full one.
pub fn offered(host_control_messages: &[u16], role: AppRole) -> bool {
    asked_at_launch(role) && host_control_messages.contains(&RATE_CHANGE_MESSAGE_TYPE)
}

/// The stream menu's bitrate and rate controls. On a stream that takes 0x553C, "Change bitrate /
/// frame rate" replaces the HTTP bitrate row and the size editor's frame rate, so each shows once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuControls {
    /// "Change bitrate / frame rate".
    pub rate_change: bool,
    /// The bitrate field and "Apply bitrate" over HTTP.
    pub http_bitrate: bool,
    /// The 0x5532 size editor.
    pub size_editor: bool,
    /// The size editor's frame rate field.
    pub size_editor_fps: bool,
}

pub fn menu_controls(runtime_bitrate: bool, reconfigure: bool, rate_change: bool) -> MenuControls {
    MenuControls {
        rate_change,
        http_bitrate: runtime_bitrate && !rate_change,
        size_editor: reconfigure,
        size_editor_fps: reconfigure && !rate_change,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Menu,
    Hint,
    /// "Never: keep my bitrate": the stream's own bitrate, sent once so the PC stops lowering it.
    Keep,
    /// The query for the stream's bitrate as the PC counts it ([`RateChange::query_request`]).
    Query,
}

/// What a result changed, for the stream to follow.
#[derive(Debug, Default, PartialEq)]
pub struct Answer {
    /// Shown as a toast.
    pub toast: Option<String>,
    /// The stream's bitrate after the result.
    pub bitrate_kbps: Option<u32>,
    /// The stream's rate after the result, when it changed.
    pub fps_millihz: Option<u32>,
    /// The bitrate is one the player set in the menu, which a reconnect keeps.
    pub by_hand: bool,
}

pub struct RateChange {
    next_serial: u32,
    /// Requests waiting for their result, with when they were sent.
    pending: Vec<(u32, Origin, u64)>,
    /// The serial of the newest request whose result set the bitrate; 0 for none yet.
    bitrate_serial: u32,
    hint: Option<client_hint::Bitrate>,
    last_tick_ms: Option<u64>,
    /// "Never: keep my bitrate" is chosen and the takeover has not been sent yet.
    keep_pending: bool,
    /// The PC keeps this stream's bitrate: the takeover, the menu's Keep my bitrate or a bitrate
    /// the player chose in the menu went out (any bitrate change makes the PC stand back).
    kept: bool,
    /// The bitrate the player last chose in this stream's menu, which Keep my bitrate keeps.
    chosen_kbps: Option<u32>,
    /// The stream's bitrate at its start, which Keep my bitrate keeps when none was chosen: the
    /// PC's answer to the query once it came.
    configured_kbps: u32,
    /// The query is out and unanswered: the takeover and the client-hint controller wait for it.
    asking: bool,
    /// The query could not be sent at the start: when it goes once more.
    query_again_at: Option<u64>,
    query_retried: bool,
    /// The PC said it lowered the bitrate before the answer came ([`Self::pc_lowered`]).
    cut_before_answer: bool,
    /// The client-hint controller's cap and the cut a reconnect returns it to, for its start from
    /// the PC's answer.
    limit_kbps: u32,
    resume_kbps: Option<u32>,
    bitrate_kbps: u32,
    fps_millihz: u32,
    /// The menu's fields.
    pub menu_kbps: u32,
    pub menu_fps: f64,
}

impl RateChange {
    /// `hint` runs the client-hint controller from the stream's bitrate, under this client's
    /// cap `limit_kbps` (0 for none).
    pub fn new(bitrate_kbps: u32, fps_millihz: u32, hint: bool, limit_kbps: u32, now_ms: u64) -> Self {
        Self {
            next_serial: 1,
            pending: Vec::new(),
            bitrate_serial: 0,
            hint: hint.then(|| client_hint::Bitrate::new(bitrate_kbps, limit_kbps, now_ms)),
            last_tick_ms: None,
            keep_pending: false,
            kept: false,
            chosen_kbps: None,
            configured_kbps: bitrate_kbps,
            asking: false,
            query_again_at: None,
            query_retried: false,
            cut_before_answer: false,
            limit_kbps,
            resume_kbps: None,
            bitrate_kbps,
            fps_millihz,
            menu_kbps: bitrate_kbps,
            menu_fps: f64::from(fps_millihz) / 1000.0,
        }
    }

    /// "Never: keep my bitrate": [`Self::takeover_request`] sends the stream's bitrate once.
    pub fn keep_bitrate(&mut self) {
        self.keep_pending = true;
    }

    /// A reconnect after the player chose a bitrate in the menu: Keep my bitrate keeps that one,
    /// which came from this PC's own answers, whatever the query answers.
    pub fn chosen_before(&mut self, kbps: u32) {
        self.chosen_kbps = Some(kbps);
    }

    /// The query (bitrate and rate 0), at the stream's start before anything that sets a
    /// bitrate: every PC that takes 0x553C answers it without changing anything, with the
    /// stream's bitrate as it counts it (Rubylight 6.0.1 and older in the encoder's unit, newer
    /// ones in the launch's). Keep my bitrate and the client-hint controller wait for the answer
    /// and start from it; without one in [`ANSWER_MS`] they go on as before.
    pub fn query_request(&mut self, now_ms: u64) -> RateChangeRequest {
        self.asking = true;
        eprintln!("nightjar: rate change: asking the PC what it counts this stream's bitrate as");
        self.request(Origin::Query, 0, 0, now_ms)
    }

    /// The query could not be sent: once more a second later ([`Self::query_again`]), then the
    /// stream goes on without it. Quietly, as the stream did before it asked.
    pub fn query_unsent(&mut self, request: &RateChangeRequest, error: &dyn std::fmt::Display, now_ms: u64) {
        self.take(request.serial);
        if !std::mem::replace(&mut self.query_retried, true) {
            eprintln!("nightjar: rate change: the bitrate query could not be sent yet ({error}); trying again in a second");
            self.query_again_at = Some(now_ms + TICK_MS);
        } else {
            eprintln!("nightjar: rate change: the bitrate query could not be sent ({error}); the stream goes on from {} kbps", self.configured_kbps);
            self.asking = false;
        }
    }

    /// The PC said it lowered the bitrate for the network (a 0x5538 notice, limited or floor):
    /// an answer to the query that comes after it may hold that cut, so it is not used.
    pub fn pc_lowered(&mut self) {
        if self.asking { self.cut_before_answer = true; }
    }

    /// While the query waits, the menu's bitrate cannot change the stream's: a number picked now
    /// would be in the setting's unit, which an older PC takes as its encoder's. The menu shows the
    /// control disabled until the answer, its timeout, or an answer that is not used.
    pub fn bitrate_locked(&self) -> bool {
        self.asking
    }

    /// The stream menu's change, as `Streaming::change_rates` sends it.
    pub fn menu_change(&mut self, now_ms: u64) -> Result<RateChangeRequest, String> {
        self.menu_request(now_ms)
    }
    /// The query once more, a second after it could not be sent.
    pub fn query_again(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        if !self.query_again_at.is_some_and(|at| now_ms >= at) { return None; }
        self.query_again_at = None;
        Some(self.request(Origin::Query, 0, 0, now_ms))
    }

    /// The one request that makes the PC's link-rate and congestion controllers stand back for
    /// this stream: the stream's own bitrate as a bitrate change (the PC stops lowering what a
    /// client has set). `None` once sent, while the query waits for its answer, or when "Never"
    /// is not chosen.
    pub fn takeover_request(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        if self.asking || !std::mem::take(&mut self.keep_pending) { return None; }
        self.kept = true;
        let kbps = self.keep_kbps();
        eprintln!("nightjar: keep my bitrate: sending the stream's {kbps} kbps as a bitrate change so the PC stops lowering it");
        Some(self.request(Origin::Keep, kbps, 0, now_ms))
    }

    /// The stream's tick: the takeover goes out with the first tick that saw video.
    pub fn takeover_on_tick(&mut self, recent_video: bool, now_ms: u64) -> Option<RateChangeRequest> {
        if !recent_video { return None; }
        self.takeover_request(now_ms)
    }

    /// What the stream menu's Keep my bitrate shows: the PC keeps this stream's bitrate (or will,
    /// once the takeover goes out) and no controller of this device steers it.
    pub fn kept(&self) -> bool {
        (self.kept || self.keep_pending) && !self.hint_active()
    }

    /// The stream menu's Keep my bitrate turned on: the bitrate the player last chose in this
    /// stream, else the stream's own, as one bitrate change, and this device's own lowering
    /// stops. `None` when the PC already keeps it, so nothing more goes out.
    pub fn keep_now(&mut self, now_ms: u64) -> Option<RateChangeRequest> {
        let steering = self.hint_active();
        self.hint = None;
        if !steering && (self.kept || self.keep_pending) { return None; }
        if self.asking {
            // From the stream's bitrate as the PC counts it: the takeover goes out after its answer.
            eprintln!("nightjar: keep my bitrate (stream menu): goes out once the PC answered the bitrate query");
            self.keep_pending = true;
            return None;
        }
        self.keep_pending = false;
        self.kept = true;
        let kbps = self.keep_kbps();
        eprintln!("nightjar: keep my bitrate (stream menu): sending {kbps} kbps as a bitrate change so the PC stops lowering it");
        Some(self.request(Origin::Keep, kbps, 0, now_ms))
    }

    /// The bitrate Keep my bitrate keeps, at the start and from the switch alike: the one the player
    /// last chose in this stream, else the one the stream started with (negotiated).
    fn keep_kbps(&self) -> u32 {
        self.chosen_kbps.unwrap_or(self.configured_kbps)
    }

    /// The stream menu's Keep my bitrate turned off: the switch shows off and nothing is sent.
    /// The PC has no message that gives it control back, so this stream keeps its bitrate.
    pub fn release(&mut self) {
        self.kept = false;
        self.keep_pending = false;
    }

    /// After an automatic reconnect: the client-hint controller returns to the rate the stream
    /// before had cut to.
    pub fn resume_hint(&mut self, kbps: u32) {
        self.resume_kbps = Some(kbps);
        if let Some(hint) = &mut self.hint { hint.resume_from(kbps); }
    }

    /// The client-hint controller steers this stream's bitrate.
    pub fn hint_active(&self) -> bool {
        self.hint.as_ref().is_some_and(|hint| !hint.stopped())
    }

    /// The controller can still raise the bitrate.
    pub fn hint_has_room(&self) -> bool {
        self.hint.as_ref().is_some_and(|hint| !hint.stopped() && hint.current_kbps() < hint.max_kbps())
    }

    /// A menu request waits for its result.
    pub fn menu_busy(&self) -> bool {
        self.pending.iter().any(|(_, origin, _)| *origin == Origin::Menu)
    }

    fn request(&mut self, origin: Origin, bitrate_kbps: u32, fps_millihz: u32, now_ms: u64) -> RateChangeRequest {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        self.pending.push((serial, origin, now_ms));
        RateChangeRequest { serial, bitrate_kbps, fps_millihz }
    }

    /// The menu's request: only the fields that differ from the stream's. `Err` is a toast.
    pub fn menu_request(&mut self, now_ms: u64) -> Result<RateChangeRequest, String> {
        let fps_millihz = (self.menu_fps * 1000.0).round() as u32;
        if self.asking && self.menu_kbps != self.bitrate_kbps {
            eprintln!("nightjar: rate change: {} kbps not sent: the PC has not said yet what it counts this stream's bitrate as", self.menu_kbps);
            self.menu_kbps = self.bitrate_kbps;
        }
        let bitrate = if self.menu_kbps != self.bitrate_kbps { self.menu_kbps } else { 0 };
        let fps = if fps_millihz != self.fps_millihz { fps_millihz } else { 0 };
        if bitrate == 0 && fps == 0 {
            return Err(tr("The stream already runs at this bitrate and frame rate.").to_owned());
        }
        let request = RateChangeRequest { serial: 0, bitrate_kbps: bitrate, fps_millihz: fps };
        if !request.valid() {
            return Err(tr("This stream cannot use the requested size or frame rate.").to_owned());
        }
        Ok(self.request(Origin::Menu, bitrate, fps, now_ms))
    }

    /// Once a second during the stream: the controller's request, if it wants a change.
    pub fn tick(&mut self, stats: &Stats) -> Option<RateChangeRequest> {
        // A change by hand goes first: the controller's would supersede it. Nor before the PC's
        // answer to the query: the controller starts from it.
        if self.menu_busy() || self.asking {
            return None;
        }
        let hint = self.hint.as_mut()?;
        if self.last_tick_ms.is_some_and(|at| stats.now_ms < at + TICK_MS) {
            return None;
        }
        self.last_tick_ms = Some(stats.now_ms);
        let kbps = hint.sample(stats)?;
        Some(self.request(Origin::Hint, kbps, 0, stats.now_ms))
    }

    /// Gives up on requests the host has not answered within [`ANSWER_MS`]: their toast. The
    /// controller stops, as for a refusal.
    pub fn expire(&mut self, now_ms: u64) -> Option<String> {
        let mut toast = None;
        while let Some(at) = self.pending.iter().position(|(_, _, sent)| now_ms >= sent + ANSWER_MS) {
            let (serial, origin, _) = self.pending.remove(at);
            eprintln!("nightjar: rate change {serial} got no answer in {ANSWER_MS} ms");
            toast = Some(match origin {
                Origin::Menu => tr("The PC did not answer the rate change.").to_owned(),
                Origin::Hint => {
                    if let Some(hint) = &mut self.hint { hint.refused(); }
                    trf("Loss-based bitrate stopped: {}", &[&tr("the PC did not answer")])
                }
                Origin::Keep => {
                    eprintln!("nightjar: keep my bitrate: the PC did not answer; it may still lower the bitrate");
                    self.kept = false;
                    continue;
                }
                Origin::Query => {
                    // Quietly, as before the query: the takeover and the controller start from the stream's own.
                    eprintln!("nightjar: rate change: the PC did not answer the bitrate query; the stream goes on from {} kbps", self.configured_kbps);
                    self.asking = false;
                    continue;
                }
            });
        }
        toast
    }

    /// `request` could not be sent: its toast. The controller stops, as for a refusal.
    pub fn send_failed(&mut self, request: &RateChangeRequest, error: &dyn std::fmt::Display) -> String {
        let origin = self.take(request.serial);
        if origin == Some(Origin::Hint) {
            if let Some(hint) = &mut self.hint { hint.refused(); }
            return trf("Loss-based bitrate stopped: {}", &[error]);
        }
        if origin == Some(Origin::Keep) {
            eprintln!("nightjar: keep my bitrate: the change could not be sent ({error}); the PC may still lower the bitrate");
            self.kept = false;
            return trf("Could not change the stream: {}", &[error]);
        }
        trf("Could not change the stream: {}", &[error])
    }

    fn take(&mut self, serial: u32) -> Option<Origin> {
        let at = self.pending.iter().position(|(s, _, _)| *s == serial)?;
        Some(self.pending.remove(at).1)
    }

    /// The host's answer to one request.
    pub fn result(&mut self, result: &RateChangeResult, now_ms: u64) -> Answer {
        let Some(origin) = self.take(result.serial) else {
            eprintln!("nightjar: rate change result for serial {} that is not pending, ignored", result.serial);
            return Answer::default();
        };
        if origin == Origin::Query && (self.query_retried || self.cut_before_answer) {
            // Video ran before the PC handled it (a retried query), or the PC said it lowered the
            // bitrate first: the answer may be a cut, which would be kept for the whole stream.
            eprintln!("nightjar: rate change: the PC's answer to the bitrate query ({} kbps) may hold a cut it made {}; the stream goes on from {} kbps",
                result.bitrate_kbps, if self.cut_before_answer { "before it" } else { "while the query waited to go out" }, self.configured_kbps);
            self.asking = false;
            self.query_again_at = None;
            return Answer::default();
        }
        let mut answer = Answer::default();
        let before_kbps = self.bitrate_kbps;
        // The host answers a request with a rate once its switch ran, with the bitrate it set
        // then, so a bitrate-only request sent after it is answered first: an older result keeps
        // the newer bitrate.
        let newest = self.bitrate_serial == 0 || (result.serial.wrapping_sub(self.bitrate_serial) as i32) > 0;
        if result.bitrate_kbps > 0 && newest {
            self.bitrate_serial = result.serial;
            self.bitrate_kbps = result.bitrate_kbps;
            self.menu_kbps = result.bitrate_kbps;
            answer.bitrate_kbps = Some(result.bitrate_kbps);
        }
        if result.fps_millihz > 0 && result.fps_millihz != self.fps_millihz {
            self.fps_millihz = result.fps_millihz;
            answer.fps_millihz = Some(result.fps_millihz);
        }
        self.menu_fps = f64::from(self.fps_millihz) / 1000.0;
        let changed = matches!(result.bitrate.status, Status::Applied | Status::Clamped);
        match origin {
            Origin::Menu => {
                if changed && answer.bitrate_kbps.is_some() {
                    // A bitrate set by hand: the controller's range follows it.
                    if let Some(hint) = &mut self.hint { hint.overridden(self.bitrate_kbps, now_ms); }
                    answer.by_hand = true;
                    // The PC stands back for a bitrate the player chose, as for the takeover.
                    self.chosen_kbps = Some(self.bitrate_kbps);
                    self.kept = true;
                }
                answer.toast = Some(menu_text(result));
            }
            Origin::Keep => {
                eprintln!("nightjar: keep my bitrate: the PC answered {:?} at {} kbps{}", result.bitrate.status, result.bitrate_kbps,
                    if changed { "; it no longer lowers the bitrate" } else { "; it may still lower the bitrate" });
                // Refused: the PC still lowers the bitrate, so the switch shows off and the notice may say so.
                if !changed { self.kept = false; }
            }
            Origin::Query => {
                self.asking = false;
                self.query_again_at = None;
                if result.bitrate_kbps > 0 {
                    eprintln!("nightjar: rate change: the PC counts this stream's bitrate as {} kbps; keep my bitrate and the client-hint controller start from it", result.bitrate_kbps);
                    self.configured_kbps = result.bitrate_kbps;
                    // Unless a change by hand was answered first: the controller follows that one.
                    if answer.bitrate_kbps.is_some()
                        && let Some(hint) = &mut self.hint {
                            *hint = client_hint::Bitrate::new(result.bitrate_kbps, self.limit_kbps, now_ms);
                            if let Some(kbps) = self.resume_kbps { hint.resume_from(kbps); }
                        }
                } else {
                    eprintln!("nightjar: rate change: the PC's answer to the bitrate query named no bitrate; the stream goes on from {} kbps", self.configured_kbps);
                }
                // It changed nothing: the stream follows only a count that differs from its own.
                if answer.bitrate_kbps == Some(before_kbps) { answer.bitrate_kbps = None; }
            }
            Origin::Hint => {
                let Some(hint) = &mut self.hint else { return answer };
                if changed {
                    hint.applied(self.bitrate_kbps, now_ms);
                } else if result.bitrate.reason == Reason::Superseded {
                    // A change by hand replaced it; that one's result sets the range.
                    hint.overridden(self.bitrate_kbps, now_ms);
                } else {
                    hint.refused();
                    answer.toast = Some(trf("Loss-based bitrate stopped: {}", &[&reason_text(result.bitrate.reason)]));
                }
            }
        }
        answer
    }

    /// The bitrate changed another way (by hand over HTTP): the controller's range follows.
    pub fn bitrate_set(&mut self, kbps: u32, now_ms: u64) {
        self.bitrate_kbps = kbps;
        self.menu_kbps = kbps;
        if let Some(hint) = &mut self.hint { hint.overridden(kbps, now_ms); }
    }

    /// The rate changed another way (a 0x5532 reconfigure).
    pub fn fps_set(&mut self, fps_millihz: u32) {
        self.fps_millihz = fps_millihz;
        self.menu_fps = f64::from(fps_millihz) / 1000.0;
    }
}

/// Whether the stream starts a bitrate controller of this device (the busy-network one, also for
/// automatic quality): never under Never or after a bitrate the player chose.
pub fn own_lowering_at_start(manual_bitrate: bool, keep_bitrate: bool, adaptive_bitrate: bool, automatic_quality: bool) -> bool {
    !manual_bitrate && !keep_bitrate && (adaptive_bitrate || automatic_quality)
}

/// Whether a stream sends the takeover once its video runs: Never is chosen, or a reconnect starts
/// at a bitrate the player chose (the PC's new session would lower it again) with no controller of
/// this device steering.
pub fn takeover_at_start(keep_bitrate: bool, player_chose_bitrate: bool, hint: bool) -> bool {
    keep_bitrate || (player_chose_bitrate && !hint)
}

/// What the stream menu's Keep my bitrate shows; `None` hides it, on a PC that did not take this
/// stream's rate changes. Off while this device's busy-network controller steers the bitrate.
pub fn keep_switch(rates: Option<&RateChange>, adaptive_bitrate: bool) -> Option<bool> {
    rates.map(|rates| rates.kept() && !adaptive_bitrate)
}

/// Keep my bitrate as the main settings keep it: Lower bitrate's Never when on, The PC decides
/// when off, stored as choosing them in Settings stores them.
pub fn store_keep(settings: &mut crate::config::Settings, keep: bool) {
    settings.lower_bitrate = if keep { LowerBitrate::Keep } else { LowerBitrate::Off };
}

fn fps_text(fps_millihz: u32) -> String {
    let fps = f64::from(fps_millihz) / 1000.0;
    if fps_millihz.is_multiple_of(1000) { format!("{fps:.0}") } else { format!("{fps:.3}") }
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::Invalid => tr("the request was not valid"),
        Reason::Disabled => tr("the PC does not change this stream’s rates"),
        Reason::Codec => tr("this stream’s codec keeps its rates"),
        Reason::HostLimit => tr("the PC’s limit"),
        Reason::EncoderLimit => tr("the encoder’s limit"),
        Reason::EncoderFailed => tr("the encoder could not run at it"),
        Reason::Superseded => tr("a later change replaced it"),
        Reason::None | Reason::Other => tr("the PC gave no reason"),
    }
}

/// The toast for a menu request's result: what became of each value asked for.
fn menu_text(result: &RateChangeResult) -> String {
    let kbps = result.bitrate_kbps;
    let fps = fps_text(result.fps_millihz);
    let bitrate = match result.bitrate.status {
        Status::Unchanged => None,
        Status::Applied => Some(trf("Bitrate set to {} kbps.", &[&kbps])),
        Status::Clamped => Some(trf("Bitrate limited to {} kbps: {}.", &[&kbps, &reason_text(result.bitrate.reason)])),
        Status::Refused => Some(trf("Bitrate not changed: {}.", &[&reason_text(result.bitrate.reason)])),
    };
    let rate = match result.fps.status {
        Status::Unchanged => None,
        Status::Applied => Some(trf("Frame rate set to {} fps.", &[&fps])),
        Status::Clamped => Some(trf("Frame rate limited to {} fps: {}.", &[&fps, &reason_text(result.fps.reason)])),
        Status::Refused => Some(trf("Frame rate not changed: {}.", &[&reason_text(result.fps.reason)])),
    };
    match (bitrate, rate) {
        (Some(a), Some(b)) => format!("{a} {b}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => tr("The PC changed nothing.").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::rate_change::Outcome;

    fn stats(second: u64, loss: f32) -> Stats {
        Stats { now_ms: second * 1000, recent_video: true, loss_percent: Some(loss), ..Stats::default() }
    }

    fn result(request: &RateChangeRequest, bitrate_kbps: u32, fps_millihz: u32, bitrate: Outcome, fps: Outcome) -> RateChangeResult {
        RateChangeResult { serial: request.serial, bitrate_kbps, fps_millihz, bitrate, fps }
    }

    #[test]
    fn only_a_full_stream_whose_host_listed_0x553c_offers_the_menu() {
        assert!(offered(&[0x5530, 0x553C], AppRole::Stream));
        assert!(!offered(&[0x5530, 0x5532], AppRole::Stream), "a launch the host did not take");
        assert!(!offered(&[], AppRole::Stream));
        assert!(!offered(&[0x553C], AppRole::InputOnly));
        assert!(!offered(&[0x553C], AppRole::RemoteMonitor));
    }

    #[test]
    fn a_full_stream_asks_for_rate_changes_at_launch_and_no_other_does() {
        use nightjar_core::http::{HostCapabilities, LaunchRequest};
        for (role, asked) in [(AppRole::Stream, true), (AppRole::RemoteMonitor, false), (AppRole::InputOnly, false)] {
            let request = LaunchRequest { role, rate_change: asked_at_launch(role), ..LaunchRequest::default() };
            assert_eq!(request.query(&HostCapabilities::UNKNOWN, false).contains("&rlRateChange=1"), asked, "{role:?}");
        }
    }

    #[test]
    fn the_rate_change_entry_replaces_the_other_bitrate_and_rate_controls() {
        // A PC with 0x553C: one bitrate and one rate control; the size editor keeps the size.
        assert_eq!(menu_controls(true, true, true),
            MenuControls { rate_change: true, http_bitrate: false, size_editor: true, size_editor_fps: false });
        assert_eq!(menu_controls(false, false, true),
            MenuControls { rate_change: true, http_bitrate: false, size_editor: false, size_editor_fps: false });
        // A PC without it keeps the older controls.
        assert_eq!(menu_controls(true, true, false),
            MenuControls { rate_change: false, http_bitrate: true, size_editor: true, size_editor_fps: true });
        assert_eq!(menu_controls(true, false, false),
            MenuControls { rate_change: false, http_bitrate: true, size_editor: false, size_editor_fps: false });
        assert_eq!(menu_controls(false, false, false),
            MenuControls { rate_change: false, http_bitrate: false, size_editor: false, size_editor_fps: false });
    }

    #[test]
    fn the_menu_asks_only_for_what_changed_and_shows_every_answer() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        assert!(rates.menu_request(0).is_err(), "nothing to change");
        rates.menu_kbps = 40_000;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (40_000, 0));
        assert!(rates.menu_busy());
        let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::clamped(Reason::HostLimit), Outcome::UNCHANGED), 1000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate limited to 30000 kbps: the PC’s limit."));
        assert_eq!((answer.bitrate_kbps, answer.fps_millihz), (Some(30_000), None));
        assert!(!rates.menu_busy());
        assert_eq!(rates.menu_kbps, 30_000, "the field shows what the stream runs at");

        rates.menu_kbps = 50_000;
        rates.menu_fps = 119.88;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (50_000, 119_880));
        let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::APPLIED, Outcome::APPLIED), 2000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate set to 50000 kbps. Frame rate set to 119.880 fps."));
        assert_eq!(answer.fps_millihz, Some(119_880));

        // Refusals are loud too.
        rates.menu_fps = 144.0;
        let request = rates.menu_request(0).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 144_000));
        let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::UNCHANGED, Outcome::refused(Reason::Disabled)), 3000);
        assert_eq!(answer.toast.as_deref(), Some("Frame rate not changed: the PC does not change this stream’s rates."));
        assert_eq!(answer.fps_millihz, None);
        assert_eq!(rates.menu_fps, 119.88, "the field goes back to the stream's rate");

        rates.menu_fps = 5.0;
        assert!(rates.menu_request(0).is_err(), "below the protocol's lowest rate");
        assert!(!rates.menu_busy());
        rates.menu_fps = 119.88;

        // A result nobody asked for changes nothing.
        let stray = RateChangeResult { serial: 99, ..result(&request, 1000, 30_000, Outcome::APPLIED, Outcome::APPLIED) };
        assert_eq!(rates.result(&stray, 4000), Answer::default());

        crate::strings::set(crate::strings::Lang::De);
        rates.menu_kbps = 10_000;
        let request = rates.menu_request(0).unwrap();
        let answer = rates.result(&result(&request, 10_000, 119_880, Outcome::APPLIED, Outcome::UNCHANGED), 5000);
        assert_eq!(answer.toast.as_deref(), Some("Bitrate auf 10000 kbit/s gesetzt."));
    }

    #[test]
    fn the_controller_ticks_once_a_second_and_one_request_at_a_time() {
        let mut off = RateChange::new(20_000, 60_000, false, 0, 0);
        assert!((0..30).all(|s| off.tick(&stats(s, 10.0)).is_none()), "off by default");

        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        assert!(rates.hint_active());
        let at = |ms: u64| Stats { now_ms: ms, ..stats(0, 10.0) };
        assert_eq!(rates.tick(&at(2500)), None, "start delay");
        assert_eq!(rates.tick(&at(3000)), None, "under a second since the last tick");
        let request = rates.tick(&at(3500)).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (14_000, 0));
        assert!(!rates.menu_busy(), "the controller's request is not the menu's");
        assert_eq!(rates.tick(&stats(4, 10.0)), None, "the first is still pending");
        let answer = rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 4000);
        assert_eq!(answer, Answer { bitrate_kbps: Some(14_000), ..Answer::default() }, "the controller's changes are quiet");
        assert_eq!(rates.tick(&stats(5, 10.0)), None, "cooldown");
        let request = rates.tick(&stats(6, 10.0)).unwrap();
        assert_eq!(request.bitrate_kbps, 9800);

        // A change by hand supersedes it: the controller follows instead of stopping.
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(0).unwrap();
        assert!(rates.result(&result(&request, 14_000, 60_000, Outcome::refused(Reason::Superseded), Outcome::UNCHANGED), 6100).toast.is_none());
        rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 6200);
        assert!(rates.hint_active());
        let request = rates.tick(&stats(9, 10.0)).unwrap();
        assert_eq!(request.bitrate_kbps, 21_000, "from the bitrate set by hand");

        // A refusal stops it, loudly.
        let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::refused(Reason::Codec), Outcome::UNCHANGED), 10_000);
        assert_eq!(answer.toast.as_deref(), Some("Loss-based bitrate stopped: this stream’s codec keeps its rates"));
        assert!(!rates.hint_active() && !rates.hint_has_room());
        assert!((11..60).all(|s| rates.tick(&stats(s, 10.0)).is_none()));
    }

    /// After an automatic reconnect the client-hint controller's first request is the cut the
    /// stream before had made; without a controller nothing is asked.
    #[test]
    fn a_resumed_hint_asks_for_the_earlier_cut_first() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.resume_hint(9_000);
        assert_eq!(rates.tick(&stats(4, 0.0)).map(|request| request.bitrate_kbps), Some(9_000));
        let mut off = RateChange::new(20_000, 60_000, false, 0, 0);
        off.resume_hint(9_000);
        assert!(off.tick(&stats(4, 0.0)).is_none());
    }

    #[test]
    fn a_result_that_comes_after_a_later_requests_keeps_that_ones_bitrate() {
        // The host holds the answer to a request with a rate until its switch ran, with the
        // bitrate it set then; a bitrate-only request sent after it is answered first.
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.menu_kbps = 30_000;
        rates.menu_fps = 120.0;
        let first = rates.menu_request(0).unwrap();
        rates.menu_kbps = 40_000;
        rates.menu_fps = 60.0;
        let second = rates.menu_request(100).unwrap();
        assert_eq!((second.bitrate_kbps, second.fps_millihz), (40_000, 0));
        let newer = rates.result(&result(&second, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 200);
        assert_eq!(newer.bitrate_kbps, Some(40_000));
        let older = rates.result(&result(&first, 30_000, 120_000, Outcome::APPLIED, Outcome::APPLIED), 900);
        assert_eq!(older.toast.as_deref(), Some("Bitrate set to 30000 kbps. Frame rate set to 120 fps."), "still said");
        assert_eq!((older.bitrate_kbps, older.fps_millihz), (None, Some(120_000)), "the rate is the stream's, the bitrate not");
        assert_eq!(rates.menu_kbps, 40_000);
        let request = rates.tick(&stats(4, 10.0)).unwrap();
        assert_eq!(request.bitrate_kbps, 28_000, "the controller follows the later bitrate");

        // Serials wrap past 0; one after the wrap is still newer.
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        rates.next_serial = u32::MAX;
        rates.menu_kbps = 30_000;
        let before = rates.menu_request(0).unwrap();
        rates.menu_kbps = 40_000;
        let after = rates.menu_request(0).unwrap();
        assert_eq!((before.serial, after.serial), (u32::MAX, 1));
        assert_eq!(rates.result(&result(&before, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 100).bitrate_kbps, Some(30_000));
        assert_eq!(rates.result(&result(&after, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 200).bitrate_kbps, Some(40_000));
    }

    #[test]
    fn never_keep_my_bitrate_sends_the_streams_bitrate_once_and_only_when_chosen() {
        let mut plain = RateChange::new(25_000, 60_000, false, 0, 0);
        assert_eq!(plain.takeover_request(1000), None, "another choice sends nothing");
        let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        let request = rates.takeover_request(1000).expect("the takeover");
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (25_000, 0), "the configured bitrate, no rate");
        assert_eq!(rates.takeover_request(2000), None, "once");
        assert!((0..30).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "no controller of its own runs");
        // The PC's answer is followed and shows nothing.
        let answer = rates.result(&result(&request, 25_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 1100);
        assert_eq!((answer.bitrate_kbps, answer.toast, answer.by_hand), (Some(25_000), None, false));
        // A PC that clamped it: the stream follows the PC's rate.
        let mut clamped = RateChange::new(25_000, 60_000, false, 0, 0);
        clamped.keep_bitrate();
        let request = clamped.takeover_request(0).unwrap();
        let answer = clamped.result(&result(&request, 20_000, 60_000, Outcome::clamped(Reason::HostLimit), Outcome::UNCHANGED), 100);
        assert_eq!((answer.bitrate_kbps, answer.toast), (Some(20_000), None));
    }

    #[test]
    fn keep_my_bitrate_in_the_menu_sends_once_the_bitrate_the_player_chose_or_the_streams() {
        // Absent on a PC that did not take this stream's rate changes.
        assert_eq!(keep_switch(None, false), None);
        // A stream started with Never shows it on before and after the takeover, and turning it on sends nothing more.
        let mut never = RateChange::new(25_000, 60_000, false, 0, 0);
        never.keep_bitrate();
        assert_eq!(keep_switch(Some(&never), false), Some(true));
        assert_eq!(never.keep_now(500), None, "the takeover is still to go out");
        assert!(never.takeover_request(1000).is_some());
        assert_eq!(keep_switch(Some(&never), false), Some(true));
        assert_eq!((never.keep_now(2000), never.takeover_request(3000)), (None, None), "nothing more");
        // The PC decides: off until turned on, then exactly one change at the stream's bitrate.
        let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
        assert_eq!(keep_switch(Some(&rates), false), Some(false));
        let request = rates.keep_now(1000).expect("one change");
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (25_000, 0), "the stream's bitrate, no rate");
        assert_eq!(rates.keep_now(2000), None, "once");
        let answer = rates.result(&result(&request, 25_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 1100);
        assert_eq!((answer.toast, answer.by_hand), (None, false), "its answer shows nothing");
        // Off shows off and sends nothing.
        rates.release();
        assert_eq!(keep_switch(Some(&rates), false), Some(false));
        assert_eq!(rates.takeover_request(3000), None);
        // A bitrate the player chose in the menu already makes the PC stand back: on, and kept at that bitrate.
        let mut chosen = RateChange::new(25_000, 60_000, false, 0, 0);
        chosen.menu_kbps = 40_000;
        let manual = chosen.menu_request(0).unwrap();
        chosen.result(&result(&manual, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 100);
        assert_eq!(keep_switch(Some(&chosen), false), Some(true));
        assert_eq!(chosen.keep_now(150), None, "the PC already keeps it");
        chosen.release();
        assert_eq!(chosen.keep_now(200).map(|r| r.bitrate_kbps), Some(40_000), "the player's bitrate, not the stream's");
        // Lower it sooner steering: off; on stops that controller and sends the stream's own bitrate.
        let mut hint = RateChange::new(25_000, 60_000, true, 0, 0);
        assert_eq!(keep_switch(Some(&hint), false), Some(false));
        assert_eq!(hint.keep_now(1000).map(|r| r.bitrate_kbps), Some(25_000));
        assert!(!hint.hint_active() && hint.kept());
        assert!((0..30).all(|s| hint.tick(&stats(s, 10.0)).is_none()), "this device lowers nothing");
        // The busy-network controller steering: off.
        assert_eq!(keep_switch(Some(&never), true), Some(false));
    }

    #[test]
    fn a_takeover_the_pc_refused_never_answered_or_that_could_not_go_out_is_not_kept() {
        let mut refused = RateChange::new(25_000, 60_000, false, 0, 0);
        refused.keep_bitrate();
        let request = refused.takeover_on_tick(true, 1000).unwrap();
        refused.result(&result(&request, 25_000, 60_000, Outcome::refused(Reason::Disabled), Outcome::UNCHANGED), 1100);
        assert_eq!(keep_switch(Some(&refused), false), Some(false));
        let mut silent = RateChange::new(25_000, 60_000, false, 0, 0);
        silent.keep_bitrate();
        silent.takeover_on_tick(true, 1000).unwrap();
        assert_eq!(silent.expire(1000 + ANSWER_MS), None, "says nothing");
        assert_eq!(keep_switch(Some(&silent), false), Some(false));
        let mut unsent = RateChange::new(25_000, 60_000, false, 0, 0);
        let request = unsent.keep_now(0).unwrap();
        unsent.send_failed(&request, &"closed");
        assert!(!unsent.kept());
    }

    #[test]
    fn the_takeover_goes_out_with_the_first_tick_that_saw_video_and_the_start_rules_hold() {
        let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        assert_eq!(rates.takeover_on_tick(false, 500), None, "no video yet");
        assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(25_000));
        assert_eq!(rates.takeover_on_tick(true, 2000), None);
        // Under Never, or after a bitrate the player chose, the stream starts no controller of its own.
        assert!(!own_lowering_at_start(false, true, true, true));
        assert!(!own_lowering_at_start(true, false, true, false));
        assert!(own_lowering_at_start(false, false, true, false) && own_lowering_at_start(false, false, false, true));
        // A reconnect sends the takeover again for Never and for a bitrate the player chose.
        assert!(takeover_at_start(true, false, false));
        assert!(takeover_at_start(false, true, false));
        assert!(!takeover_at_start(false, true, true), "Lower it sooner steers it");
        assert!(!takeover_at_start(false, false, false));
    }

    /// The PC's answer to the query: the stream's bitrate as it counts it, nothing changed. Rubylight
    /// 6.0.1 counts a stream launched at 20000 as its encoder's 14988, newer PCs as 20000.
    fn answered(request: &RateChangeRequest, kbps: u32) -> RateChangeResult {
        result(request, kbps, 60_000, Outcome::UNCHANGED, Outcome::refused(Reason::Invalid))
    }

    #[test]
    fn on_an_older_pc_keep_my_bitrate_and_the_menu_start_from_what_it_answered() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        let query = rates.query_request(0);
        assert_eq!((query.bitrate_kbps, query.fps_millihz), (0, 0), "asks for nothing");
        assert_eq!(rates.takeover_on_tick(true, 1000), None, "nothing sets a bitrate before the answer");
        assert_eq!(keep_switch(Some(&rates), false), Some(true));
        let answer = rates.result(&answered(&query, 14_988), 1100);
        assert_eq!((answer.toast, answer.by_hand, answer.fps_millihz), (None, false, None), "says nothing");
        assert_eq!(answer.bitrate_kbps, Some(14_988), "the stream follows the PC's count");
        assert_eq!(rates.menu_kbps, 14_988, "the menu's field");
        let keep = rates.takeover_on_tick(true, 2000).expect("the takeover");
        assert_eq!((keep.bitrate_kbps, keep.fps_millihz), (14_988, 0), "not 20000, which would raise the encoder");
        assert_eq!(rates.takeover_on_tick(true, 3000), None, "once");
        // A PC's answer to the query is never a takeover's: the switch stays on.
        assert_eq!(keep_switch(Some(&rates), false), Some(true));
    }

    #[test]
    fn on_an_older_pc_the_client_hint_controller_starts_from_what_it_answered() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        let query = rates.query_request(0);
        assert!((0..10).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "nothing before the answer");
        assert!(rates.hint_active(), "it owns the bitrate meanwhile, so the PC's adaptive bitrate stays off");
        rates.result(&answered(&query, 14_988), 10_000);
        let cut = (11..20).find_map(|s| rates.tick(&stats(s, 10.0))).expect("a cut");
        assert!(cut.bitrate_kbps < 14_988, "{}", cut.bitrate_kbps);
    }

    #[test]
    fn on_a_newer_pc_the_answer_is_the_streams_own_bitrate_and_everything_is_as_before() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        let query = rates.query_request(0);
        let answer = rates.result(&answered(&query, 20_000), 100);
        assert_eq!(answer, Answer::default(), "nothing to follow");
        assert_eq!(rates.tick(&stats(2, 10.0)), None, "the start delay");
        assert_eq!(rates.tick(&stats(4, 10.0)).map(|r| r.bitrate_kbps), Some(14_000));
        let mut kept = RateChange::new(20_000, 60_000, false, 0, 0);
        kept.keep_bitrate();
        let query = kept.query_request(0);
        kept.result(&answered(&query, 20_000), 100);
        assert_eq!(kept.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000));
    }

    #[test]
    fn with_no_answer_the_stream_goes_on_as_before_once_and_a_late_answer_changes_nothing() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        let query = rates.query_request(0);
        assert_eq!(rates.takeover_on_tick(true, 4000), None);
        assert_eq!(rates.expire(ANSWER_MS - 1), None);
        assert_eq!(rates.expire(ANSWER_MS), None, "quietly");
        assert_eq!(rates.takeover_on_tick(true, ANSWER_MS).map(|r| r.bitrate_kbps), Some(20_000), "the stream's own");
        assert_eq!(rates.result(&answered(&query, 14_988), ANSWER_MS + 100), Answer::default(), "too late");
        assert_eq!(rates.menu_kbps, 20_000);
        assert_eq!(rates.takeover_on_tick(true, ANSWER_MS + 1000), None, "once");

        let mut hint = RateChange::new(20_000, 60_000, true, 0, 0);
        hint.query_request(0);
        assert!((0..5).all(|s| hint.tick(&stats(s, 10.0)).is_none()));
        assert_eq!(hint.expire(ANSWER_MS), None);
        assert_eq!(hint.tick(&stats(5, 10.0)).map(|r| r.bitrate_kbps), Some(14_000), "from the stream's own");
    }

    #[test]
    fn a_query_that_cannot_be_sent_yet_goes_once_more_a_second_later() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        let query = rates.query_request(0);
        rates.query_unsent(&query, &"not yet", 0);
        assert_eq!(rates.query_again(500), None);
        assert_eq!(rates.takeover_on_tick(true, 500), None, "still waits for it");
        let again = rates.query_again(1000).expect("once more");
        assert_eq!((again.bitrate_kbps, again.fps_millihz), (0, 0));
        assert_eq!(rates.query_again(2000), None);
        // Video ran meanwhile, so a cut by the PC may be in the answer: not kept, the stream's own goes out.
        assert_eq!(rates.result(&answered(&again, 14_988), 1100), Answer::default());
        assert_eq!(rates.menu_kbps, 20_000);
        assert_eq!(rates.takeover_on_tick(true, 2000).map(|r| r.bitrate_kbps), Some(20_000));

        // Twice unsent: the stream goes on from its own bitrate, as before the query.
        let mut unsent = RateChange::new(20_000, 60_000, false, 0, 0);
        unsent.keep_bitrate();
        let query = unsent.query_request(0);
        unsent.query_unsent(&query, &"closed", 0);
        let again = unsent.query_again(1000).unwrap();
        unsent.query_unsent(&again, &"closed", 1000);
        assert_eq!(unsent.query_again(5000), None);
        assert_eq!(unsent.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000));
    }

    #[test]
    fn a_cut_the_pc_made_before_the_answer_is_not_kept() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        rates.keep_bitrate();
        let query = rates.query_request(0);
        // The PC's link-rate controller lowered the bitrate before it handled the query: its answer is the cut.
        rates.pc_lowered();
        assert_eq!(rates.result(&answered(&query, 12_000), 100), Answer::default());
        assert_eq!(rates.menu_kbps, 20_000);
        assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000), "the stream's own, as before the query");

        let mut hint = RateChange::new(20_000, 60_000, true, 0, 0);
        let query = hint.query_request(0);
        hint.pc_lowered();
        hint.result(&answered(&query, 12_000), 100);
        assert_eq!(hint.tick(&stats(4, 10.0)).map(|r| r.bitrate_kbps), Some(14_000), "from the stream's own");

        // After the answer a cut changes nothing of it.
        let mut later = RateChange::new(20_000, 60_000, false, 0, 0);
        later.keep_bitrate();
        let query = later.query_request(0);
        later.result(&answered(&query, 14_988), 100);
        later.pc_lowered();
        assert_eq!(later.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(14_988));
    }

    #[test]
    fn while_the_query_waits_the_menu_cannot_change_the_bitrate() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        let query = rates.query_request(0);
        assert!(rates.bitrate_locked());
        // Refused by the state, not only by the disabled control: the field goes back to the stream's.
        rates.menu_kbps = 30_000;
        assert!(rates.menu_change(100).is_err(), "nothing else to change");
        assert_eq!(rates.menu_kbps, 20_000);
        rates.menu_kbps = 30_000;
        rates.menu_fps = 120.0;
        let request = rates.menu_change(200).unwrap();
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 120_000), "the rate alone");
        // A clean answer: usable, at the PC's number.
        rates.result(&answered(&query, 14_988), 300);
        assert!(!rates.bitrate_locked());
        assert_eq!(rates.menu_kbps, 14_988);
        rates.menu_kbps = 30_000;
        assert_eq!(rates.menu_change(400).map(|r| r.bitrate_kbps), Ok(30_000));

        // An answer that is not used: usable, at the stream's own number.
        let mut tainted = RateChange::new(20_000, 60_000, false, 0, 0);
        let query = tainted.query_request(0);
        tainted.pc_lowered();
        tainted.result(&answered(&query, 12_000), 100);
        assert!(!tainted.bitrate_locked());
        assert_eq!(tainted.menu_kbps, 20_000);

        // No answer: usable after the timeout, at the stream's own number.
        let mut silent = RateChange::new(20_000, 60_000, false, 0, 0);
        silent.query_request(0);
        assert!(silent.bitrate_locked());
        silent.expire(ANSWER_MS);
        assert!(!silent.bitrate_locked());
        assert_eq!(silent.menu_kbps, 20_000);

        // A reconnect is a new stream: a new wait, the control locked and nothing left from before.
        let mut again = RateChange::new(30_000, 60_000, false, 0, 0);
        again.query_request(0);
        assert!(again.bitrate_locked());
        assert_eq!(again.menu_kbps, 30_000);
    }

    /// The bitrate's unit plays no part in a frame rate: picked before the answer, it goes out at once
    /// with the bitrate unchanged, and the bitrate stays the stream's after an older PC's answer.
    #[test]
    fn a_frame_rate_picked_before_the_answer_goes_out_at_once_and_leaves_the_bitrate_alone() {
        let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
        let query = rates.query_request(0);
        rates.menu_fps = 120.0;
        let request = rates.menu_change(100).expect("goes out at once");
        assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 120_000));
        rates.result(&answered(&query, 14_988), 200);
        assert_eq!(rates.menu_kbps, 14_988, "the menu shows the stream's bitrate as the PC counts it");
        rates.result(&result(&request, 14_988, 120_000, Outcome::UNCHANGED, Outcome::APPLIED), 300);
        rates.menu_fps = 90.0;
        let next = rates.menu_change(400).expect("a rate");
        assert_eq!((next.bitrate_kbps, next.fps_millihz), (0, 90_000), "the encoder's bitrate is left alone");
        assert_eq!(rates.menu_kbps, 14_988);
    }

    #[test]
    fn a_reconnect_after_the_player_chose_a_bitrate_sends_that_bitrate_whatever_the_pc_answers() {
        // The player's 30000 came from this PC's own answers, so it is in the PC's unit already.
        let mut rates = RateChange::new(30_000, 60_000, false, 0, 0);
        rates.chosen_before(30_000);
        rates.keep_bitrate();
        let query = rates.query_request(0);
        rates.result(&answered(&query, 22_488), 100);
        assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(30_000));
    }

    #[test]
    fn keep_my_bitrate_turned_on_before_the_answer_sends_what_the_pc_answered() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        let query = rates.query_request(0);
        assert_eq!(rates.keep_now(500), None, "nothing before the answer");
        assert!(rates.kept() && !rates.hint_active());
        rates.result(&answered(&query, 14_988), 600);
        assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(14_988));
        assert!((0..30).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "this device lowers nothing");
    }

    #[test]
    fn keep_my_bitrate_is_stored_as_settings_stores_never_and_the_pc_decides() {
        let mut s = crate::config::Settings { lower_bitrate: LowerBitrate::WhenFramesLost, ..Default::default() };
        store_keep(&mut s, true);
        assert_eq!(s.lower_bitrate, LowerBitrate::Keep);
        store_keep(&mut s, false);
        assert_eq!(s.lower_bitrate, LowerBitrate::Off, "the default");
    }

    #[test]
    fn the_controller_waits_for_a_change_by_hand() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(0).unwrap();
        assert!((3..10).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "the menu's request is pending");
        rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 10_000);
        assert_eq!(rates.tick(&stats(12, 10.0)).map(|r| r.bitrate_kbps), Some(21_000));
    }

    #[test]
    fn a_request_that_cannot_be_sent_is_loud_and_stops_the_controller() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        let request = rates.tick(&stats(3, 10.0)).unwrap();
        assert_eq!(rates.send_failed(&request, &"closed"), "Loss-based bitrate stopped: closed");
        assert!(!rates.hint_active());
        rates.menu_kbps = 30_000;
        let request = rates.menu_request(0).unwrap();
        assert_eq!(rates.send_failed(&request, &"closed"), "Could not change the stream: closed");
        assert!(!rates.menu_busy());
    }

    #[test]
    fn a_request_the_host_never_answers_is_given_up_on_loudly() {
        let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
        rates.menu_kbps = 30_000;
        let manual = rates.menu_request(1000).unwrap();
        assert_eq!(rates.expire(5999), None);
        assert_eq!(rates.expire(6000).as_deref(), Some("The PC did not answer the rate change."));
        assert!(!rates.menu_busy(), "the menu takes a new request");
        assert_eq!(rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 7000), Answer::default(), "too late");
        let request = rates.tick(&stats(8, 10.0)).unwrap();
        assert_eq!(rates.expire(12_999), None);
        assert_eq!(rates.expire(13_000).as_deref(), Some("Loss-based bitrate stopped: the PC did not answer"));
        assert!(!rates.hint_active());
        assert_eq!(rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 13_100), Answer::default());
    }
}
