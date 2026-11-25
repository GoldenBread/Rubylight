//! VRR display-check pacing, tear-free sync modes, and upstream flip-latch guard.

use super::{after_display_check, PacingMode};

/// What a VRR stream's pacing does with the display check's verdict and `NIGHTJAR_VRR_SYNC`, a
/// step on each present (`Streaming::sync_vrr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VrrPacing {
    /// The display check's verdict turns a fixed refresh display to `fallback`.
    pub check: bool,
    pub fallback: PacingMode,
    /// The check changed the pacing.
    pub fell_back: bool,
    pub sync: VrrSync,
    /// The sync interval the renderer was last given.
    pub interval: i32,
    /// The pacing the check turned from, which a recheck that sees the display refresh on present
    /// returns to.
    pub(in crate::stream::present) restore: Option<PacingMode>,
    /// When the display is measured again while fallen back (µs on the session clock): None while
    /// a check runs, and for good once a stream that returned falls back again, so the schedule
    /// never flaps. A verdict the probe reaches by itself still returns the stream: on Linux a DRM
    /// reading after the window became fullscreen and focused again, one switch per such change.
    recheck_us: Option<u64>,
    recheck_every_us: u64,
    rechecks: bool,
    /// The stream returned from a fall back once.
    pub recovered: bool,
    /// The display check runs for the present mode alone (`stats::checks_display_for_present_mode`):
    /// its verdict changes no pacing.
    pub present_check: bool,
    /// The last verdict found the display at a fixed refresh, and none since found it refreshing
    /// on present.
    fixed_display: bool,
    /// The stream repeats the held picture for a VRR panel itself (`stream::lfc`), which reads the
    /// scan line too.
    pub lfc: bool,
    /// Refreshes each V-Sync picture stays on the display (fractional V-Sync,
    /// `nightjar_policy::vsync::refreshes_per_frame`). The stream holds each picture itself until
    /// the refresh that opens its turn, so the swap chain keeps sync interval 1; 1 shows each
    /// picture at the next refresh.
    pub refreshes: u32,
    /// `NIGHTJAR_VRR_SYNC` chose `sync`, which then stays as chosen (`distrust_scan`).
    pub sync_forced: bool,
    /// Tear-free VRR's wait turned to sync interval 1 on a display whose scan line does not say
    /// when it shows a picture (`distrust_scan`).
    pub distrusted: bool,
}

/// The first recheck after a "no VRR" fall back, and the longest between two: each one doubles
/// the wait. A recheck costs the render thread a scan line reading per present for a second or two.
pub const RECHECK_FIRST_US: u64 = 10_000_000;
pub const RECHECK_MAX_US: u64 = 160_000_000;

impl VrrPacing {
    /// `mode`: the pacing the stream starts with, whose sync interval the renderer has.
    pub fn new(check: bool, fallback: PacingMode, sync: VrrSync, mode: PacingMode) -> Self {
        Self {
            check, fallback, fell_back: false, sync, interval: sync_interval(mode, VrrSync::Off),
            restore: None, recheck_us: None, recheck_every_us: RECHECK_FIRST_US, rechecks: true, recovered: false,
            present_check: false, fixed_display: false, lfc: false, refreshes: 1, sync_forced: false, distrusted: false,
        }
    }

    /// Holds each V-Sync picture for `refreshes` refreshes (fractional V-Sync); the next `step`
    /// gives the renderer the new interval.
    #[cfg(test)]
    pub fn with_refreshes(self, refreshes: u32) -> Self {
        Self { refreshes: refreshes.clamp(1, nightjar_policy::vsync::MAX_REFRESHES), ..self }
    }

    /// The refreshes a picture is held for now: after a change of the display's refresh or the
    /// stream's rate. The next `step` gives the renderer the interval when it changed.
    pub fn set_refreshes(&mut self, refreshes: u32) {
        self.refreshes = refreshes.clamp(1, nightjar_policy::vsync::MAX_REFRESHES);
    }

