//! The previous host's frame generation, capture and rational limiter policy.
//! No global config is mutated: each launch resolves its own inherited policy.
use anyhow::{Result, bail};
use butterpollo_config::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate(pub u32);
impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_multiple_of(1000) {
            write!(f, "{}", self.0 / 1000)
        } else {
            write!(f, "{}.{:03}", self.0 / 1000, self.0 % 1000)
        }
    }
}
impl Rate {
    pub fn parse(value: &str) -> Result<Self> {
        let value = butterpollo_config::config::number_text(value);
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || fraction.len() > 3
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            bail!("frame limit must be 0–1000 FPS with at most three decimal places");
        }
        let whole: u32 = whole.parse()?;
        let fraction = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u32>()? * 10u32.pow(3 - fraction.len() as u32)
        };
        let rate = whole
            .checked_mul(1000)
            .and_then(|r| r.checked_add(fraction))
            .filter(|r| *r <= 1_000_000)
            .ok_or_else(|| anyhow::anyhow!("frame limit is outside 0–1000 FPS"))?;
        Ok(Self(rate))
    }
    pub fn from_client(raw: u32) -> Self {
        Self(if raw < 1000 {
            raw.saturating_mul(1000)
        } else {
            raw
        })
    }
    pub fn rounded(self) -> u32 {
        self.0.saturating_add(500) / 1000
    }
    pub fn rational(self) -> (u32, u32) {
        let (mut a, mut b) = (self.0, 1000);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        (self.0 / a, 1000 / a)
    }
    pub fn period(self) -> std::time::Duration {
        std::time::Duration::from_nanos(1_000_000_000_000 / u64::from(self.0.max(1)))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    None,
    Auto,
    Rtss,
}
impl Provider {
    pub fn parse(value: &str) -> Self {
        match normalize(value).as_str() {
            "none" | "disabled" => Self::None,
            "rtss" => Self::Rtss,
            _ => Self::Auto,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Rtss => "rtss",
        }
    }
}
/// A `frame_limiter_provider` naming the removed NVIDIA driver limiter.
pub use butterpollo_config::config::retired_frame_limiter_provider as retired_provider;
/// RTSS's limiter modes (`SyncLimiter` in its profiles): asynchronous, front edge sync, back
/// edge sync, and NVIDIA Reflex, which only an NVIDIA GPU runs.
pub const SYNC_FRONT_EDGE: u32 = 1;
pub const SYNC_REFLEX: u32 = 3;
/// An `rtss_frame_limit_type` naming RTSS's NVIDIA Reflex mode.
pub fn is_reflex(value: &str) -> bool {
    matches!(normalize(value).as_str(), "nvidiareflex" | "reflex")
}
/// An app's frame generation naming the NVIDIA driver's Smooth Motion.
pub use butterpollo_config::config::retired_frame_generation as retired_generation;
/// The configured frame limiting Moonlight extensions report in serverinfo:
/// whether limiting is on by default, whether virtual displays limit
/// automatically, and the manual limit in millihertz.
pub fn advertised(config: &Config, virtual_display_enabled: bool) -> (bool, bool, u32) {
    let virtual_limiter = virtual_refresh(config) != VirtualRefresh::Disabled;
    let manual = config.boolean("frame_limiter_enable", false)
        && Provider::parse(config.get("frame_limiter_provider", "auto")) != Provider::None;
    let limit = Rate::parse(config.get("frame_limiter_fps_limit", "0")).map_or(0, |r| r.0);
    (
        manual || (virtual_display_enabled && virtual_limiter),
        virtual_limiter,
        limit,
    )
}
fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}
/// How a virtual display's refresh follows the stream
/// (`virtual_display_refresh`, `frame_limiter_auto_virtual_framegen` before).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualRefresh {
    /// Twice the stream rate.
    Legacy,
    /// Four times the stream rate.
    Enabled,
    /// A fixed 1000 Hz.
    Vrr,
    Disabled,
}
/// The setting with Vibepollo's spellings. The default, and the fallback for a value it
/// does not know, is twice the stream rate. The fixed 1000 Hz is offered because a
/// game capped at the stream rate then waits at most 1 ms for a refresh, so its frames
/// keep the cap's even spacing instead of landing in alternate refresh slots as the cap's
/// timer drifts against the display's (rust/PERFORMANCE.md, November 16). A display that
/// refuses its refresh falls back to the fastest refresh it offers below it.
pub fn virtual_refresh(config: &Config) -> VirtualRefresh {
    let value = config.get("virtual_display_refresh", "legacy");
    match normalize(value).as_str() {
        "" | "legacy" | "2x" | "fixed2x" => VirtualRefresh::Legacy,
        "vrr" | "1000hz" | "1000" | "fixed1000hz" => VirtualRefresh::Vrr,
        "false" | "no" | "disable" | "disabled" | "off" | "0" => VirtualRefresh::Disabled,
        "enabled" | "enable" | "true" | "yes" | "on" | "1" | "smooth" | "smoother" => {
            VirtualRefresh::Enabled
        }
        _ => {
            butterpollo_config::config::invalid("virtual_display_refresh", value);
            VirtualRefresh::Legacy
        }
    }
}
/// The fastest refresh a display mode may have (`Config`'s display-mode limits, the virtual
/// display driver's).
const FASTEST_REFRESH: u32 = 1_000_000;
/// The refresh of the stream's display. A multiple of the stream rate stops at
/// [`FASTEST_REFRESH`]: twice 540 fps asked for a mode no display takes, and the stream did
/// not start.
pub fn display_rate(rate: Rate, virtual_display: bool, mode: VirtualRefresh) -> Rate {
    match (virtual_display, mode) {
        (true, VirtualRefresh::Legacy) => Rate(rate.0.saturating_mul(2).min(FASTEST_REFRESH)),
        (true, VirtualRefresh::Enabled) => Rate(rate.0.saturating_mul(4).min(FASTEST_REFRESH)),
        (true, VirtualRefresh::Vrr) => Rate(FASTEST_REFRESH),
        _ => rate,
    }
}
/// Whether a display refreshing at `display` leaves room for a frame cap at `cap`: at least
/// one and a half times the cap. The automatic cap of a virtual display counts on a refresh
/// far above the stream's rate (twice, four times or 1000 Hz), where a capped frame is shown
/// within a fraction of its period. A game in exclusive fullscreen sets the display to a mode
/// of its own, often the stream's rate. A cap at the display's refresh drifts against it,
/// and each time the two cross, frames miss their refresh and every other one is shown twice
/// for a second or two: there the display's own refresh has to pace the game.
pub fn cap_suits_display(cap: Rate, display: Rate) -> bool {
    u64::from(display.0) * 2 >= u64::from(cap.0) * 3
}
pub fn generation_provider(value: &str) -> &'static str {
    match normalize(value).as_str() {
        "game" | "gameprovided" | "gameprovider" => "game-provided",
        _ => "none",
    }
}
#[derive(Clone, Debug)]
pub struct Policy {
    pub rate: Rate,
    pub display_rate: Rate,
    pub enabled: bool,
    /// Whether the cap is the automatic one of a virtual display and nothing the user asked
    /// for (`frame_limiter_enable`): it stands down while the display runs a mode too slow
    /// for it ([`cap_suits_display`]).
    pub follows_display: bool,
    pub provider: Provider,
    pub sync_limiter: u32,
    /// Whether the stream chose `sync_limiter` itself (virtual displays and frame generation)
    /// rather than the RTSS limiter mode setting: [`Policy::for_gpu`] makes it Reflex on NVIDIA.
    pub automatic_sync: bool,
    pub capture: String,
    /// Whether the capture republishes its last picture every stream period
    /// (`capture = wgcc`, Vibepollo's constant-rate WGC variant): WGC, plus a
    /// repeat when nothing was composed for the slot.
    pub constant_rate: bool,
}
impl Policy {
    pub fn resolve(
        config: &Config,
        stream: Rate,
        virtual_display: bool,
        generation: &str,
        generation_enabled: bool,
        runtime_sync: bool,
    ) -> Result<Self> {
        let generation = generation_provider(generation);
        let framegen = generation_enabled || generation != "none";
        let mode = virtual_refresh(config);
        let automatic = virtual_display && mode != VirtualRefresh::Disabled;
        let provider = Provider::parse(config.get("frame_limiter_provider", "auto"));
        let overridden = Rate::parse(config.get("frame_limiter_fps_limit", "0"))?;
        let rate = if overridden.0 > 0 { overridden } else { stream };
        let explicit = config.boolean("frame_limiter_enable", false);
        let allow_sync_override = config.boolean("rtss_allow_virtual_display_override", false);
        let auto_sync = automatic && (!allow_sync_override || generation == "game-provided");
        let automatic_sync = !runtime_sync && (auto_sync || (!virtual_display && framegen));
        let sync = if automatic_sync {
            SYNC_FRONT_EDGE
        } else {
            match normalize(config.get("rtss_frame_limit_type", "async")).as_str() {
                "frontedgesync" => SYNC_FRONT_EDGE,
                "backedgesync" => 2,
                "nvidiareflex" | "reflex" => SYNC_REFLEX,
                _ => 0,
            }
        };
        let (requested_capture, constant_rate) =
            match config.get("capture", "").trim().to_ascii_lowercase() {
                // Vibepollo's constant-rate WGC variant: WGC, and a repeat of
                // the last picture each stream period.
                wgc if wgc == "wgcc" => ("wgc".to_owned(), true),
                // `idd-push`: frames pushed by Rubylight's own virtual display driver.
                // `amd`: AMD Direct Capture, the Radeon driver's presented surface.
                known
                    if matches!(
                        known.as_str(),
                        "" | "auto" | "wgc" | "ddx" | "dxgi" | "idd-push" | "amd"
                    ) =>
                {
                    (known, false)
                }
                other => {
                    butterpollo_config::config::invalid("capture", &other);
                    (String::new(), false)
                }
            };
        let capture = if requested_capture.is_empty() || requested_capture == "auto" {
            // Resolve Auto before opening capture so the normal WGC path,
            // including the service's user helper and DDX fallback, is used.
            "wgc"
        } else {
            &requested_capture
        }
        .to_owned();
        Ok(Self {
            rate,
            display_rate: display_rate(stream, virtual_display, mode),
            enabled: explicit || automatic || (!virtual_display && framegen),
            follows_display: automatic && !explicit,
            provider,
            sync_limiter: sync,
            automatic_sync,
            capture,
            constant_rate,
        })
    }
    /// The RTSS mode for the GPU the stream renders and encodes on (`nvidia`: an NVIDIA GPU).
    /// There the streams that sync the limiter themselves use NVIDIA Reflex, as they did before
    /// 3.0; elsewhere Reflex, which RTSS runs only on NVIDIA, becomes front edge sync, with a
    /// warning.
    pub fn for_gpu(mut self, nvidia: bool) -> Self {
        if nvidia && self.automatic_sync {
            self.sync_limiter = SYNC_REFLEX;
        } else if !nvidia && self.sync_limiter == SYNC_REFLEX {
            tracing::warn!(
                "RTSS limit mode NVIDIA Reflex needs an NVIDIA GPU, and this stream's is not one; front edge sync limits it instead"
            );
            self.sync_limiter = SYNC_FRONT_EDGE;
        }
        self
    }
    pub fn with_vrr(mut self, config: &Config, virtual_display: bool, requested: bool) -> Self {
        if virtual_display && requested && virtual_refresh(config) != VirtualRefresh::Disabled {
            self.display_rate = Rate(1_000_000);
            if matches!(config.get("capture", "auto"), "" | "auto") {
                self.capture = "wgc".into();
            }
        }
        self
    }
    /// The automatic virtual-display cap only when a limiter provider can apply it. RTSS is the
    /// only provider; on a host without it the cap would apply nothing and say so on every
    /// launch, so it stays off and the display's refresh paces the game (Virtual display
    /// refresh). A cap the user asked for (`frame_limiter_enable`, RTSS by name, an app's own
    /// frame limit) stays and fails visibly on the stream's card.
    pub fn with_provider_available(mut self, available: bool) -> Self {
        if !available && self.follows_display && self.provider != Provider::Rtss {
            self.enabled = false;
            self.follows_display = false;
        }
        self
    }
}
#[cfg(test)]
mod tests;
