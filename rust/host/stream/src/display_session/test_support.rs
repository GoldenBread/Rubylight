//! Test support helpers for [`super`] display preparation and retained virtual displays.
use super::*;

thread_local! {
    /// Makes this thread's [`displays_settling`] answer true.
    pub static DISPLAYS_SETTLING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A game display kept between streams, with no other holder, for tests
/// of the session state and the grace period.
pub fn retained_display(host: &Shared, config: &Config) -> Arc<Ready> {
    let prepared = test_prepared(host);
    let limiter = limiter::Lease::acquire(&host.directory, config, &prepared.framegen).unwrap();
    Ready::prepare((prepared, limiter)).unwrap().display.clone()
}

/// A kept display that is the virtual display `identity`, joined or a stream's own.
pub fn retained_on(host: &Shared, config: &Config, identity: &str, joined: bool) -> Arc<Ready> {
    let mut prepared = test_prepared(host);
    prepared.identity = Some(identity.into());
    prepared.joined = joined;
    let limiter = limiter::Lease::acquire(&host.directory, config, &prepared.framegen).unwrap();
    Ready::prepare((prepared, limiter)).unwrap().display.clone()
}

/// A kept display of an exclusive layout, whose grace is short whatever the settings say.
pub fn retained_exclusive_display(host: &Shared, config: &Config) -> Arc<Ready> {
    let prepared = test_prepared(host);
    let limiter = limiter::Lease::acquire(&host.directory, config, &prepared.framegen).unwrap();
    let mut stream = Ready::prepare((prepared, limiter)).unwrap();
    Arc::get_mut(&mut stream.display).unwrap().exclusive_layout = true;
    stream.display.clone()
}

pub(crate) fn test_prepared(host: &Shared) -> Prepared {
    let framegen = Policy::resolve(
        &Config::default(),
        Rate(60_000),
        false,
        "none",
        false,
        false,
    )
    .unwrap();
    Prepared {
        display: None,
        output: String::new(),
        framegen,
        _arrangement: None,
        _activation: None,
        _profile: None,
        _retained: None,
        _vulkan: None,
        _golden: None,
        mode: stream_mode(&Negotiated::default()),
        client_display: None,
        virtual_layout: Some("extended".into()),
        layout_owner: None,
        inherited_layout: Some("extended".into()),
        resolution_owner: None,
        scale_baseline: None,
        follow: None,
        identity: None,
        joined: false,
        #[cfg(any(test, feature = "test-support"))]
        layout_snapshot: None,
        #[cfg(test)]
        size_snapshot: None,
        #[cfg(test)]
        scale_snapshot: None,
        #[cfg(test)]
        scales_set: Mutex::new(Vec::new()),
        revision: 0,
        recovery: Recovery::new(),
        layout_watch: Default::default(),
        primary_watch: Default::default(),
        departures: 0,
        wake_layouts: 0,
        console_session: None,
        secure_desktop: false,
        displays_seen: None,
        shown_primary: None,
        paused_change: None,
        start_primary_mismatch: false,
        hold_layout_while_paused: false,
        mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
        recovery_scale: 0,
        recovery_dimensions: (640, 480),
        recovery_profile: None,
        virtual_mode: false,
        profile_choice: None,
        host: Arc::downgrade(host),
    }
}