    /// The refreshes a picture is held for in `mode`: `refreshes` with V-Sync, else 1 (VRR shows
    /// pictures as they come).
    pub fn held(&self, mode: PacingMode) -> u32 {
        if mode == PacingMode::Vsync { self.refreshes } else { 1 }
    }

    /// Has the stream repeat pictures for the panel (`stream::lfc`).
    pub fn with_lfc(self, lfc: bool) -> Self {
        Self { lfc, ..self }
    }

    /// Applies Whole frames under VRR (`config::VrrWait`) to Tear-free VRR's wait: Automatic leaves
    /// the turn to `distrust_scan`, Scan keeps the beam's wait and does not take that turn (a wait
    /// that would cost the next picture its turn still presents with sync interval 1,
    /// `scan_sync::scan_wait`), VSync presents with sync interval 1 from the start. A sync
    /// `NIGHTJAR_VRR_SYNC` set (`env_set`) stays as set.
    pub fn with_wait(mut self, wait: crate::config::VrrWait, env_set: bool) -> Self {
        use crate::config::VrrWait;
        if !env_set && wait == VrrWait::VSync { self.distrust_scan(); }
        self.sync_forced = env_set || wait != VrrWait::Automatic;
        self
    }

    /// Turns Tear-free VRR's wait for the beam (`VrrSync::Scan`) to sync interval 1 for the rest of
    /// the stream, on a display that does not always refresh when a picture reaches it
    /// (`scan_sync::UNTRUSTED_MISSES`): there a picture the wait sends into the blanking can still
    /// tear, and only the swap chain's own wait for the refresh shows it whole. A sync
    /// `NIGHTJAR_VRR_SYNC` chose stays. Returns whether it turned; the next `step` gives the
    /// renderer the interval.
    pub fn distrust_scan(&mut self) -> bool {
        if self.sync != VrrSync::Scan || self.sync_forced { return false; }
        self.sync = VrrSync::Present1;
        self.distrusted = true;
        true
    }

    /// Runs the display check for the present mode where it changes no pacing.
    pub fn with_present_check(self, present_check: bool) -> Self {
        Self { present_check, ..self }
    }

    /// Whether the display, as far as the check knows, refreshes on present: false once it fell
    /// back or a verdict found a fixed refresh. The present mode follows it (`prefers_immediate`).
    pub fn display_vrr(&self) -> bool {
        !(self.fell_back || self.fixed_display)
    }

    /// Folds the display check's verdict (`verdict`, None before the probe runs) into `mode`, and
    /// returns the sync interval to give the renderer when it is not the last one given. It is
    /// given once: a renderer that refuses it refuses it every frame. A stream that fell back
    /// returns to its pacing once a recheck finds the display refreshing on present, and plans the
    /// next recheck when one ends without (`now_us`, `recheck_due`).
    pub fn step(&mut self, mode: &mut PacingMode, verdict: Option<&crate::stream::scanline::Verdict>, now_us: u64) -> Option<i32> {
        if let (true, Some(verdict)) = (self.check || self.present_check, verdict) {
            if verdict.fixed_refresh() {
                self.fixed_display = true;
            } else if verdict.refreshes_on_present() {
                self.fixed_display = false;
            }
        }
        if let (true, Some(verdict)) = (self.check, verdict) {
            let after = after_display_check(*mode, verdict, self.fallback);
            if after != *mode {
                self.restore = Some(*mode);
                *mode = after;
                self.fell_back = true;
                // Back once and no VRR again: the display stays on the fall back.
                self.rechecks &= !self.recovered;
                self.recheck_us = None;
            } else if let (true, Some(restore)) = (self.fell_back, self.restore) {
                if verdict.refreshes_on_present() {
                    *mode = restore;
                    (self.fell_back, self.recovered, self.recheck_us) = (false, true, None);
                } else if *verdict == crate::stream::scanline::Verdict::Checking {
                    // A recheck under way: the next is planned from its verdict.
                    self.recheck_us = None;
                } else if self.rechecks && self.recheck_us.is_none() {
                    self.recheck_us = Some(now_us + self.recheck_every_us);
                }
            }
        }
        let interval = sync_interval(*mode, self.sync);
        (interval != self.interval).then(|| {
            self.interval = interval;
            interval
        })
    }

