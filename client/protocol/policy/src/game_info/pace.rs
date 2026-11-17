//! How fast each service is asked. IGDB allows 4 requests a second and answers 429 past that;
//! SteamGridDB documents no number but answers 429 too. Every request waits its turn behind the
//! last one to the same service, and a 429 pauses that service, by its `Retry-After` when it gave
//! one, else for twice as long as the time before.

use serde::{Deserialize, Serialize};

use super::job::Service;

/// The least time between two requests to one service, in milliseconds: IGDB's 4 a second.
pub const SPACING_MS: u64 = 250;
/// The first pause after a 429 without `Retry-After`.
pub const FIRST_BACKOFF_MS: u64 = 1_000;
/// The longest pause after a 429.
pub const MAX_BACKOFF_MS: u64 = 60_000;
/// How many 429s or failed tries one request takes before its part of the lookup is given up.
pub const TRIES: u32 = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Lane {
    /// When the last request was sent, by the caller's monotonic clock.
    last_ms: Option<u64>,
    /// No request before this.
    paused_until_ms: u64,
}

/// The pace of every service, kept for as long as the app runs lookups.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pace {
    lanes: [Lane; 3],
}

fn lane(service: Service) -> usize {
    match service {
        Service::SteamGridDb => 0,
        // The token comes from Twitch, but it is one step of asking IGDB.
        Service::Igdb | Service::Twitch => 1,
        Service::Image => 2,
    }
}

impl Pace {
    /// How long a request to `service` must wait at `now_ms`, 0 when it may go.
    pub fn wait_ms(&self, service: Service, now_ms: u64) -> u64 {
        if service == Service::Image {
            return 0;
        }
        let lane = &self.lanes[lane(service)];
        let spaced = lane.last_ms.map_or(0, |last| last.saturating_add(SPACING_MS));
        spaced.max(lane.paused_until_ms).saturating_sub(now_ms)
    }

    /// Notes a request to `service` sent at `now_ms`.
    pub fn sent(&mut self, service: Service, now_ms: u64) {
        self.lanes[lane(service)].last_ms = Some(now_ms);
    }

    /// Pauses `service` after its `tries`th 429 (1 for the first) at `now_ms`, for its
    /// `retry_after` seconds when it gave them, else [`backoff_ms`]. Returns the pause.
    pub fn too_many(&mut self, service: Service, now_ms: u64, retry_after: Option<u64>, tries: u32) -> u64 {
        let pause = match retry_after {
            Some(seconds) => seconds.saturating_mul(1_000).min(MAX_BACKOFF_MS),
            None => backoff_ms(tries),
        };
        let lane = &mut self.lanes[lane(service)];
        lane.paused_until_ms = lane.paused_until_ms.max(now_ms.saturating_add(pause));
        pause
    }
}

/// The pause after the `tries`th failure in a row (1 for the first): 1 s, 2 s, 4 s, ... up to
/// [`MAX_BACKOFF_MS`].
pub fn backoff_ms(tries: u32) -> u64 {
    let doubled = FIRST_BACKOFF_MS.saturating_mul(1u64 << tries.saturating_sub(1).min(16));
    doubled.min(MAX_BACKOFF_MS)
}
