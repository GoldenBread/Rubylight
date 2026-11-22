//! Session guest DTOs (implemented in `nightjar_policy::http`).

#[allow(unused_imports)]
pub use nightjar_policy::http::{
    guest_ended, join_refused, GuestAction, GuestRequest, InviteCreate, InviteCreated,
    SessionGuest, SessionGuests, EXPIRED, FEATURE_GUEST_INVITES, FEATURE_SESSION_GUESTS,
    GAME_OVER, GUESTS_POLL_MS, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE, GUEST_PERMISSIONS,
    GUEST_VIEW, INVITED_JOINS_ONLY, KICKED, REVOKED,
};
