//! Packed 4:4:4 chroma mode capability probing, settings-save reprobing, and user-session
//! retry handling.

use super::codec_probe::{PackedProbe, STANDARD_CODECS};
use super::*;

/// The first wait before a packed 4:4:4 probe that found the signed-in
/// user's session not ready runs again; each further one doubles, to 12x.
pub(super) const PACKED_SESSION_RETRY: Duration =
    Duration::from_millis(if cfg!(any(test, feature = "test-support")) {
        20
    } else {
        5000
    });

/// Packed 4:4:4 probes in a row that may find the session not ready.
pub(super) const PACKED_SESSION_TRIES: u8 = 6;

pub(super) const PACKED_MODES: [(u8, bool); 4] = [(1, false), (1, true), (2, false), (2, true)];

impl Host {
    /// The standard codecs are probed: launches go on at once, and a stream that starts from
    /// here on leaves the optional probes for after it, so the pass completes. The packed
    /// 4:4:4 probe then runs on, and `/serverinfo` waits for it (`startup_packed`). A launch
    /// that starts meanwhile has the encoder: the probe yields to it and runs again once the
    /// stream has ended, as a retry pass does, and not at the same time (an AMD encoder hang).
    pub(super) fn publish_standard_codecs(&self, config: &Config, flags: u32, probe: PackedProbe) {
        self.startup_codecs.send_replace(true);
        let streaming = || !self.sessions.lock().unwrap().idle();
        self.pass_packed_probe(config, flags, &streaming, probe);
        self.startup_packed.send_replace(true);
    }

    /// A codec pass's packed 4:4:4 probe of every mode `flags` allows. The
    /// modes a stream stopped, or that need a signed-in user's session, as
    /// when the host starts with Windows, stay pending: the pass probes them
    /// again once no stream runs and a user is signed in.
    pub(super) fn pass_packed_probe(
        &self,
        config: &Config,
        flags: u32,
        busy: &dyn Fn() -> bool,
        probe: PackedProbe,
    ) {
        use std::sync::atomic::Ordering;
        let _probe = self.optional_probe.lock().unwrap();
        let all = rubylight_protocol::packed444::ALL_MODES;
        let live = || packed_modes_allowed(&self.config.read().unwrap());
        match probe_packed_modes(config, flags, all, busy, &live, probe) {
            Some((found, waiting)) => {
                self.publish_packed_modes(found, all & !waiting);
                self.wait_for_packed_session(waiting);
            }
            None => {
                self.packed_probe_pending.fetch_or(all, Ordering::AcqRel);
                self.publish_packed_modes(0, 0);
            }
        }
    }

    /// Leave the packed 4:4:4 modes in `waiting`, whose probe found the
    /// signed-in user's session not ready, pending for a probe once it is.
    /// After `PACKED_SESSION_TRIES` such probes in a row they fail like any
    /// other error, so a helper that never starts is not started forever.
    pub(super) fn wait_for_packed_session(&self, waiting: u8) {
        use std::sync::atomic::Ordering;
        if waiting == 0 {
            self.packed_session_waits.store(0, Ordering::Release);
            return;
        }
        let waits = self.packed_session_waits.fetch_add(1, Ordering::AcqRel) + 1;
        if waits < PACKED_SESSION_TRIES {
            tracing::info!(
                modes = packed_mode_names(waiting),
                tries = waits,
                "packed 4:4:4 probe waits for a signed-in user; it runs again then"
            );
            self.packed_probe_pending
                .fetch_or(waiting, Ordering::AcqRel);
            return;
        }
        self.packed_session_waits.store(0, Ordering::Release);
        tracing::warn!(
            modes = packed_mode_names(waiting),
            tries = waits,
            "packed 4:4:4 probe failed; clients are not offered this mode: the signed-in user's session never became ready"
        );
        self.publish_packed_modes(0, waiting);
    }

    /// A save changed the settings from `before`. The packed 4:4:4 modes they
    /// no longer allow are withdrawn at once, and modes they newly allow are
    /// probed once no stream runs or starts. Until then the startup probe was
    /// the only one, so turning full chroma on took a host restart.
    pub fn packed_settings_saved(self: &Arc<Self>, before: &Config) {
        self.packed_settings_saved_with(before, crate::win_bridge::probe_packed);
    }

