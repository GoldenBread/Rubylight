//! Client roaming: which default-network callbacks are a network change the engine must hear
//! about. Pure state machine (no I/O or clock), matching Android's `RoamingNetworkWatch`.

pub const RECHECK_MS: u64 = crate::recovery::PEER_TIMEOUT_MS as u64;
pub const NETWORK_CARRIES_ON: i32 = 0;
pub const NETWORK_ROAMED: i32 = 1;
pub const NETWORK_CANNOT_CONTINUE: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilitiesStep {
    Ignore,
    Ask { metered: bool, returned: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressesStep {
    Ignore,
    Ask { metered: bool },
}

#[derive(Clone, Debug)]
pub struct RoamingTracker {
    current: Option<u64>,
    lost: bool,
    stopped: bool,
    recheck: Option<u64>,
    recheck_metered: bool,
    recheck_until: u64,
    recheck_addresses: Option<String>,
    recheck_nat64: bool,
}

impl RoamingTracker {
    pub fn new(stream_network: Option<u64>) -> Self {
        Self {
            current: stream_network,
            lost: false,
            stopped: false,
            recheck: None,
            recheck_metered: false,
            recheck_until: 0,
            recheck_addresses: None,
            recheck_nat64: false,
        }
    }

    pub fn begin_capabilities(&mut self, network: u64, not_metered: bool) -> CapabilitiesStep {
        if self.stopped {
            return CapabilitiesStep::Ignore;
        }
        let Some(current) = self.current else {
            // Nothing known about the stream's network: the first one seen is where it runs.
            self.current = Some(network);
            return CapabilitiesStep::Ignore;
        };
        if network == current && !self.lost {
            if self.recheck == Some(network) {
                self.recheck_metered = !not_metered;
            }
            return CapabilitiesStep::Ignore;
        }
        let returned = self.lost;
        let metered = !not_metered;
        self.current = Some(network);
        self.lost = false;
        self.recheck = None;
        CapabilitiesStep::Ask { metered, returned }
    }

    pub fn finish_capabilities(
        &mut self,
        network: u64,
        metered: bool,
        returned: bool,
        change: i32,
        now_ms: u64,
    ) -> Option<(i32, bool)> {
        if self.stopped || !returned {
            return None;
        }
        if change == NETWORK_CARRIES_ON {
            // The same address back, or no route yet: ask again when its addresses change.
            self.recheck = Some(network);
            self.recheck_metered = metered;
            self.recheck_until = now_ms.saturating_add(RECHECK_MS);
            self.recheck_addresses = None;
            self.recheck_nat64 = false;
        }
        Some((change, metered))
    }

    pub fn capabilities(
        &mut self,
        network: u64,
        not_metered: bool,
        now_ms: u64,
        ask_engine: impl FnOnce(bool) -> i32,
    ) -> Option<(i32, bool)> {
        match self.begin_capabilities(network, not_metered) {
            CapabilitiesStep::Ignore => None,
            CapabilitiesStep::Ask { metered, returned } => {
                let change = ask_engine(metered);
                self.finish_capabilities(network, metered, returned, change, now_ms)
            }
        }
    }

    pub fn begin_addresses(
        &mut self,
        network: u64,
        addresses: &str,
        nat64: bool,
        now_ms: u64,
    ) -> AddressesStep {
        if self.stopped || self.lost || self.recheck != Some(network) {
            return AddressesStep::Ignore;
        }
        if now_ms > self.recheck_until {
            self.recheck = None;
            return AddressesStep::Ignore;
        }
        // A prefix found after the addresses is news too.
        let nat64_changed = nat64 != self.recheck_nat64;
        self.recheck_nat64 = nat64;
        if self.recheck_addresses.as_deref() == Some(addresses) && !nat64_changed {
            return AddressesStep::Ignore;
        }
        self.recheck_addresses = Some(addresses.to_owned());
        AddressesStep::Ask {
            metered: self.recheck_metered,
        }
    }

    pub fn finish_addresses(&mut self, change: i32) -> Option<(i32, bool)> {
        if self.stopped || change == NETWORK_CARRIES_ON {
            return None;
        }
        self.recheck = None;
        Some((change, self.recheck_metered))
    }

    pub fn addresses(
        &mut self,
        network: u64,
        addresses: &str,
        nat64: bool,
        now_ms: u64,
        ask_engine: impl FnOnce(bool) -> i32,
    ) -> Option<(i32, bool)> {
        match self.begin_addresses(network, addresses, nat64, now_ms) {
            AddressesStep::Ignore => None,
            AddressesStep::Ask { metered } => {
                let change = ask_engine(metered);
                self.finish_addresses(change)
            }
        }
    }

    pub fn lost(&mut self, network: u64) -> bool {
        if self.stopped || self.lost || self.current != Some(network) {
            return false;
        }
        self.lost = true;
        self.recheck = None;
        true
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIFI: u64 = 1;
    const OTHER_WIFI: u64 = 2;
    const CELLULAR: u64 = 3;

    #[derive(Default)]
    struct Events {
        seen: Vec<String>,
        answer: i32,
    }

    impl Events {
        fn new() -> Self {
            Self {
                seen: Vec::new(),
                answer: NETWORK_CARRIES_ON,
            }
        }

        fn drive_cap(
            &mut self,
            tracker: &mut RoamingTracker,
            net: u64,
            not_metered: bool,
            now: u64,
        ) {
            let answer = self.answer;
            let seen = &mut self.seen;
            if let Some((change, metered)) = tracker.capabilities(net, not_metered, now, |m| {
                seen.push(if m { "changed metered" } else { "changed" }.into());
                answer
            }) {
                seen.push(format!(
                    "returned {change}{}",
                    if metered { " metered" } else { "" }
                ));
            }
        }

        fn drive_addr(
            &mut self,
            tracker: &mut RoamingTracker,
            net: u64,
            addrs: &str,
            nat64: bool,
            now: u64,
        ) {
            let answer = self.answer;
            let seen = &mut self.seen;
            if let Some((change, metered)) = tracker.addresses(net, addrs, nat64, now, |m| {
                seen.push(if m { "changed metered" } else { "changed" }.into());
                answer
            }) {
                seen.push(format!(
                    "returned {change}{}",
                    if metered { " metered" } else { "" }
                ));
            }
        }

        fn drive_lost(&mut self, tracker: &mut RoamingTracker, net: u64) {
            if tracker.lost(net) {
                self.seen.push("lost".into());
            }
        }
    }

    #[test]
    fn the_streams_own_network_and_its_capability_updates_are_no_change() {
        let mut tracker = RoamingTracker::new(Some(WIFI));
        let mut reports = Vec::new();
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        tracker.capabilities(WIFI, false, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert!(reports.is_empty());
    }

    #[test]
    fn a_new_default_network_is_reported_once_with_whether_it_is_metered() {
        let mut tracker = RoamingTracker::new(Some(WIFI));
        let mut reports = Vec::new();
        tracker.capabilities(CELLULAR, false, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        tracker.capabilities(CELLULAR, false, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        tracker.capabilities(OTHER_WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert_eq!(reports, vec![true, false]);
    }

    #[test]
    fn the_same_network_coming_back_after_it_was_lost_is_a_change() {
        let mut tracker = RoamingTracker::new(Some(WIFI));
        let mut reports = Vec::new();
        assert!(tracker.lost(WIFI));
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert_eq!(reports, vec![false]);
        assert!(!tracker.lost(CELLULAR));
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert_eq!(reports, vec![false]);
    }

    #[test]
    fn without_the_start_network_the_first_one_seen_is_the_baseline() {
        let mut tracker = RoamingTracker::new(None);
        let mut reports = Vec::new();
        tracker.capabilities(WIFI, true, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert!(reports.is_empty());
        tracker.capabilities(CELLULAR, false, 0, |m| {
            reports.push(m);
            NETWORK_CARRIES_ON
        });
        assert_eq!(reports, vec![true]);
    }

    #[test]
    fn a_lost_network_coming_back_is_a_return_with_the_engines_answer() {
        let mut events = Events::new();
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_lost(&mut tracker, WIFI);
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, OTHER_WIFI, true, 0);
        events.drive_cap(&mut tracker, OTHER_WIFI, true, 0);
        assert_eq!(events.seen, vec!["lost", "changed", "returned 0"]);

        events.seen.clear();
        events.answer = NETWORK_CANNOT_CONTINUE;
        events.drive_lost(&mut tracker, OTHER_WIFI);
        events.drive_cap(&mut tracker, CELLULAR, false, 0);
        assert_eq!(
            events.seen,
            vec!["lost", "changed metered", "returned 2 metered"]
        );
    }

    #[test]
    fn a_return_without_a_route_yet_is_asked_again_as_its_addresses_arrive() {
        let mut events = Events::new();
        let mut now = 1000;
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, OTHER_WIFI, true, now);
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", false, now);
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", false, now);
        events.drive_addr(&mut tracker, CELLULAR, "10.0.0.5", false, now);
        now += 7000;
        events.answer = NETWORK_CANNOT_CONTINUE;
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1 192.168.1.21", false, now);
        events.drive_addr(
            &mut tracker,
            OTHER_WIFI,
            "fe80::1 192.168.1.21 2001:db8::21",
            false,
            now,
        );
        assert_eq!(
            events.seen,
            vec![
                "lost",
                "changed",
                "returned 0",
                "changed",
                "changed",
                "returned 2"
            ]
        );
    }

    #[test]
    fn a_return_is_asked_again_only_until_the_connections_own_timeout() {
        let mut events = Events::new();
        let mut now = 1000;
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, OTHER_WIFI, false, now);
        events.seen.clear();
        events.drive_cap(&mut tracker, OTHER_WIFI, true, now);
        events.drive_cap(&mut tracker, OTHER_WIFI, false, now);
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", false, now);
        assert_eq!(events.seen, vec!["changed metered"]);
        now += RECHECK_MS + 1;
        events.answer = NETWORK_CANNOT_CONTINUE;
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1 192.168.1.21", false, now);
        assert_eq!(events.seen, vec!["changed metered"]);
    }

    #[test]
    fn only_a_return_the_connection_carried_on_over_is_asked_again() {
        let mut events = Events::new();
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_addr(&mut tracker, WIFI, "192.168.1.20", false, 0);
        events.answer = NETWORK_ROAMED;
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, OTHER_WIFI, true, 0);
        events.seen.clear();
        events.drive_addr(&mut tracker, OTHER_WIFI, "192.168.1.21", false, 0);
        assert!(events.seen.is_empty());

        events.answer = NETWORK_CANNOT_CONTINUE;
        events.drive_lost(&mut tracker, OTHER_WIFI);
        events.drive_cap(&mut tracker, CELLULAR, true, 0);
        events.seen.clear();
        events.drive_addr(&mut tracker, CELLULAR, "10.0.0.5", false, 0);
        assert!(events.seen.is_empty());

        events.answer = NETWORK_CARRIES_ON;
        events.drive_lost(&mut tracker, CELLULAR);
        events.drive_cap(&mut tracker, WIFI, true, 0);
        events.drive_lost(&mut tracker, WIFI);
        events.seen.clear();
        events.drive_addr(&mut tracker, WIFI, "192.168.1.22", false, 0);
        events.drive_cap(&mut tracker, WIFI, true, 0);
        tracker.stop();
        events.drive_addr(&mut tracker, WIFI, "192.168.1.23", false, 0);
        assert_eq!(events.seen, vec!["changed", "returned 0"]);
    }

    #[test]
    fn a_nat64_prefix_found_after_the_addresses_asks_again() {
        let mut events = Events::new();
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, OTHER_WIFI, true, 0);
        events.seen.clear();
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", false, 0);
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", false, 0);
        events.drive_addr(&mut tracker, OTHER_WIFI, "fe80::1", true, 0);
        assert_eq!(events.seen, vec!["changed", "changed"]);
    }

    #[test]
    fn an_engine_that_tells_keeps_its_own_answer_and_new_network_without_loss() {
        let mut events = Events::new();
        let mut tracker = RoamingTracker::new(Some(WIFI));
        events.drive_addr(&mut tracker, WIFI, "192.168.1.20", false, 0);
        events.drive_lost(&mut tracker, WIFI);
        events.drive_cap(&mut tracker, CELLULAR, false, 0);
        events.drive_addr(&mut tracker, CELLULAR, "10.0.0.5", false, 0);
        assert_eq!(
            events.seen,
            vec![
                "lost",
                "changed metered",
                "returned 0 metered",
                "changed metered"
            ]
        );

        let mut no_loss = Events::new();
        let mut tracker2 = RoamingTracker::new(Some(WIFI));
        no_loss.drive_cap(&mut tracker2, OTHER_WIFI, true, 0);
        no_loss.drive_lost(&mut tracker2, WIFI);
        assert_eq!(no_loss.seen, vec!["changed"]);

        let mut ended = Events::new();
        let mut tracker3 = RoamingTracker::new(Some(WIFI));
        tracker3.stop();
        ended.drive_lost(&mut tracker3, WIFI);
        ended.drive_cap(&mut tracker3, OTHER_WIFI, true, 0);
        assert!(ended.seen.is_empty());
    }
}
