//! mDNS discovery lifecycle, restarting registration cleanly across Windows subnet changes.

use anyhow::Result;
use butterpollo_config::config::Config;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

/// How often mdns-sd re-reads this PC's interfaces on its own: a fallback for the interface and
/// address change notifications below, which have them re-read within a second of a change
/// instead, and covers a notification that could not be registered or missed one.
const DISCOVERY_FALLBACK: u32 = 30;
/// How soon after a change the notification has mdns-sd re-read the interfaces.
const DISCOVERY_SOON: u32 = 1;
/// Bound on waiting for the previous daemon's goodbye packets and socket teardown before
/// starting a replacement.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// Pause before retrying a failed restart once Windows finishes settling an interface transition.
const RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct DiscoverySpec {
    bind: IpAddr,
    hostname: String,
    instance: String,
    http_port: u16,
}

fn filter_addresses(bind: IpAddr, addrs: Vec<IpAddr>) -> Vec<IpAddr> {
    let bind = bind.to_canonical();
    let mut filtered: Vec<IpAddr> = addrs
        .into_iter()
        .map(|ip| ip.to_canonical())
        .filter(|ip| {
            if !bind.is_unspecified() {
                *ip == bind
            } else if bind.is_ipv4() {
                ip.is_ipv4()
            } else {
                true
            }
        })
        .collect();
    filtered.sort();
    filtered.dedup();
    filtered
}

fn current_addresses(bind: IpAddr) -> Option<Vec<IpAddr>> {
    butterpollo_win_io::net::unicast_addresses()
        .ok()
        .map(|addrs| filter_addresses(bind, addrs))
}

trait DaemonBackend {
    type Daemon;
    fn start(&self) -> Result<Self::Daemon>;
    fn set_interval(&self, daemon: &Self::Daemon, secs: u32);
    fn shutdown(&self, daemon: Self::Daemon);
    fn addresses(&self) -> Option<Vec<IpAddr>>;
    fn retry_pause(&self);
}

struct MdnsBackend {
    spec: DiscoverySpec,
}

impl DaemonBackend for MdnsBackend {
    type Daemon = ServiceDaemon;

    fn start(&self) -> Result<Self::Daemon> {
        let daemon = ServiceDaemon::new()?;
        let result = (|| -> Result<()> {
            daemon.set_ip_check_interval(DISCOVERY_FALLBACK)?;
            daemon.disable_interface(IfKind::LoopbackV4)?;
            daemon.disable_interface(IfKind::LoopbackV6)?;
            if self.spec.bind.is_ipv4() {
                daemon.disable_interface(IfKind::IPv6)?;
            }
            if !self.spec.bind.is_unspecified() {
                daemon.disable_interface(IfKind::All)?;
                daemon.enable_interface(IfKind::Addr(self.spec.bind))?;
            }
            let addresses = if self.spec.bind.is_unspecified() {
                String::new()
            } else {
                self.spec.bind.to_string()
            };
            let service = ServiceInfo::new(
                "_nvstream._tcp.local.",
                &self.spec.instance,
                &self.spec.hostname,
                addresses.as_str(),
                self.spec.http_port,
                None,
            )?;
            let service = if self.spec.bind.is_unspecified() {
                service.enable_addr_auto()
            } else {
                service
            };
            daemon.register(service)?;
            tracing::debug!(instance = %self.spec.instance, "Moonlight discovery registered");
            Ok(())
        })();
        if let Err(error) = result {
            if let Ok(done) = daemon.shutdown() {
                let _ = done.recv_timeout(SHUTDOWN_WAIT);
            }
            return Err(error);
        }
        Ok(daemon)
    }

    fn set_interval(&self, daemon: &Self::Daemon, secs: u32) {
        let _ = daemon.set_ip_check_interval(secs);
    }

    fn shutdown(&self, daemon: Self::Daemon) {
        if let Ok(done) = daemon.shutdown() {
            let _ = done.recv_timeout(SHUTDOWN_WAIT);
        }
    }

    fn addresses(&self) -> Option<Vec<IpAddr>> {
        current_addresses(self.spec.bind)
    }

    fn retry_pause(&self) {
        std::thread::sleep(RETRY_DELAY);
    }
}

struct Reloader<B: DaemonBackend> {
    backend: B,
    daemon: Option<B::Daemon>,
    addresses: Option<Vec<IpAddr>>,
    instance: String,
}

impl<B: DaemonBackend> Reloader<B> {
    fn on_changed(&mut self) {
        if let Some(daemon) = &self.daemon {
            self.backend.set_interval(daemon, DISCOVERY_SOON);
        }
    }