    pub fn packed_settings_saved_with(self: &Arc<Self>, before: &Config, probe: PackedProbe) {
        use std::sync::atomic::Ordering;
        // The settings alone, not the codecs probed so far: a save during the
        // startup probe must still ask for a probe.
        let was = packed_modes_allowed(before);
        let now = packed_modes_allowed(&self.config.read().unwrap());
        if was == now {
            return;
        }
        let offered = {
            let _publish = self.packed_publish.lock().unwrap();
            self.packed_modes.fetch_and(now, Ordering::AcqRel) & now
        };
        // Only the modes the save newly allows are probed: the others keep
        // what their last probe found. During the startup probe the codecs
        // are not known yet, and the probe after it checks them. With no
        // standard codec found yet, the codec retry's next pass probes them.
        let mut new = now & !was;
        let codecs = self.codecs.load(Ordering::Acquire);
        if !self.probing_codecs.load(Ordering::Acquire) {
            if codecs & STANDARD_CODECS == 0 {
                new = 0;
            }
            let offerable = packed_modes_offerable(&self.config.read().unwrap(), codecs);
            self.warn_packed_codec_missing(new & !offerable);
            new &= offerable;
        }
        if new == 0 {
            tracing::info!(
                modes = packed_mode_names(offered),
                "packed 4:4:4 modes offered to clients"
            );
            return;
        }
        self.packed_probe_pending.fetch_or(new, Ordering::AcqRel);
        self.start_packed_probe(probe);
    }

    /// Probe the pending packed 4:4:4 modes on a thread once no stream runs,
    /// unless one already waits or probes.
    pub(super) fn start_packed_probe(self: &Arc<Self>, probe: PackedProbe) {
        use std::sync::atomic::Ordering;
        if self.packed_reprobing.swap(true, Ordering::AcqRel) {
            return;
        }
        let h = self.clone();
        std::thread::spawn(move || {
            loop {
                while h.packed_probe_pending.load(Ordering::Acquire) != 0 {
                    // A session not ready yet after sign-in gets longer each try.
                    let waits = h.packed_session_waits.load(Ordering::Acquire);
                    let delay = match waits {
                        0 => Duration::ZERO,
                        n => (PACKED_SESSION_RETRY * (1 << (n - 1).min(4)))
                            .min(PACKED_SESSION_RETRY * 12),
                    };
                    if !h.wait_for_session(delay, "packed 4:4:4") {
                        h.packed_reprobing.store(false, Ordering::Release);
                        return;
                    }
                    h.reprobe_packed_modes(probe);
                }
                h.packed_reprobing.store(false, Ordering::Release);
                // A save between the last check and the store found this
                // thread still running and left its modes to it.
                if h.packed_probe_pending.load(Ordering::Acquire) == 0
                    || h.packed_reprobing.swap(true, Ordering::AcqRel)
                {
                    return;
                }
            }
        });
    }

    /// Warn once about each packed 4:4:4 mode in `modes`, which the settings
    /// allow but whose codec the startup probe did not find.
    fn warn_packed_codec_missing(&self, modes: u8) {
        use std::sync::atomic::Ordering;
        let new = modes & !self.packed_codec_missing.fetch_or(modes, Ordering::AcqRel);
        for (codec, ten_bit) in PACKED_MODES {
            let bit = rubylight_protocol::packed444::mode_bit(codec, ten_bit);
            if new & bit != 0 {
                let codec = if codec == 1 { "HEVC" } else { "AV1" };
                tracing::warn!(
                    mode = packed_mode_names(bit),
                    "packed 4:4:4 mode not offered: {codec} not found by the startup probe; restart the host"
                );
            }
        }
    }

    /// Probe the pending packed 4:4:4 modes the saved settings still allow
    /// and publish them. A stream that starts ends the probe and leaves its
    /// modes pending for after.
    pub(super) fn reprobe_packed_modes(&self, probe: PackedProbe) -> bool {
        use std::sync::atomic::Ordering;
        let _probe = self.optional_probe.lock().unwrap();
        let pending = self.packed_probe_pending.swap(0, Ordering::AcqRel);
        let mut config = self.config.read().unwrap().clone();
        config.values.remove("amd_ltr_frames");
        let busy = || !self.sessions.lock().unwrap().idle();
        let live = || packed_modes_allowed(&self.config.read().unwrap());
        let codecs = self.codecs.load(Ordering::Acquire);
        let offerable = packed_modes_offerable(&config, codecs);
        // With no standard codec found, the codec retry's next pass probes
        // the packed modes as well: nothing is missing yet.
        if codecs & STANDARD_CODECS != 0 {
            self.warn_packed_codec_missing(pending & packed_modes_allowed(&config) & !offerable);
        }
        let modes = pending & offerable;
        if modes == 0 {
            return true;
        }
        match probe_packed_modes(&config, codecs, modes, &busy, &live, probe) {
            Some((found, waiting)) => {
                self.publish_packed_modes(found, modes & !waiting);
                self.wait_for_packed_session(waiting);
                true
            }
            None => {
                self.packed_probe_pending.fetch_or(modes, Ordering::AcqRel);
                tracing::info!(
                    "packed 4:4:4 probe stopped for a stream; it runs again once no stream runs"
                );
                false
            }
        }
    }