    /// Whether the display check takes the presents as tearing ones: the pacing tears, or the
    /// check itself turned it to V-Sync and a recheck has to see what the stream had before. Not
    /// once Tear-free VRR turned to sync interval 1 (`distrust_scan`): its presents tear no more,
    /// and a late start of their scans is no sign of a fixed refresh.
    pub fn probe_tears(&self, mode: PacingMode) -> bool { (mode != PacingMode::Vsync && !self.distrusted) || self.fell_back }

    /// Whether the display check is to measure the display again now (`RaceProbe::recheck`): a
    /// stream on its fall back whose recheck time has come. The next one waits twice as long.
    pub fn recheck_due(&mut self, now_us: u64) -> bool {
        if !self.recheck_us.is_some_and(|at| now_us >= at) { return false; }
        self.recheck_us = None;
        self.recheck_every_us = (self.recheck_every_us * 2).min(RECHECK_MAX_US);
        true
    }
}

/// How a VRR stream presents while the display may still be scanning the last picture
/// (`NIGHTJAR_VRR_SYNC`, a measurement switch). A tearing present then shows the new picture from
/// the beam down: 6-20% of the presents of a 60 fps stream on a 240 Hz VRR panel did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrrSync {
    /// Present on arrival, with tearing.
    #[default]
    Off,
    /// Sync interval 1: the flip is immediate on an idle display and waits out a scan under way.
    Present1,
    /// Wait out a scan under way on this thread (the beam from the scan line, at most one scan),
    /// then present with tearing. When that wait's landing would pass the next picture's due
    /// time, present with sync interval 1 instead (`scan_sync::scan_wait`), shown whole at the
    /// next refresh.
    Scan,
    /// Tears only when the display is in its vertical blank and the present before it has left the
    /// queue, as read just before the present; any other present goes out with sync interval 1 and
    /// is shown whole after the scan under way.
    Blank,
}

impl VrrSync {
    /// `NIGHTJAR_VRR_SYNC`'s value; anything else is said on stderr and leaves it off.
    pub fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("" | "off" | "0") => Self::Off,
            Some("present1") => Self::Present1,
            Some("scan") => Self::Scan,
            Some("blank") => Self::Blank,
            Some(other) => {
                eprintln!("nightjar: NIGHTJAR_VRR_SYNC={other:?} is not present1, scan, blank or off, so it is off");
                Self::Off
            }
        }
    }

    /// A VRR stream's sync: what `NIGHTJAR_VRR_SYNC` says when it is set (`off` shows every frame
    /// at once), else Tear-free VRR (`vrr_tear_free`), which waits out a scan under way where the
    /// scan line can be read, on Windows.
    pub fn choose(env: Option<&str>, tear_free: bool, windows: bool) -> Self {
        match env.map(str::trim) {
            None | Some("") => if tear_free && windows { Self::Scan } else { Self::Off },
            text => Self::parse(text),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Present1 => "present1",
            Self::Scan => "scan",
            Self::Blank => "blank",
        }
    }
}

/// What the swap chain and the scan line said about the last present, read just before the next
/// one. Mirrors upstream `D3D11PresentPolicy::FlipObservation`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlipObservation {
    pub predecessor_known: bool,
    pub statistics_valid: bool,
    pub pending: bool,
    pub raster_valid: bool,
    pub in_vertical_blank: bool,
    pub raster_query_start_us: u64,
    pub raster_query_end_us: u64,
}

