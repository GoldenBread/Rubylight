//! Tests for shared virtual display leases, rate raises, reshaping, and mode listing.

use super::super::*;
use std::sync::{Arc, Mutex};

/// A shared virtual display as Windows treats it.
#[derive(Debug)]
struct Fake {
    created: Timing,
    mode: Timing,
    /// What Windows shows.
    shown: std::cell::Cell<Timing>,
    /// Off, as a paused display can be, refuses every mode.
    on: bool,
    /// Refused refreshes, after which Windows shows 60 Hz.
    refuses: Vec<u32>,
    /// The modes Windows lists, and so can set.
    listed: std::cell::RefCell<Vec<Timing>>,
    /// Offered modes Windows does not list yet.
    pending: std::cell::RefCell<Vec<Timing>>,
    /// The look at the display's modes after an offer in which Windows
    /// lists the offered modes; None never does.
    lists_after: Option<u32>,
    /// Looks at the display's modes since the last offer.
    looks: std::cell::Cell<u32>,
    /// The fastest refresh Windows lists an offered mode at, as on a PC
    /// that lists 4K only up to 240 Hz.
    fastest: u32,
    /// The driver refuses to offer a mode.
    driver_refuses: bool,
    offered: Vec<Timing>,
    log: std::cell::RefCell<Vec<String>>,
}
impl Fake {
    fn new(mode: Timing) -> Self {
        Self {
            created: mode,
            mode,
            shown: mode.into(),
            on: true,
            refuses: Vec::new(),
            listed: vec![mode].into(),
            pending: Default::default(),
            lists_after: Some(1),
            looks: 0.into(),
            fastest: u32::MAX,
            driver_refuses: false,
            offered: vec![mode],
            log: Default::default(),
        }
    }
    /// The refreshes (whole hertz) Windows lists at the mode's size.
    fn rates(&self) -> Vec<u32> {
        self.listed
            .borrow()
            .iter()
            .filter(|listed| (listed.0, listed.1) == (self.mode.0, self.mode.1))
            .map(|listed| listed.2 / 1000)
            .collect()
    }
}
impl SharedDisplay for Fake {
    fn modes(&self) -> (Timing, Timing) {
        (self.created, self.mode)
    }
    fn set_mode(&mut self, mode: Timing) {
        self.mode = mode;
    }
    fn offer_mode(&self) -> bool {
        self.log.borrow_mut().push(format!("offer {}", self.mode.2));
        if self.driver_refuses {
            return false;
        }
        self.pending.borrow_mut().push(self.mode);
        self.looks.set(0);
        true
    }
    fn offered(&mut self) -> &mut Vec<Timing> {
        &mut self.offered
    }
    fn listed_rates(&self) -> Vec<u32> {
        self.looks.set(self.looks.get() + 1);
        if self
            .lists_after
            .is_some_and(|after| self.looks.get() >= after)
        {
            let offered: Vec<_> = self.pending.borrow_mut().drain(..).collect();
            self.listed.borrow_mut().extend(
                offered
                    .into_iter()
                    .map(|(width, height, rate)| (width, height, rate.min(self.fastest))),
            );
        }
        self.rates()
    }
    fn apply(&self, stage: &str) -> bool {
        self.log.borrow_mut().push(stage.into());
        let (width, height, rate) = self.mode;
        if !self.on || self.refuses.contains(&rate) {
            self.shown.set((width, height, 60_000));
            return false;
        }
        if self.listed.borrow().contains(&self.mode) {
            self.shown.set(self.mode);
            return true;
        }
        // A rate Windows does not list is a bad mode: apply_mode keeps
        // the size at a slower listed rate, else Windows keeps its mode.
        if let Some(fallback) = crate::display::modes::fallback_rate(&self.rates(), rate) {
            self.shown.set((width, height, fallback));
        }
        false
    }
    fn applied_size(&self) -> Option<(u32, u32)> {
        let shown = self.shown.get();
        ((shown.0, shown.1) == (self.mode.0, self.mode.1)).then_some((shown.0, shown.1))
    }
    fn switch_on(&mut self) -> anyhow::Result<crate::display::Monitor> {
        self.on = true;
        self.log.borrow_mut().push("on".into());
        Ok(crate::display::monitor("virtual", 1, false))
    }
}
/// Lease `id` for a stream at `requested`, creating `fresh` when nothing has it.
fn lease(
    displays: &mut crate::display::virtual_display::Leases<Fake>,
    requested: Timing,
    fresh: Option<Fake>,
) -> anyhow::Result<(Arc<Mutex<Fake>>, Option<u32>)> {
    crate::display::virtual_display::lease(displays, "shared", requested, || {
        Ok(fresh.expect("a display was created"))
    })
}
/// Lease and switch on, as a stream's display guard does.
fn stream(
    displays: &mut crate::display::virtual_display::Leases<Fake>,
    requested: Timing,
    fresh: Option<Fake>,
) -> anyhow::Result<Arc<Mutex<Fake>>> {
    let (display, needed) = lease(displays, requested, fresh)?;
    crate::display::virtual_display::joined_monitor(&mut *display.lock().unwrap(), needed)?;
    Ok(display)
}