    /// Runs a per-ANNOUNCE packed 4:4:4 probe serialized against background
    /// optional probes and concurrent ANNOUNCEs (`optional_probe`), and skips
    /// hardware probing while another video stream is active to avoid
    /// concurrent AMD AMF encoder sessions.
    pub(crate) fn probe_packed_for_announce(
        &self,
        config: &Config,
        negotiated: &butterpollo_media_policy::rtsp::Negotiated,
        probe: impl FnOnce(
            &Config,
            &butterpollo_media_policy::rtsp::Negotiated,
            &dyn Fn() -> bool,
        ) -> Result<()>,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;
        let _probe = self.optional_probe.lock().unwrap();
        let active_video = || {
            self.sessions.lock().unwrap().active.values().any(|s| {
                !s.stopping() && s.launch.role != butterpollo_session::session::Role::InputOnly
            })
        };
        if active_video() {
            tracing::info!(
                width = negotiated.width,
                height = negotiated.height,
                fps = negotiated.fps,
                codec = negotiated.codec,
                "skipping packed 4:4:4 ANNOUNCE hardware probe while another video stream is active"
            );
            return Ok(());
        }
        probe(config, negotiated, &|| {
            self.stop.load(Ordering::Acquire) || active_video()
        })
    }

    /// Offer what a probe of the `probed` modes found, beside the modes it
    /// did not probe, narrowed to the modes the settings saved by now allow:
    /// a save during the probe that turned a mode off withdrew it, and one
    /// that turned a mode on asked for another probe. Packed modes of a codec
    /// clients are not offered stay off.
    pub(super) fn publish_packed_modes(&self, found: u8, probed: u8) -> u8 {
        use std::sync::atomic::Ordering;
        let _publish = self.packed_publish.lock().unwrap();
        let config = self.config.read().unwrap();
        let offered = (self.packed_modes.load(Ordering::Acquire) & !probed | found & probed)
            & packed_modes_offerable(&config, self.codecs.load(Ordering::Acquire));
        self.packed_modes.store(offered, Ordering::Release);
        if config.packed_444() {
            tracing::info!(
                modes = packed_mode_names(offered),
                "packed 4:4:4 modes offered to clients"
            );
        }
        offered
    }
}

/// The packed 4:4:4 modes the settings let clients be offered: none while
/// full chroma is off, and none for a codec or 10-bit mode turned off.
pub(super) fn packed_modes_allowed(config: &Config) -> u8 {
    if !config.packed_444() {
        return 0;
    }
    PACKED_MODES
        .into_iter()
        .filter(|&(codec, ten_bit)| {
            let mode = config.integer(if codec == 1 { "hevc_mode" } else { "av1_mode" }, 0);
            !(mode == 1 || (ten_bit && mode == 2))
        })
        .fold(0, |modes, (codec, ten_bit)| {
            modes | rubylight_protocol::packed444::mode_bit(codec, ten_bit)
        })
}

/// `packed_modes_allowed`, less the modes of a codec the startup probe did
/// not find in `codecs` (`ServerCodecModeSupport`): turning HEVC or AV1 on
/// takes a restart, and a packed mode of a codec clients are not offered
/// is never chosen.
pub(super) fn packed_modes_offerable(config: &Config, codecs: u32) -> u8 {
    PACKED_MODES
        .into_iter()
        .filter(|&(codec, _)| codecs & if codec == 1 { 0x100 } else { 0x10000 } != 0)
        .fold(0, |modes, (codec, ten_bit)| {
            modes | rubylight_protocol::packed444::mode_bit(codec, ten_bit)
        })
        & packed_modes_allowed(config)
}

