//! Pure host record schemas, address slot parsing/sync, polling policy, power/update/pairing
//! state machines, invite helpers, per-host notice/mode/capability policies, and unreadable-host
//! sanitization shared across Nightjar clients.

pub mod address;
pub mod invite;
pub mod meta;
pub mod pairing;
pub mod poll;
pub mod power;
pub mod record;
pub mod sanitize;

pub use address::{
    DEFAULT_HTTP_PORT, EndpointSlot, HostSlots, PinnedAddress, is_site_local_ip,
    is_site_local_ipv4, public_ipv4, sync_shared_from_slots, sync_slots_from_shared,
};
pub use invite::{
    EXPIRED, INTERNET_OFF, InviteErrorKind, KNOWN_PC, NOT_AN_INVITE, REFUSED,
    classify_invite_error, invite_claims_known_pc, is_tunnel_address,
};
pub use meta::{
    ALL_PACKED_MODES, HostCapabilitiesSnapshot, HostNotice, HostNoticesState,
    HostPackedEntry, HostPackedModesState, NOTICE_FRAME_WARP_UNAVAILABLE,
    NOTICE_PACKED_UNAVAILABLE, NOTICE_PYROWAVE_FALLBACK,
};
pub use pairing::{
    PAIRING_WATCH_INTERVAL_MS, PairFailureKind, PairFailureState, PairingWatchCommand,
    PairingWatchFinish, PairingWatchTracker, UnpairFailureKind, already_paired,
    classify_pairing_failure, classify_pairing_http_refusal, classify_unpair,
    pairing_refused_status, strip_bidi_controls,
};
pub use poll::{
    APPLIST_POLL_PERIOD_MS, APPLIST_RETRY_PERIOD_MS, CandidateSlot, EMPTY_APPLIST_THRESHOLD,
    HostReachState, INITIAL_POLL_TRIES, INTERNET_PLAY_READ_INTERVAL_MS, OFFLINE_POLL_TRIES,
    POLL_DATA_TTL_MS, PollErrorKind, PollOutcome, PollTupleState, ProbeCandidate,
    SERVERINFO_POLL_PERIOD_MS, accept_applist_response, is_internet_play_read_due,
    next_applist_interval_ms, port_matches_active_address, probe_plan, punches_now,
    should_clear_cert_on_unpair,
};
pub use power::{
    PC_POWER_REFUSAL_POLL_MS, PC_POWER_REFUSAL_READS, PcPowerStepAction, PowerFailureKind,
    SLEEP_TOKEN_REFRESH_MS, SleepOfferState, SleepStepAction, SleepUnavailableReason, TokenStamp,
    UPDATE_MAX_WAIT_MS, UPDATE_POLL_MS, UPDATE_WAIT_MS, UpdateStepAction, UpdateTargetError,
    UpdateTracker, classify_sleep_unavailable, sleep_option, update_target,
};
pub use record::{ALIAS_MAX_CHARS, HostNames, IdentityFile, SharedHostFields};
pub use sanitize::keep_readable_hosts;

#[cfg(test)]
mod tests;
