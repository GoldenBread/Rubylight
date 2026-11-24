//! Fake [`PowerSystem`] implementation for Wake-on-LAN and idle-sleep tests.

use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Called with the method's name before a fake answers.
pub type Hook = Arc<dyn Fn(&Fake, &'static str) + Send + Sync>;

/// A PC as the tests need it; the defaults are the `/wake` fixture's PC.
pub struct Fake {
    pub adapters: Mutex<Result<Vec<Adapter>, String>>,
    /// The default route's source address: the Ethernet adapter's.
    pub route: Mutex<Result<Option<IpAddr>, String>>,
    pub nics: Mutex<Result<NicReport, String>>,
    pub facts: Mutex<Result<SystemFacts, String>>,
    pub execution: Mutex<u32>,
    pub input: Mutex<Input>,
    pub slept: Mutex<Result<Duration, String>>,
    /// How much `sleep` adds to `slept`.
    pub nap: Mutex<Duration>,
    pub hook: Mutex<Option<Hook>>,
    pub nic_reads: AtomicUsize,
    pub inputs: AtomicUsize,
    pub sleeps: AtomicUsize,
    /// What `apply_nics` was given.
    pub plans: Mutex<Vec<FixPlan>>,
    /// What `run_fix` was given.
    pub fixes: Mutex<Vec<FixPlan>>,
    /// Power requests taken, and how many are held now.
    pub holds: AtomicUsize,
    pub held: Arc<AtomicUsize>,
}

/// A fake power request.
struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub const WIRED: &str = "{875511C7-0EF5-45F7-876C-3BE50726AC42}";
pub const WIFI: &str = "{A5234352-A9A6-4D45-8B0B-2A1CA4957AF2}";

impl Fake {
    pub fn new() -> Arc<Self> {
        let adapters = serde_json::from_value(json!([
            {
                "index": 7, "guid": WIRED, "name": "Ethernet",
                "mac": "02:7C:16:B4:13:4F", "permanent": "02:7C:16:B4:13:4F",
                "hardware": true, "connector": true, "medium": "wired", "connected": true,
                "metric": 20, "ipv4": [["192.168.4.20", 24]], "addresses": ["192.168.4.20"]
            },
            {
                "index": 36, "guid": "{9EDBCD36-5287-4EAC-9C2D-BE5640B6FF4C}",
                "name": "vEthernet (WSL)", "mac": "00:15:5D:55:02:28",
                "hardware": false, "connector": false, "medium": "wired", "connected": true,
                "metric": 5000, "ipv4": [["172.30.112.1", 20]], "addresses": ["172.30.112.1"]
            },
            {
                "index": 12, "guid": WIFI, "name": "WLAN", "mac": "02:E0:4C:68:00:01",
                "hardware": true, "connector": true, "medium": "wifi", "connected": false,
                "metric": 25
            }
        ]))
        .unwrap();
        let nics = NicReport::parse(
            &json!({"adapters": [
                {
                    "guid": WIRED, "pnp": "PCI\\VEN_10EC&DEV_8125\\01", "name": "Ethernet",
                    "wake_enable": true, "magic_packet_only": false,
                    "keywords": {"*WakeOnMagicPacket": "1"}, "errors": []
                },
                {
                    "guid": WIFI, "pnp": "PCI\\VEN_8086&DEV_2725\\01", "name": "WLAN",
                    "wake_enable": false, "magic_packet_only": null,
                    "keywords": {"*WakeOnMagicPacket": "0"}, "errors": []
                }
            ]})
            .to_string(),
        )
        .unwrap();
        Arc::new(Self {
            adapters: Mutex::new(Ok(adapters)),
            route: Mutex::new(Ok(Some([192, 168, 4, 20].into()))),
            nics: Mutex::new(Ok(nics)),
            facts: Mutex::new(Ok(SystemFacts {
                s3: true,
                modern_standby: false,
                hibernate: false,
                hiberboot: Some(true),
            })),
            execution: Mutex::new(0),
            input: Mutex::new(Input::Idle(Duration::from_secs(24 * 3600))),
            slept: Mutex::new(Ok(Duration::ZERO)),
            nap: Mutex::new(Duration::ZERO),
            hook: Mutex::new(None),
            nic_reads: AtomicUsize::new(0),
            inputs: AtomicUsize::new(0),
            sleeps: AtomicUsize::new(0),
            plans: Mutex::default(),
            fixes: Mutex::default(),
            holds: AtomicUsize::new(0),
            held: Arc::default(),
        })
    }
    /// Install it in `h`.
    pub fn on(self: &Arc<Self>, h: &Host) -> Arc<Self> {
        h.wake.set_power(self.clone());
        self.clone()
    }
    pub fn sleeps(&self) -> usize {
        self.sleeps.load(Ordering::SeqCst)
    }
    pub fn hook(&self, hook: impl Fn(&Fake, &'static str) + Send + Sync + 'static) {
        *locked(&self.hook) = Some(Arc::new(hook));
    }
    fn call(&self, method: &'static str) {
        let hook = locked(&self.hook).clone();
        if let Some(hook) = hook {
            hook(self, method);
        }
    }
}

impl PowerSystem for Fake {
    fn adapters(&self) -> Result<Vec<Adapter>> {
        locked(&self.adapters).clone().map_err(anyhow::Error::msg)
    }
    fn default_route(&self) -> Result<Option<IpAddr>> {
        locked(&self.route).clone().map_err(anyhow::Error::msg)
    }
    fn read_nics(&self) -> Result<NicReport> {
        self.call("read_nics");
        self.nic_reads.fetch_add(1, Ordering::SeqCst);
        locked(&self.nics).clone().map_err(anyhow::Error::msg)
    }
    fn system_facts(&self) -> Result<SystemFacts> {
        locked(&self.facts).clone().map_err(anyhow::Error::msg)
    }
    fn execution_state(&self) -> Result<u32> {
        self.call("execution_state");
        Ok(*locked(&self.execution))
    }
    fn slept(&self) -> Result<Duration> {
        locked(&self.slept).clone().map_err(anyhow::Error::msg)
    }
    fn input(&self) -> Input {
        self.inputs.fetch_add(1, Ordering::SeqCst);
        locked(&self.input).clone()
    }
    fn sleep(&self) -> Result<()> {
        self.call("sleep");
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        let nap = *locked(&self.nap);
        if let Ok(slept) = locked(&self.slept).as_mut() {
            *slept += nap;
        }
        Ok(())
    }
    fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport> {
        locked(&self.plans).push(plan.clone());
        Ok(ApplyReport {
            restarted: plan
                .adapters
                .iter()
                .filter(|fix| !fix.keywords.is_empty())
                .map(|fix| fix.guid.clone())
                .collect(),
            ..Default::default()
        })
    }
    fn run_fix(&self, plan: &FixPlan) -> Result<Applied> {
        self.call("run_fix");
        locked(&self.fixes).push(plan.clone());
        apply_plan(self, plan)
    }
    fn hold_awake(&self, _: &str) -> Result<Box<dyn Send>> {
        self.holds.fetch_add(1, Ordering::SeqCst);
        self.held.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Held(self.held.clone())))
    }
}
