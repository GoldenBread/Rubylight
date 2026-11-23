//! Thread-safe mDNS discovery deduplication tracker and scope-preserving IPv6 selector
//! backed by `nightjar_policy::mdns`.

use std::net::Ipv6Addr;
use std::sync::Mutex;

pub use nightjar_policy::mdns::{
    CandidateKey, DiscoveryState, ProbeDecision, best_ipv6_index, is_excluded_global_ipv6,
    is_local_ipv6, link_local_ipv6_index, local_ipv6_index,
};

/// Thread-safe wrapper around [`DiscoveryState`] so blocking host probes can run outside the
/// mutex while generation checks and deduplication remain atomic.
#[derive(Debug, Default)]
pub struct MdnsDiscoveryTracker {
    state: Mutex<DiscoveryState>,
}

impl MdnsDiscoveryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DiscoveryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation()
    }

    pub fn reset(&self) -> u64 {
        self.lock().reset()
    }

    pub fn begin_candidate(&self, candidate: CandidateKey, generation: u64) -> ProbeDecision {
        self.lock().begin_candidate(candidate, generation)
    }

    pub fn mark_answered(&self, candidate: CandidateKey) {
        self.lock().mark_answered(candidate);
    }

    pub fn forget_answered(&self) {
        self.lock().forget_answered();
    }

    pub fn computers(&self) -> Vec<CandidateKey> {
        self.lock().computers().to_vec()
    }
}

fn parse_ipv6_base(raw: &str) -> Option<Ipv6Addr> {
    let trimmed = raw.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let base = unbracketed.split('%').next().unwrap_or(unbracketed);
    base.parse::<Ipv6Addr>().ok()
}

/// Parses IPv6 address strings (stripping any `%scope_id` suffix before parsing so callers can
/// preserve the original `Inet6Address` object with its scope ID) and returns
/// `(best_global_index, local_index)` relative to `addrs`.
pub fn select_ipv6_strings(addrs: &[&str]) -> (Option<usize>, Option<usize>) {
    let mut indices = Vec::with_capacity(addrs.len());
    let mut parsed = Vec::with_capacity(addrs.len());
    for (idx, raw) in addrs.iter().enumerate() {
        if let Some(ip) = parse_ipv6_base(raw) {
            indices.push(idx);
            parsed.push(ip);
        }
    }
    let best = best_ipv6_index(&parsed).map(|i| indices[i]);
    let local = local_ipv6_index(&parsed).map(|i| indices[i]);
    (best, local)
}

/// Parses IPv6 address strings (stripping any `%scope_id`) and returns the index of the first
/// link-local IPv6 address in `addrs`, if any.
pub fn link_local_ipv6_strings(addrs: &[&str]) -> Option<usize> {
    let mut indices = Vec::with_capacity(addrs.len());
    let mut parsed = Vec::with_capacity(addrs.len());
    for (idx, raw) in addrs.iter().enumerate() {
        if let Some(ip) = parse_ipv6_base(raw) {
            indices.push(idx);
            parsed.push(ip);
        }
    }
    link_local_ipv6_index(&parsed).map(|i| indices[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_and_scoped_ipv6_selection_preserve_indices() {
        let addrs = [
            "fe80::1234:5678:9abc:def0%wlan0",
            "2001:db8::aaaa:bbbb:cccc:dddd",
            "2001:db8::1234:5678:9abc:def0%3",
        ];
        assert_eq!(select_ipv6_strings(&addrs), (Some(2), Some(0)));
        assert_eq!(link_local_ipv6_strings(&addrs), Some(0));
        assert_eq!(link_local_ipv6_strings(&["[fe80::1%wlan0]"]), Some(0));
        assert_eq!(
            select_ipv6_strings(&["[fe80::1%wlan0]", "[2001:db8::1234:5678:9abc:def0%3]"]),
            (Some(1), Some(0))
        );

        let tracker = MdnsDiscoveryTracker::new();
        let gen1 = tracker.reset();
        let candidate = CandidateKey {
            name: "PC".into(),
            port: 47989,
            local: Some("192.168.1.5".into()),
            v6: None,
        };
        assert_eq!(
            tracker.begin_candidate(candidate.clone(), gen1),
            ProbeDecision::Probe
        );
        tracker.mark_answered(candidate.clone());
        assert_eq!(tracker.computers().len(), 1);
        let gen2 = tracker.reset();
        assert_eq!(
            tracker.begin_candidate(candidate.clone(), gen2),
            ProbeDecision::Duplicate
        );
        tracker.forget_answered();
        let gen3 = tracker.reset();
        assert_eq!(tracker.begin_candidate(candidate, gen3), ProbeDecision::Probe);
    }
}
