//! Device pairing approval DTOs (implemented in `nightjar_policy::http`).

pub use nightjar_policy::http::{
    valid_pairing_pin, PairingApprove, PairingApproved, PairingDeny, PairingRequest,
    PairingRequests, FEATURE_PAIRING_APPROVE,
};
