//! Host power/update DTOs (implemented in `nightjar_policy::http`).

pub use nightjar_policy::http::{
    host_words, update_arrived, HostUpdate, LateRefusal, PcPower, PcPowerOffer, PowerAvailability,
    PowerRequest, FEATURE_RESTART, FEATURE_SHUTDOWN, FEATURE_UPDATE, POWER_TOKEN_REFRESH,
};
