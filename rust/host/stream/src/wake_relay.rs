//! Away wake relay, the PC's side: paired devices on this PC's network enroll as relays, and a
//! paired device that is away asks one of them over Traverse to send this PC Wake-on-LAN
//! (`docs/designs/15-away-wake-relay.md`).
use butterpollo_config::config::Config;

/// Whether this PC offers the away wake relay: the setting is on, and so is Play over the
/// internet, whose rendezvous the relay and the device that is away meet through.
pub fn offered(config: &Config) -> bool {
    crate::internet_play::enabled(config) && config.away_wake_relay()
}