    fn on_quiet(&mut self) {
        let Some(now_addrs) = self.backend.addresses() else {
            if let Some(daemon) = &self.daemon {
                self.backend.set_interval(daemon, DISCOVERY_FALLBACK);
            }
            return;
        };
        if self.daemon.is_some() && self.addresses.as_ref() == Some(&now_addrs) {
            if let Some(daemon) = &self.daemon {
                self.backend.set_interval(daemon, DISCOVERY_FALLBACK);
            }
            return;
        }
        if now_addrs.is_empty() {
            self.addresses = Some(now_addrs);
            if let Some(daemon) = &self.daemon {
                self.backend.set_interval(daemon, DISCOVERY_FALLBACK);
            }
            return;
        }
        // Shut the previous daemon down completely before starting its replacement so goodbye
        // packets never follow the new announcement.
        if let Some(old) = self.daemon.take() {
            self.backend.shutdown(old);
        }
        let started = self.backend.start().or_else(|_| {
            self.backend.retry_pause();
            self.backend.start()
        });
        match started {
            Ok(fresh) => {
                self.daemon = Some(fresh);
                self.addresses = Some(now_addrs);
                tracing::info!(
                    instance = %self.instance,
                    addresses = ?self.addresses.as_deref().unwrap_or(&[]),
                    "restarted Moonlight discovery after network address change"
                );
            }
            Err(error) => {
                self.addresses = None;
                tracing::warn!(
                    instance = %self.instance,
                    %error,
                    "could not restart Moonlight discovery after network address change"
                );
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(old) = self.daemon.take() {
            self.backend.shutdown(old);
        }
    }
}

pub struct Discovery {
    /// Windows' interface change notification: `None` when Windows refused it, and the daemon's
    /// own interval is the fallback. Dropped before the reloader shuts down, so its worker thread
    /// cannot race or wake a shut-down daemon.
    watch: Option<butterpollo_win_io::net::InterfaceChanges>,
    reloader: Arc<Mutex<Reloader<MdnsBackend>>>,
}

impl Discovery {
    pub fn start(config: &Config, bind: IpAddr) -> Result<Option<Self>> {
        if !config.boolean("enable_discovery", true) || bind.is_loopback() {
            return Ok(None);
        }
        let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "butterpollo".into());
        let hostname = format!(
            "{}.local.",
            hostname.trim_end_matches('.').to_ascii_lowercase()
        );
        let instance = super::host_name(config);
        let spec = DiscoverySpec {
            bind,
            hostname,
            instance: instance.clone(),
            http_port: config.ports()?.http,
        };
        let backend = MdnsBackend { spec };
        let daemon = backend.start()?;
        let initial_addrs = backend.addresses();
        let reloader = Arc::new(Mutex::new(Reloader {
            backend,
            daemon: Some(daemon),
            addresses: initial_addrs,
            instance,
        }));
        let watch = butterpollo_win_io::net::InterfaceChanges::register(
            {
                let reloader = Arc::clone(&reloader);
                move || {
                    reloader
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .on_changed();
                }
            },
            {
                let reloader = Arc::clone(&reloader);
                move || {
                    reloader
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .on_quiet();
                }
            },
        );
        if watch.is_none() {
            tracing::warn!(
                "interface change notifications are unavailable; Moonlight discovery re-reads the interfaces on its own timer instead"
            );
        }
        Ok(Some(Self { watch, reloader }))
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // Drop the interface watcher first so its thread finishes before locking and shutting down
        // the daemon.
        self.watch = None;
        self.reloader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Start(u32),
        Interval(u32, u32),
        Shutdown(u32),
        RetryPause,
    }

    struct FakeBackend {
        next_id: RefCell<u32>,
        addrs: RefCell<Option<Vec<IpAddr>>>,
        start_results: RefCell<VecDeque<Result<()>>>,
        events: RefCell<Vec<Event>>,
    }

    impl FakeBackend {
        fn new(addrs: Option<Vec<IpAddr>>) -> Self {
            Self {
                next_id: RefCell::new(1),
                addrs: RefCell::new(addrs),
                start_results: RefCell::new(VecDeque::new()),
                events: RefCell::new(Vec::new()),
            }
        }
    }

    impl DaemonBackend for FakeBackend {
        type Daemon = u32;

        fn start(&self) -> Result<u32> {
            if let Some(Err(error)) = self.start_results.borrow_mut().pop_front() {
                return Err(error);
            }
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            self.events.borrow_mut().push(Event::Start(id));
            Ok(id)
        }

        fn set_interval(&self, daemon: &u32, secs: u32) {
            self.events
                .borrow_mut()
                .push(Event::Interval(*daemon, secs));
        }

        fn shutdown(&self, daemon: u32) {
            self.events.borrow_mut().push(Event::Shutdown(daemon));
        }

        fn addresses(&self) -> Option<Vec<IpAddr>> {
            self.addrs.borrow().clone()
        }

        fn retry_pause(&self) {
            self.events.borrow_mut().push(Event::RetryPause);
        }
    }

    fn ips(list: &[&str]) -> Option<Vec<IpAddr>> {
        Some(list.iter().map(|s| s.parse().unwrap()).collect())
    }

    #[test]
    fn unchanged_addresses_restore_fallback_without_restarting() {
        let initial = ips(&["172.16.225.121"]);
        let backend = FakeBackend::new(initial.clone());
        let daemon = backend.start().unwrap();
        let mut reloader = Reloader {
            backend,
            daemon: Some(daemon),
            addresses: initial,
            instance: "test".into(),
        };
        reloader.on_changed();
        reloader.on_quiet();
        assert_eq!(reloader.daemon, Some(1));
        assert_eq!(
            *reloader.backend.events.borrow(),
            vec![
                Event::Start(1),
                Event::Interval(1, DISCOVERY_SOON),
                Event::Interval(1, DISCOVERY_FALLBACK),
            ]
        );
    }

    #[test]
    fn hotspot_disconnect_then_new_subnet_restarts_after_shutdown() {
        let backend = FakeBackend::new(ips(&["172.16.225.121"]));
        let daemon = backend.start().unwrap();
        let mut reloader = Reloader {
            backend,
            daemon: Some(daemon),
            addresses: ips(&["172.16.225.121"]),
            instance: "test".into(),
        };

        // Disconnect clears local addresses without spawning a premature restart.
        *reloader.backend.addrs.borrow_mut() = Some(Vec::new());
        reloader.on_changed();
        reloader.on_quiet();
        assert_eq!(reloader.daemon, Some(1));
        assert_eq!(reloader.addresses, Some(Vec::new()));

        // Reconnect on the new Android hotspot subnet shuts down old before starting new.
        *reloader.backend.addrs.borrow_mut() = ips(&["192.168.186.121"]);
        reloader.on_changed();
        reloader.on_quiet();
        assert_eq!(reloader.daemon, Some(2));
        assert_eq!(reloader.addresses, ips(&["192.168.186.121"]));
        assert_eq!(
            *reloader.backend.events.borrow(),
            vec![
                Event::Start(1),
                Event::Interval(1, DISCOVERY_SOON),
                Event::Interval(1, DISCOVERY_FALLBACK),
                Event::Interval(1, DISCOVERY_SOON),
                Event::Shutdown(1),
                Event::Start(2),
            ]
        );
    }

    #[test]
    fn direct_subnet_switch_restarts_after_shutdown() {
        let backend = FakeBackend::new(ips(&["172.16.225.121"]));
        let daemon = backend.start().unwrap();
        let mut reloader = Reloader {
            backend,
            daemon: Some(daemon),
            addresses: ips(&["172.16.225.121"]),
            instance: "test".into(),
        };

        *reloader.backend.addrs.borrow_mut() = ips(&["172.31.225.121"]);
        reloader.on_changed();
        reloader.on_quiet();
        assert_eq!(reloader.daemon, Some(2));
        assert_eq!(reloader.addresses, ips(&["172.31.225.121"]));
        assert_eq!(
            *reloader.backend.events.borrow(),
            vec![
                Event::Start(1),
                Event::Interval(1, DISCOVERY_SOON),
                Event::Shutdown(1),
                Event::Start(2),
            ]
        );
    }

    #[test]
    fn start_failure_clears_addresses_and_recovers_on_next_change() {
        let backend = FakeBackend::new(ips(&["172.16.225.121"]));
        let daemon = backend.start().unwrap();
        let mut reloader = Reloader {
            backend,
            daemon: Some(daemon),
            addresses: ips(&["172.16.225.121"]),
            instance: "test".into(),
        };

        *reloader.backend.addrs.borrow_mut() = ips(&["192.168.186.121"]);
        reloader.backend.start_results.borrow_mut().extend([
            Err(anyhow!("busy 1")),
            Err(anyhow!("busy 2")),
            Ok(()),
        ]);
        reloader.on_quiet();
        assert_eq!(reloader.daemon, None);
        assert_eq!(reloader.addresses, None);

        // Next quiet event with the same subnet recovers because addresses were cleared.
        reloader.on_quiet();
        assert_eq!(reloader.daemon, Some(2));
        assert_eq!(reloader.addresses, ips(&["192.168.186.121"]));
        assert_eq!(
            *reloader.backend.events.borrow(),
            vec![
                Event::Start(1),
                Event::Shutdown(1),
                Event::RetryPause,
                Event::Start(2),
            ]
        );
    }

    #[test]
    fn filter_addresses_respects_bind_canonicalizes_and_dedups() {
        let raw: Vec<IpAddr> = vec![
            "192.168.4.2".parse().unwrap(),
            "::ffff:192.168.4.2".parse().unwrap(),
            "172.31.225.121".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
        ];
        assert_eq!(
            Some(filter_addresses("0.0.0.0".parse().unwrap(), raw.clone())),
            ips(&["172.31.225.121", "192.168.4.2"])
        );
        assert_eq!(
            Some(filter_addresses("::".parse().unwrap(), raw.clone())),
            ips(&["172.31.225.121", "192.168.4.2", "2001:db8::1"])
        );
        assert_eq!(
            Some(filter_addresses("172.31.225.121".parse().unwrap(), raw)),
            ips(&["172.31.225.121"])
        );
    }
}
