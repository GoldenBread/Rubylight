//! Controller-vs-phone microphone route policy, ported from `NvConnection.java`.
//!
//! Tracks per-controller capture ownership (`ready` / `host_muted`), ensures removing a
//! host-muted controller latches local mute so phone fallback does not unmute unasked, and
//! resolves reconnect mute snapshots without querying native connection state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Per-controller microphone state reported by USB / controller audio endpoints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerMicState {
    pub allowed: bool,
    pub ready: bool,
    pub host_muted: bool,
}

impl ControllerMicState {
    /// Normalizes `ready` so a disallowed controller never claims readiness.
    pub const fn new(allowed: bool, ready: bool, host_muted: bool) -> Self {
        Self {
            allowed,
            ready: allowed && ready,
            host_muted,
        }
    }
}

/// Effective microphone route status across controller owners and phone fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicRouteView {
    /// At least one controller microphone is active and ready.
    ControllerReady { muted: bool },
    /// No controller is ready, but a controller holds mute (`host_muted`) or `mic_muted`
    /// while allowed.
    ControllerMuted,
    /// Phone fallback owns (or would own) the route.
    Phone,
}

/// Deterministic microphone route state machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicRoutePolicy {
    mic_muted: bool,
    owners: BTreeMap<u64, ControllerMicState>,
}

impl MicRoutePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mic_muted(&self) -> bool {
        self.mic_muted
    }

    pub fn set_mic_muted(&mut self, muted: bool) {
        self.mic_muted = muted;
    }

    pub fn clear_owners(&mut self) {
        self.owners.clear();
    }

    /// Updates or removes `owner_id`. When `can_send_controller` is false, `state` is treated
    /// as `None`. Removing a previously `host_muted` owner latches `mic_muted = true` so the
    /// phone fallback stays muted until the user explicitly unmutes.
    pub fn update_owner(
        &mut self,
        owner_id: u64,
        can_send_controller: bool,
        state: Option<ControllerMicState>,
    ) -> bool {
        let state = if can_send_controller { state } else { None };
        let previous = self.owners.get(&owner_id).copied();
        if state.is_none() && previous.is_some_and(|prev| prev.host_muted) {
            self.mic_muted = true;
        }
        match state {
            Some(s) => {
                self.owners.insert(owner_id, s);
            }
            None => {
                self.owners.remove(&owner_id);
            }
        }
        self.controller_owns_route()
    }

    /// Whether any controller owns the microphone route (ready or host-muted).
    pub fn controller_owns_route(&self) -> bool {
        self.owners.values().any(|s| s.ready || s.host_muted)
    }

    /// Snapshot of mute state for reconnect intents without querying native state.
    pub fn is_muted_for_reconnect(&self) -> bool {
        self.mic_muted || self.owners.values().any(|s| s.host_muted)
    }

    /// Resolves the UI-facing microphone route view across all current controller owners.
    pub fn route_view(&self) -> MicRouteView {
        let mut controller_muted = false;
        for state in self.owners.values() {
            if state.ready {
                return MicRouteView::ControllerReady {
                    muted: self.mic_muted,
                };
            }
            controller_muted |= state.host_muted || (state.allowed && self.mic_muted);
        }
        if controller_muted {
            MicRouteView::ControllerMuted
        } else {
            MicRouteView::Phone
        }
    }
}

/// Stateless evaluation of `controller_owns_route` over a slice of states.
pub fn slice_controller_owns_route(states: &[ControllerMicState]) -> bool {
    states.iter().any(|s| s.ready || s.host_muted)
}

/// Stateless evaluation of `is_muted_for_reconnect` over `mic_muted` and controller states.
pub fn slice_is_muted_for_reconnect(mic_muted: bool, states: &[ControllerMicState]) -> bool {
    mic_muted || states.iter().any(|s| s.host_muted)
}

/// Stateless evaluation of `route_view` over `mic_muted` and controller states.
pub fn slice_route_view(mic_muted: bool, states: &[ControllerMicState]) -> MicRouteView {
    let mut controller_muted = false;
    for state in states {
        if state.ready {
            return MicRouteView::ControllerReady { muted: mic_muted };
        }
        controller_muted |= state.host_muted || (state.allowed && mic_muted);
    }
    if controller_muted {
        MicRouteView::ControllerMuted
    } else {
        MicRouteView::Phone
    }
}