#[test]
fn a_stream_beside_the_game_takes_its_display_as_it_is_at_any_size_or_refresh() {
    let owner = (1920, 1080, 60_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, owner, Some(Fake::new(owner))).unwrap();
    let asked = (1280, 720, 120_000);
    let no_display = || -> anyhow::Result<Fake> { panic!("a display was created") };
    // Asked for as a second stream, this size is refused and this refresh raises.
    assert!(
        crate::display::virtual_display::lease_or_join(
            &mut displays,
            "shared",
            false,
            asked,
            no_display
        )
        .is_err()
    );
    assert_eq!(
        crate::display::virtual_display::joined_lease(&displays, "shared")
            .unwrap()
            .1,
        owner
    );
    let (joined, needed) = crate::display::virtual_display::lease_or_join(
        &mut displays,
        "shared",
        true,
        asked,
        no_display,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&first, &joined));
    first.lock().unwrap().log.borrow_mut().clear();
    // As a guard does with what a join's lease returns.
    crate::display::virtual_display::joined_monitor(&mut *joined.lock().unwrap(), needed).unwrap();
    let display = first.lock().unwrap();
    assert_eq!(display.modes(), (owner, owner));
    assert_eq!(*display.log.borrow(), ["on"]);
    drop(display);
    // Nothing to join once its last stream is gone: the stream makes its own.
    drop((first, joined));
    assert!(crate::display::virtual_display::joined_lease(&displays, "shared").is_none());
    let (own, _) = crate::display::virtual_display::lease_or_join(
        &mut displays,
        "shared",
        true,
        asked,
        || Ok(Fake::new(asked)),
    )
    .unwrap();
    assert_eq!(own.lock().unwrap().modes(), (asked, asked));
}

#[test]
fn a_second_stream_joins_a_shared_display_whose_mode_changed_in_place() {
    let launched = (1920, 1080, 60_000);
    // The first stream raised the rate, turned, or Windows took a size
    // the user picked: the launch mode and the shown one both join.
    for current in [
        (1920, 1080, 120_000),
        (1080, 1920, 60_000),
        (2560, 1440, 60_000),
    ] {
        assert_eq!(
            crate::display::virtual_display::join(launched, current, launched).unwrap(),
            None
        );
        assert_eq!(
            crate::display::virtual_display::join(launched, current, current).unwrap(),
            None
        );
        let error = crate::display::virtual_display::join(launched, current, (1280, 720, 60_000))
            .unwrap_err()
            .to_string();
        assert!(error.contains("asks for 1280x720"), "{error}");
    }
    assert_eq!(
        crate::display::virtual_display::join(launched, launched, launched).unwrap(),
        None
    );
}

#[test]
fn a_vrr_stream_and_a_fixed_rate_stream_share_a_virtual_display_either_way() {
    // Nightjar desktop with VRR asks 1000 Hz, a phone at 60 fps 120 Hz.
    let vrr = (1920, 1080, 1_000_000);
    let fixed = (1920, 1080, 120_000);
    // VRR first: its 1000 Hz serves the 60 fps stream as it is.
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, vrr, Some(Fake::new(vrr))).unwrap();
    let second = stream(&mut displays, fixed, None).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.lock().unwrap().shown.get(), vrr);
    // Only switched on, by each stream: no mode was set.
    assert_eq!(*first.lock().unwrap().log.borrow(), ["on", "on"]);
    // The fixed rate first: the VRR stream raises the display, and keeps VRR.
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    let second = stream(&mut displays, vrr, None).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.lock().unwrap().mode, vrr);
    assert_eq!(first.lock().unwrap().shown.get(), vrr);
    // Now at 1000 Hz, a third stream at either rate joins as it is.
    assert_eq!(lease(&mut displays, fixed, None).unwrap().1, None);
    assert_eq!(lease(&mut displays, vrr, None).unwrap().1, None);
    // Once every stream left, the next one gets a display of its own.
    drop((first, second));
    let fresh = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    assert_eq!(fresh.lock().unwrap().mode, fixed);
}

#[test]
fn a_shared_virtual_display_that_cannot_serve_a_stream_refuses_it_with_why() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    // Another size, even at a rate the display serves.
    let error = lease(&mut displays, (2560, 1440, 60_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it is 1920x1080, and this stream asks for 2560x1440"
    );
    // A refresh Windows refuses to raise the display to.
    first.lock().unwrap().refuses.push(1_000_000);
    let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
    );
    // Windows left 60 Hz behind: the stream already on it gets its mode back.
    let display = first.lock().unwrap();
    assert_eq!(display.mode, fixed);
    assert_eq!(display.shown.get(), fixed);
    assert_eq!(
        display.log.borrow().last().map(String::as_str),
        Some("stream raise refused")
    );
}

