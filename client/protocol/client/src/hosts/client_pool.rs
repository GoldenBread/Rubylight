//! Idle-TTL connection pool of [`HostClient`] instances keyed by `(address, http_port, cert_der)`.
//!
//! Replaces Java `PollConnections` and Desktop's ad-hoc `client_for` construction: preserves
//! rustls TLS configs across polls for up to [`POLL_DATA_TTL_MS`] (30 s), resets cached HTTPS
//! ports when passed `0` (forcing fresh HTTP `serverinfo` discovery), isolates entries by pinned
//! certificate bytes, discards cancelled clients, and increments a generation counter on [`clear`]
//! so in-flight requests borrowed before a network change or host removal are never reused.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nightjar_policy::hosts::POLL_DATA_TTL_MS;

use crate::http::{ClientIdentity, Error, HostClient};

const IDLE_TTL: Duration = Duration::from_millis(POLL_DATA_TTL_MS);
const MAX_IDLE_PER_KEY: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PoolKey {
    address: String,
    http_port: u16,
    cert_der: Option<Vec<u8>>,
}

struct IdleEntry {
    client: HostClient,
    last_used: Instant,
}

struct PoolState {
    identity: Arc<ClientIdentity>,
    device_name: String,
    generation: u64,
    entries: HashMap<PoolKey, Vec<IdleEntry>>,
}

impl PoolState {
    fn sweep(&mut self, now: Instant) {
        self.entries.retain(|_, list| {
            list.retain(|entry| {
                now.saturating_duration_since(entry.last_used) < IDLE_TTL
                    && !entry.client.is_cancelled()
            });
            !list.is_empty()
        });
    }
}

/// Thread-safe pool of [`HostClient`] instances shared across polling, applist, and host flows.
pub struct HostClientPool {
    state: Mutex<PoolState>,
}

impl std::fmt::Debug for HostClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let idle: usize = state.entries.values().map(Vec::len).sum();
        f.debug_struct("HostClientPool")
            .field("generation", &state.generation)
            .field("idle_entries", &idle)
            .finish()
    }
}

impl HostClientPool {
    /// Creates a new pool authenticated with `identity`.
    pub fn new(identity: Arc<ClientIdentity>) -> Self {
        Self::with_device_name(identity, "Nightjar")
    }

    /// Creates a new pool authenticated with `identity` and advertising `device_name` on pair.
    pub fn with_device_name(
        identity: Arc<ClientIdentity>,
        device_name: impl Into<String>,
    ) -> Self {
        Self {
            state: Mutex::new(PoolState {
                identity,
                device_name: device_name.into(),
                generation: 0,
                entries: HashMap::new(),
            }),
        }
    }

    /// Swaps the active client identity, clearing cached clients and bumping generation.
    pub fn set_identity(&self, identity: Arc<ClientIdentity>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.identity = identity;
        state.entries.clear();
        state.generation = state.generation.wrapping_add(1);
    }

    /// Updates the device name sent during pairing requests.
    pub fn set_device_name(&self, name: impl Into<String>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.device_name = name.into();
    }

    /// Borrows a pooled [`HostClient`] in an RAII guard that returns it to the pool on drop.
    pub fn borrow(
        self: &Arc<Self>,
        address: &str,
        http_port: u16,
        https_port: u16,
        cert_der: Option<&[u8]>,
    ) -> Result<PooledHostClient, Error> {
        let (client, generation) = self.take(address, http_port, https_port, cert_der)?;
        Ok(PooledHostClient {
            pool: Arc::clone(self),
            key: PoolKey {
                address: address.to_owned(),
                http_port,
                cert_der: cert_der.map(ToOwned::to_owned),
            },
            generation,
            client: Some(client),
        })
    }

    /// Takes a [`HostClient`] (reusing a non-expired idle entry if available) and returns
    /// `(client, generation)` for manual [`Self::give_back`].
    pub fn take(
        &self,
        address: &str,
        http_port: u16,
        https_port: u16,
        cert_der: Option<&[u8]>,
    ) -> Result<(HostClient, u64), Error> {
        self.take_at(address, http_port, https_port, cert_der, Instant::now())
    }

    fn take_at(
        &self,
        address: &str,
        http_port: u16,
        https_port: u16,
        cert_der: Option<&[u8]>,
        now: Instant,
    ) -> Result<(HostClient, u64), Error> {
        let key = PoolKey {
            address: address.to_owned(),
            http_port,
            cert_der: cert_der.map(ToOwned::to_owned),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.sweep(now);
        let generation = state.generation;
        if let Some(list) = state.entries.get_mut(&key) {
            while let Some(entry) = list.pop() {
                if !entry.client.is_cancelled() {
                    if list.is_empty() {
                        state.entries.remove(&key);
                    }
                    entry.client.set_https_port(https_port);
                    return Ok((entry.client, generation));
                }
            }
            state.entries.remove(&key);
        }
        let identity = Arc::clone(&state.identity);
        let device_name = state.device_name.clone();
        drop(state);

        let mut client =
            HostClient::new(address, http_port, identity)?.with_device_name(device_name);
        if https_port != 0 {
            client = client.with_https_port(https_port);
        }
        if let Some(der) = cert_der {
            client.pin_server_certificate_der(Some(der.to_vec()));
        }
        Ok((client, generation))
    }

    /// Returns a [`HostClient`] to the pool unless it was cancelled or borrowed before [`Self::clear`].
    pub fn give_back(
        &self,
        address: &str,
        http_port: u16,
        cert_der: Option<&[u8]>,
        generation: u64,
        client: HostClient,
    ) {
        self.give_back_at(
            PoolKey {
                address: address.to_owned(),
                http_port,
                cert_der: cert_der.map(ToOwned::to_owned),
            },
            generation,
            client,
            Instant::now(),
        );
    }

    fn give_back_at(&self, key: PoolKey, generation: u64, client: HostClient, now: Instant) {
        if client.is_cancelled() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.sweep(now);
        if state.generation != generation {
            return;
        }
        let list = state.entries.entry(key).or_default();
        if list.len() < MAX_IDLE_PER_KEY {
            list.push(IdleEntry {
                client,
                last_used: now,
            });
        }
    }

    /// Discards all idle pooled clients and bumps the generation counter so any in-flight
    /// borrowed clients are dropped rather than returned when finished.
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.entries.clear();
        state.generation = state.generation.wrapping_add(1);
    }

