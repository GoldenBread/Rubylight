//! Pure mDNS discovery deduplication, generation guard, and IPv6 address selection
//! shared across Nightjar clients without network or thread state.

use std::net::Ipv6Addr;

use serde::{Deserialize, Serialize};

/// Whether `addr` is a local IPv6 address (link-local `fe80::/10`, site-local `fec0::/10`,
/// or unique local `fc00::/7`).
pub fn is_local_ipv6(addr: Ipv6Addr) -> bool {
    let seg0 = addr.segments()[0];
    (seg0 & 0xffc0) == 0xfe80 || (seg0 & 0xffc0) == 0xfec0 || (seg0 & 0xfe00) == 0xfc00
}

/// Index of the first local IPv6 address in `addrs`, if any.
pub fn local_ipv6_index(addrs: &[Ipv6Addr]) -> Option<usize> {
    addrs.iter().position(|&addr| is_local_ipv6(addr))
}

/// Index of the first link-local IPv6 address (`fe80::/10`) in `addrs`, if any.
pub fn link_local_ipv6_index(addrs: &[Ipv6Addr]) -> Option<usize> {
    addrs
        .iter()
        .position(|addr| (addr.segments()[0] & 0xffc0) == 0xfe80)
}

/// Whether `addr` is excluded as a global IPv6 endpoint: link-local, site-local, loopback,
/// unspecified, multicast, 6to4 (`2002::/16`), Teredo (`2001::/32`), or ULA (`fc00::/7`).
pub fn is_excluded_global_ipv6(addr: Ipv6Addr) -> bool {
    let segs = addr.segments();
    is_local_ipv6(addr)
        || addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        || segs[0] == 0x2002
        || (segs[0] == 0x2001 && segs[1] == 0x0000)
}

/// Selects the best global IPv6 address index from `addrs`, preferring a non-excluded global
/// address whose 64-bit SLAAC interface identifier (`octets[8..16]`) matches the first
/// link-local address, falling back to the first non-excluded global address.
pub fn best_ipv6_index(addrs: &[Ipv6Addr]) -> Option<usize> {
    let link_local = link_local_ipv6_index(addrs).map(|i| addrs[i].octets());
    if let Some(ll_octets) = link_local
        && let Some(idx) = addrs
            .iter()
            .position(|&addr| !is_excluded_global_ipv6(addr) && addr.octets()[8..16] == ll_octets[8..16])
    {
        return Some(idx);
    }
    addrs
        .iter()
        .position(|&addr| !is_excluded_global_ipv6(addr))
}

/// Unique identity of one discovered mDNS candidate `(name, port, local, v6)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CandidateKey {
    pub name: String,
    pub port: u16,
    pub local: Option<String>,
    pub v6: Option<String>,
}

/// Outcome of registering a candidate in [`DiscoveryState::begin_candidate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeDecision {
    /// Resolution belonged to an older discovery generation and was ignored.
    StaleGeneration,
    /// Candidate was already seen in this session or already answered before.
    Duplicate,
    /// Caller should probe this candidate outside any lock and call `mark_answered` if reachable.
    Probe,
}

/// Generation-guarded mDNS discovery cache (`computers` + `answered`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryState {
    generation: u64,
    computers: Vec<CandidateKey>,
    answered: Vec<CandidateKey>,
}

impl DiscoveryState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Clears the current session's `computers` set and advances the generation counter.
    pub fn reset(&mut self) -> u64 {
        self.computers.clear();
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Registers `candidate` under `generation`. Returns [`ProbeDecision::Probe`] only when
    /// `generation` matches the current generation, `candidate` is newly added to `computers`,
    /// and `candidate` is not already in `answered`.
    pub fn begin_candidate(&mut self, candidate: CandidateKey, generation: u64) -> ProbeDecision {
        if generation != self.generation {
            return ProbeDecision::StaleGeneration;
        }
        if self.computers.contains(&candidate) {
            return ProbeDecision::Duplicate;
        }
        self.computers.push(candidate.clone());
        if self.answered.contains(&candidate) {
            ProbeDecision::Duplicate
        } else {
            ProbeDecision::Probe
        }
    }

    /// Records that `candidate` answered a probe, replacing any earlier answered entry with
    /// the same `name` so moving back to an earlier address is probed again.
    pub fn mark_answered(&mut self, candidate: CandidateKey) {
        self.answered.retain(|existing| existing.name != candidate.name);
        self.answered.push(candidate);
    }

    /// Clears remembered answered hosts (e.g. when a user deletes a PC).
    pub fn forget_answered(&mut self) {
        self.answered.clear();
    }

    pub fn computers(&self) -> &[CandidateKey] {
        &self.computers
    }
}

#[cfg(test)]
mod tests;