/// Whether the connection asks for and enables the microphone (`NvConnection.microphoneWanted`).
/// Remote Monitor is strictly view-only; Mic-only always enables microphone capture.
pub const fn microphone_wanted(enable_mic: bool, mic_only: bool, remote_monitor: bool) -> bool {
    (enable_mic || mic_only) && !remote_monitor
}

/// Whether echo cancellation should be disabled for microphone processing (Mic-only plays no
/// host audio on the device, so there is no echo to cancel).
pub const fn mic_disables_echo_cancellation(mic_only: bool) -> bool {
    mic_only
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_mute_snapshot_tracks_local_and_host_mute() {
        let mut policy = MicRoutePolicy::new();
        assert!(!policy.is_muted_for_reconnect());
        policy.set_mic_muted(true);
        assert!(policy.is_muted_for_reconnect());
        policy.set_mic_muted(false);
        assert!(!policy.is_muted_for_reconnect());

        let muted_owner = 1;
        let ready_owner = 2;
        policy.update_owner(
            muted_owner,
            true,
            Some(ControllerMicState::new(true, false, true)),
        );
        assert!(!policy.mic_muted());
        assert!(policy.is_muted_for_reconnect());

        policy.update_owner(
            muted_owner,
            true,
            Some(ControllerMicState::new(false, false, true)),
        );
        policy.update_owner(
            ready_owner,
            true,
            Some(ControllerMicState::new(true, true, false)),
        );
        assert!(policy.is_muted_for_reconnect());

        policy.update_owner(
            muted_owner,
            true,
            Some(ControllerMicState::new(false, false, false)),
        );
        assert!(!policy.is_muted_for_reconnect());

        policy.update_owner(
            muted_owner,
            true,
            Some(ControllerMicState::new(false, false, true)),
        );
        policy.update_owner(muted_owner, true, None);
        assert!(policy.mic_muted());
        assert!(policy.is_muted_for_reconnect());

        policy.set_mic_muted(false);
        assert!(!policy.is_muted_for_reconnect());
    }

    #[test]
    fn retiring_controller_cannot_displace_another_ready_controller() {
        let mut policy = MicRoutePolicy::new();
        let first = 1;
        let second = 2;
        assert!(policy.update_owner(
            first,
            true,
            Some(ControllerMicState::new(true, true, false))
        ));
        assert!(policy.update_owner(
            second,
            true,
            Some(ControllerMicState::new(true, true, false))
        ));
        assert!(policy.update_owner(first, true, None));
        assert!(policy.controller_owns_route());
        assert_eq!(
            policy.route_view(),
            MicRouteView::ControllerReady { muted: false }
        );
        assert!(!policy.update_owner(second, true, None));
        assert!(!policy.controller_owns_route());
        assert_eq!(policy.route_view(), MicRouteView::Phone);
    }

    #[test]
    fn host_mute_and_capture_denial_latch_until_explicit_unmute() {
        let mut policy = MicRoutePolicy::new();
        let pad = 1;
        policy.update_owner(pad, true, Some(ControllerMicState::new(true, true, false)));
        policy.update_owner(pad, true, Some(ControllerMicState::new(true, false, true)));
        assert_eq!(policy.route_view(), MicRouteView::ControllerMuted);
        policy.update_owner(pad, true, Some(ControllerMicState::new(false, false, true)));
        assert_eq!(policy.route_view(), MicRouteView::ControllerMuted);
        policy.update_owner(pad, true, None);
        assert!(policy.mic_muted());
        assert_eq!(policy.route_view(), MicRouteView::Phone);
        policy.set_mic_muted(false);
        assert!(!policy.mic_muted());
    }

    #[test]
    fn microphone_wanted_respects_remote_monitor_and_mic_only() {
        assert!(!microphone_wanted(true, false, true));
        assert!(!microphone_wanted(true, true, true));
        assert!(microphone_wanted(false, true, false));
        assert!(microphone_wanted(true, false, false));
        assert!(!microphone_wanted(false, false, false));
    }
}