#[test]
fn a_stream_that_asks_for_more_than_its_launch_raises_a_shared_display_too() {
    // Two streams launched at 60 fps on one display; one asks for 240 Hz
    // at stream start.
    let fixed = (1920, 1080, 120_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    let second = stream(&mut displays, fixed, None).unwrap();
    assert_eq!(
        crate::display::virtual_display::raise_lease(&second, 240_000),
        Some(240_000)
    );
    assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
    // Asking for less leaves the faster refresh; a refusal puts it back.
    assert_eq!(
        crate::display::virtual_display::raise_lease(&first, 120_000),
        Some(240_000)
    );
    first.lock().unwrap().refuses.push(1_000_000);
    assert_eq!(
        crate::display::virtual_display::raise_lease(&first, 1_000_000),
        None
    );
    assert_eq!(second.lock().unwrap().shown.get(), (1920, 1080, 240_000));
}

#[test]
fn a_display_switched_on_under_a_new_name_is_a_new_generation() {
    let (mut name, mut generation) = (String::from(r"\\.\DISPLAY3"), 4);
    // The same name: the paused stream's guard keeps its target.
    crate::display::virtual_display::take_name(&mut name, &mut generation, r"\\.\DISPLAY3");
    assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY3", 4));
    // A new one: its guard sees another generation and moves there.
    crate::display::virtual_display::take_name(&mut name, &mut generation, r"\\.\DISPLAY5");
    assert_eq!((name.as_str(), generation), (r"\\.\DISPLAY5", 5));
}

#[test]
fn a_stream_raises_a_paused_shared_display_only_once_it_is_on_again() {
    // The VRR device relaunches onto the display a disconnected phone
    // left paused, and Windows switched it off meanwhile.
    let fixed = (1920, 1080, 120_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let paused = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = paused.lock().unwrap();
        display.on = false;
        display.log.borrow_mut().clear();
    }
    stream(&mut displays, (1920, 1080, 1_000_000), None).unwrap();
    let display = paused.lock().unwrap();
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream request"]
    );
    assert_eq!(display.shown.get(), (1920, 1080, 1_000_000));
}

#[test]
fn a_joining_stream_raises_a_shared_display_once_windows_lists_the_rate() {
    // S1a order B: the Retroid at 120 Hz first, then the VRR laptop at
    // 1000 Hz. The driver published the 1000 Hz list 16 ms after the host
    // set it, and Windows refused it as a bad mode.
    let fixed = (1920, 1080, 120_000);
    let vrr = (1920, 1080, 1_000_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.lists_after = Some(4);
        display.log.borrow_mut().clear();
    }
    let second = stream(&mut displays, vrr, None).unwrap();
    let display = second.lock().unwrap();
    // Set only in the fourth look, once Windows listed it.
    assert_eq!(display.looks.get(), 4);
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream request"]
    );
    assert_eq!(display.shown.get(), vrr);
}

#[test]
fn a_mode_the_driver_refused_to_offer_is_not_waited_for() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.driver_refuses = true;
        display.log.borrow_mut().clear();
    }
    let started = Instant::now();
    assert_eq!(
        crate::display::virtual_display::raise_lease(&first, 1_000_000),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    {
        let display = first.lock().unwrap();
        assert_eq!(display.looks.get(), 1);
        assert_eq!(
            *display.log.borrow(),
            ["offer 1000000", "stream raise refused"]
        );
        assert_eq!(display.shown.get(), fixed);
    }
    // Windows lists it already: the raise needs no new list.
    first
        .lock()
        .unwrap()
        .listed
        .borrow_mut()
        .push((1920, 1080, 240_000));
    assert_eq!(
        crate::display::virtual_display::raise_lease(&first, 240_000),
        Some(240_000)
    );
    assert_eq!(first.lock().unwrap().shown.get(), (1920, 1080, 240_000));
}

#[test]
fn a_turned_stream_keeps_its_size_at_a_slower_rate_windows_lists() {
    // A VRR stream at 4K and 1000 Hz turns to portrait on a PC that lists
    // 4K only up to 240 Hz. Windows lists the turned size in the third
    // look; the turn applies there at 240 Hz (issue #40).
    let landscape = (3840, 2160, 1_000_000);
    let mut display = Fake::new(landscape);
    display.fastest = 240_000;
    display.lists_after = Some(3);
    assert!(crate::display::virtual_display::reshape(
        &mut display,
        2160,
        3840
    ));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(*display.log.borrow(), ["offer 1000000", "stream turned"]);
    assert_eq!(display.shown.get(), (2160, 3840, 240_000));
    assert_eq!(display.mode, (2160, 3840, 1_000_000));
    // Turned back: the landscape size is listed, so one look serves it.
    display.log.borrow_mut().clear();
    assert!(crate::display::virtual_display::reshape(
        &mut display,
        3840,
        2160
    ));
    assert_eq!(display.looks.get(), 1);
    assert_eq!(display.shown.get(), landscape);
}