/// Probe each packed 4:4:4 mode in `modes` the settings and `codecs` allow,
/// warning about each that fails. Returns the modes that passed and those
/// whose probe found the signed-in user's session not ready, which are worth
/// probing again. None once `busy` says a stream wants the encoder, which
/// also stops a probe that runs. A mode the settings saved by now (`live`) no
/// longer allow is skipped, or stopped while it is probed.
pub(super) fn probe_packed_modes(
    config: &Config,
    codecs: u32,
    modes: u8,
    busy: &dyn Fn() -> bool,
    live: &dyn Fn() -> u8,
    probe: PackedProbe,
) -> Option<(u8, u8)> {
    let allowed = modes & packed_modes_offerable(config, codecs);
    let mut packed_modes = 0;
    let mut waiting = 0;
    for (codec, ten_bit) in PACKED_MODES {
        let bit = rubylight_protocol::packed444::mode_bit(codec, ten_bit);
        if allowed & bit == 0 || live() & bit == 0 {
            continue;
        }
        if busy() {
            return None;
        }
        let negotiated = butterpollo_media_policy::rtsp::Negotiated {
            width: 640,
            height: 480,
            fps: 30,
            bitrate_kbps: 2000,
            codec,
            sdr_10bit: ten_bit,
            chroma_packing: rubylight_protocol::packed444::VERSION,
            ..Default::default()
        };
        let probing = std::time::Instant::now();
        match probe(config, &negotiated, &|| busy() || live() & bit == 0) {
            Ok(()) => packed_modes |= bit,
            Err(error) if error.is::<butterpollo_win_graphics::codec_probe::Interrupted>() => {
                if live() & bit != 0 {
                    return None;
                }
                tracing::info!(
                    mode = packed_mode_names(bit),
                    "packed 4:4:4 probe stopped: the mode was turned off"
                );
            }
            Err(error) if error.is::<butterpollo_win_graphics::codec_probe::SessionNotReady>() => {
                tracing::debug!(
                    error = %format!("{error:#}"),
                    mode = packed_mode_names(bit),
                    "packed 4:4:4 probe needs a signed-in user"
                );
                waiting |= bit;
            }
            // Packed 4:4:4 is opt-in: a mode the user turned on but clients
            // are not offered must show in the log.
            Err(error) => tracing::warn!(
                error = %format!("{error:#}"),
                mode = packed_mode_names(bit),
                elapsed_ms = probing.elapsed().as_millis() as u64,
                "packed 4:4:4 probe failed; clients are not offered this mode"
            ),
        }
    }
    Some((packed_modes, waiting))
}

/// Names of the packed 4:4:4 modes in a `packed444Modes` mask.
pub(super) fn packed_mode_names(modes: u8) -> String {
    let names: Vec<_> = [
        (1, false, "HEVC 8-bit"),
        (1, true, "HEVC 10-bit"),
        (2, false, "AV1 8-bit"),
        (2, true, "AV1 10-bit"),
    ]
    .into_iter()
    .filter(|&(codec, ten_bit, _)| {
        modes & rubylight_protocol::packed444::mode_bit(codec, ten_bit) != 0
    })
    .map(|(_, _, name)| name)
    .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_session::session::Role;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn announce_packed_probe_serializes_with_optional_probe_and_skips_when_video_active() {
        let f = Fixture::new();
        let config = Config::default();
        let negotiated = butterpollo_media_policy::rtsp::Negotiated::default();

        // While an active video stream runs, hardware probing is skipped to avoid AMD hangs.
        let active = f.session(f.client(u32::MAX), Role::Stream);
        let called = AtomicUsize::new(0);
        f.host
            .probe_packed_for_announce(&config, &negotiated, |_, _, _| {
                called.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();
        assert_eq!(called.load(Ordering::Relaxed), 0);
        f.host
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&active.launch.id);

        // An InputOnly session has no video encoder, so probing still runs.
        let _input_only = f.session(f.client(u32::MAX), Role::InputOnly);
        // Holding `optional_probe` blocks the announce probe until released.
        let guard = f.host.optional_probe.lock().unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let host = f.host.clone();
        let entered_bg = entered.clone();
        let handle = std::thread::spawn(move || {
            host.probe_packed_for_announce(&Config::default(), &Default::default(), |_, _, _| {
                entered_bg.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        });
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            !entered.load(Ordering::Acquire),
            "ANNOUNCE packed probe must wait for optional_probe lock"
        );
        drop(guard);
        handle.join().unwrap();
        assert!(entered.load(Ordering::Acquire));
    }
}