/// Whether a present is latched (sync interval 1). Literal port of upstream
/// `D3D11PresentPolicy::latch` from Nonary's moonlight-qt (GPL-3.0, commit 19758e5e6),
/// app/streaming/video/ffmpeg-renderers/d3d11presentpolicy.h, without its `synchronizeAll`
/// argument. A retired predecessor can still be scanning out. Unknown or active scanout must not
/// authorize a tearing flip. A fresh blank is a risk filter, not a reservation of the phase when a
/// later asynchronous flip reaches the panel.
pub fn latch(planned_latched: bool, protection_window_us: u64, observation: impl FnOnce() -> FlipObservation, clock: impl FnOnce() -> u64) -> bool {
    if planned_latched { return true; }
    if protection_window_us == 0 { return false; } // Explicitly disabled protection.
    let observation = observation();
    if !observation.predecessor_known || !observation.statistics_valid || observation.pending
        || !observation.raster_valid || !observation.in_vertical_blank { return true; }
    let now_us = clock();
    // Bound the entire observation age, including any preemption after the query. This
    // conservative budget does not claim a remaining blank time.
    let precision_us = (protection_window_us / 16).min(250);
    precision_us == 0 || observation.raster_query_start_us == 0
        || observation.raster_query_end_us < observation.raster_query_start_us
        || now_us < observation.raster_query_end_us
        || now_us - observation.raster_query_start_us > precision_us
}

/// Upstream `vrrFloorLatchGapUs` (vrrtimingcontroller.cpp, commit 19758e5e6).
pub const FLOOR_LATCH_GAP_US: u64 = 20_000;
/// Upstream `latchedPresentationHeadroomUs` default (vrrtimingcontroller.h, commit 19758e5e6).
pub const LATCH_HEADROOM_US: u64 = 225;

/// Whether the planned flip latches (sync interval 1), from the time since the last flip. Literal
/// port of upstream `vrrtimingcontroller.cpp` (commit 19758e5e6), where the latch is decided before
/// the present. A frame closer to the last flip than one display period plus headroom is latched:
/// the scan of the picture before cannot be over. After a gap of `FLOOR_LATCH_GAP_US` or more,
/// past the VRR range, the driver may be repeating the last picture, so the flip queue places this
/// one too.
pub fn planned_latch(since_last_flip_us: Option<u64>, display_period_us: u64) -> bool {
    match since_last_flip_us {
        None => false,
        Some(since) => since < display_period_us.saturating_add(LATCH_HEADROOM_US) || since >= FLOOR_LATCH_GAP_US,
    }
}

/// The flip history of a stream's presents, for `VrrSync::Blank`.
#[derive(Default)]
pub struct FlipGuard {
    /// When the last present was issued (us), whatever its kind.
    pub last_flip_us: Option<u64>,
    /// The last present went out with sync interval 1 (`scan_sync::ScanWait::Latch`): its scan
    /// starts at the next refresh, so the time until it does is the latch's own, not a tearing
    /// flip's landing (`scan_sync::landing_ns`).
    pub last_flip_latched: bool,
    /// Presents that went out with tearing.
    pub tearing: u64,
    /// Presents latched by `planned_latch`.
    pub planned: u64,
    /// Presents latched by the observation.
    pub protected: u64,
}

/// The swap chain's sync interval for a pacing mode: 1 waits for the refresh, 0 presents at once
/// with tearing. Racing slices needs tearing whatever `sync` says. On macOS Real VRR presents
/// synchronised: a Metal layer shows a synchronised present on a display with a refresh range
/// when it is issued, and Nonary's moonlight-qt keeps `displaySyncEnabled` on under VRR for it
/// (GPL-3.0, commit 19758e5e6, vt_metal.mm).
pub fn sync_interval(mode: PacingMode, sync: VrrSync) -> i32 {
    match (mode, sync) {
        (PacingMode::Vsync, _) => 1,
        (PacingMode::HostTimes, _) if cfg!(target_os = "macos") => 1,
        (PacingMode::Immediate | PacingMode::HostTimes, VrrSync::Present1) => 1,
        _ => 0,
    }
}