    /// Number of idle clients currently cached across all keys.
    pub fn idle_len(&self) -> usize {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.sweep(Instant::now());
        state.entries.values().map(Vec::len).sum()
    }

    /// Current clear/reset generation counter.
    pub fn generation(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).generation
    }
}

/// RAII lease for a [`HostClient`] borrowed from [`HostClientPool`].
pub struct PooledHostClient {
    pool: Arc<HostClientPool>,
    key: PoolKey,
    generation: u64,
    client: Option<HostClient>,
}

impl PooledHostClient {
    /// Exposes the underlying [`HostClient`].
    pub fn client(&self) -> &HostClient {
        self.client
            .as_ref()
            .expect("PooledHostClient accessed after release")
    }

    /// Takes ownership of the inner [`HostClient`] so it will not be returned to the pool.
    pub fn detach(mut self) -> HostClient {
        self.client
            .take()
            .expect("PooledHostClient already detached")
    }
}

impl Deref for PooledHostClient {
    type Target = HostClient;

    fn deref(&self) -> &Self::Target {
        self.client()
    }
}

impl Drop for PooledHostClient {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            self.pool.give_back_at(
                std::mem::replace(
                    &mut self.key,
                    PoolKey {
                        address: String::new(),
                        http_port: 0,
                        cert_der: None,
                    },
                ),
                self.generation,
                client,
                Instant::now(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::identity::tests::fixture;

    #[test]
    fn reuses_same_address_port_and_cert_until_ttl_and_resets_https_port() {
        let pool = HostClientPool::new(Arc::new(fixture()));
        let t0 = Instant::now();
        let cert_a = vec![1u8, 2, 3, 4];
        let cert_b = vec![9u8, 8, 7, 6];

        let (c1, g1) = pool
            .take_at("192.168.1.10", 47989, 47984, Some(&cert_a), t0)
            .unwrap();
        assert_eq!(c1.https_port_cached(), 47984);
        pool.give_back_at(
            PoolKey {
                address: "192.168.1.10".into(),
                http_port: 47989,
                cert_der: Some(cert_a.clone()),
            },
            g1,
            c1,
            t0,
        );
        assert_eq!(pool.idle_len(), 1);

        // Same key with https_port = 0 reuses the entry AND resets cached https_port to 0.
        let (c2, g2) = pool
            .take_at(
                "192.168.1.10",
                47989,
                0,
                Some(&cert_a),
                t0 + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(c2.https_port_cached(), 0);
        pool.give_back_at(
            PoolKey {
                address: "192.168.1.10".into(),
                http_port: 47989,
                cert_der: Some(cert_a.clone()),
            },
            g2,
            c2,
            t0 + Duration::from_secs(5),
        );

        // Different certificate misses cache cleanly.
        let (c_other, g_other) = pool
            .take_at(
                "192.168.1.10",
                47989,
                47984,
                Some(&cert_b),
                t0 + Duration::from_secs(6),
            )
            .unwrap();
        assert_eq!(c_other.server_certificate_der().as_deref(), Some(cert_b.as_slice()));
        drop((c_other, g_other));

        // Past 30 s idle TTL, cached entry is evicted before take.
        let (_c3, _) = pool
            .take_at(
                "192.168.1.10",
                47989,
                0,
                Some(&cert_a),
                t0 + Duration::from_secs(36),
            )
            .unwrap();
        assert_eq!(pool.idle_len(), 0);
    }

    #[test]
    fn clear_discards_in_flight_return_and_cancelled_clients_are_never_pooled() {
        let pool = Arc::new(HostClientPool::new(Arc::new(fixture())));
        let (c1, g1) = pool.take("10.0.0.5", 47989, 47984, None).unwrap();
        let (c2, g2) = pool.take("10.0.0.5", 47989, 47984, None).unwrap();

        pool.clear();
        assert_eq!(pool.generation(), g1 + 1);

        // Both were borrowed before clear(), so returning them must not repopulate the pool.
        pool.give_back("10.0.0.5", 47989, None, g1, c1);
        pool.give_back("10.0.0.5", 47989, None, g2, c2);
        assert_eq!(pool.idle_len(), 0);

        // A cancelled client returned at the current generation is also discarded.
        let (c3, g3) = pool.take("10.0.0.5", 47989, 47984, None).unwrap();
        c3.cancel_pending_requests();
        pool.give_back("10.0.0.5", 47989, None, g3, c3);
        assert_eq!(pool.idle_len(), 0);

        // Concurrent RAII borrows allocate distinct clients and return both cleanly.
        let b1 = pool.borrow("10.0.0.5", 47989, 47984, None).unwrap();
        let b2 = pool.borrow("10.0.0.5", 47989, 47984, None).unwrap();
        drop(b1);
        drop(b2);
        assert_eq!(pool.idle_len(), 2);
    }
}