#[test]
fn a_turn_to_a_size_windows_lists_at_other_rates_waits_for_its_own() {
    // A portrait VRR stream turns to 1920x1080, which the driver lists at
    // 60 and 120 Hz from the start: those are not the turn's list.
    let portrait = (1080, 1920, 1_000_000);
    let landscape = (1920, 1080, 1_000_000);
    let standard = |display: &Fake| {
        display
            .listed
            .borrow_mut()
            .extend([(1920, 1080, 60_000), (1920, 1080, 120_000)]);
    };
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = Some(3);
    assert!(crate::display::virtual_display::reshape(
        &mut display,
        1920,
        1080
    ));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(display.shown.get(), landscape);
    // Where Windows lists the size only up to 480 Hz, the new list is
    // the turn's too, and the turn takes 480 Hz (issue #40).
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = Some(3);
    display.fastest = 480_000;
    assert!(crate::display::virtual_display::reshape(
        &mut display,
        1920,
        1080
    ));
    assert_eq!(display.looks.get(), 3);
    assert_eq!(display.shown.get(), (1920, 1080, 480_000));
    // A list that never changes: after the wait the turn takes 120 Hz.
    let mut display = Fake::new(portrait);
    standard(&display);
    display.lists_after = None;
    assert!(crate::display::virtual_display::reshape(
        &mut display,
        1920,
        1080
    ));
    assert_eq!(display.shown.get(), (1920, 1080, 120_000));
    // Turned back and again: the driver offered both modes already, so
    // neither turn waits, and the fallback serves the filtered one.
    for (width, height, shown) in [(1080, 1920, portrait), (1920, 1080, (1920, 1080, 120_000))] {
        let started = Instant::now();
        assert!(crate::display::virtual_display::reshape(
            &mut display,
            width,
            height
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(display.looks.get(), 1);
        assert_eq!(display.shown.get(), shown);
    }
    assert_eq!(display.offered, [portrait, landscape]);
}

#[test]
fn a_rate_windows_never_lists_is_not_set_and_the_stream_is_refused_with_why() {
    let fixed = (1920, 1080, 120_000);
    let mut displays = crate::display::virtual_display::Leases::new();
    let first = stream(&mut displays, fixed, Some(Fake::new(fixed))).unwrap();
    {
        let mut display = first.lock().unwrap();
        display.lists_after = None;
        display.log.borrow_mut().clear();
    }
    let error = stream(&mut displays, (1920, 1080, 1_000_000), None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "shared virtual display already uses a different mode: it runs at 120.000 Hz, and Windows refused the 1000.000 Hz this stream asks for"
    );
    let display = first.lock().unwrap();
    // The unlisted rate was never set; the stream on it keeps its mode.
    assert!(display.looks.get() > 1);
    assert_eq!(
        *display.log.borrow(),
        ["on", "offer 1000000", "stream raise refused"]
    );
    assert_eq!(display.mode, fixed);
    assert_eq!(display.shown.get(), fixed);
}

#[test]
fn waiting_for_a_listed_mode_costs_one_look_when_it_is_listed_and_stops_in_time() {
    assert_eq!(
        crate::display::virtual_display::wait_listed(|| true, MODE_LIST_WAIT),
        Some(1)
    );
    let mut looks = 0;
    let polls = crate::display::virtual_display::wait_listed(
        || {
            looks += 1;
            looks == 3
        },
        MODE_LIST_WAIT,
    );
    assert_eq!(polls, Some(3));
    let started = Instant::now();
    assert_eq!(
        crate::display::virtual_display::wait_listed(|| false, Duration::from_millis(30)),
        None
    );
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(30), "{waited:?}");
    assert!(waited < Duration::from_secs(1), "{waited:?}");
}

#[test]
fn the_console_names_why_setup_did_not_install_the_driver() {
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
    let missing = anyhow::Error::new(windows::core::Error::from(ERROR_NO_MORE_ITEMS.to_hresult()));
    let refused = "Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.";
    assert_eq!(
        crate::display::virtual_display::problem_text(&missing, Some(refused.into())),
        refused
    );
    // Nothing recorded, or an empty value: the error explains it.
    for recorded in [None, Some(" ".into())] {
        assert!(
            crate::display::virtual_display::problem_text(&missing, recorded)
                .starts_with("Rubylight's display driver is not installed")
        );
    }
}
