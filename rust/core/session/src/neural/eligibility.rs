//! Game and folder eligibility, anti-cheat and mod detection, proxy selection,
//! folder ACL verdicts, and switch/status state machine for Neural Rendering.
use super::{
    gpu::Capable,
    manifest::{ALREADY_ON, FG_PROXIES},
};
use serde::Serialize;
use serde_json::{Value, json};

/// How a refusal of a game without DLSS frame generation begins, and what an
/// anti-cheat refusal says: the host tells a device which it was.
pub const NO_DLSS: &str = "the game has no DLSS frame generation";
pub const ANTI_CHEAT: &str = "an anti-cheat treats Neural Rendering's DLL as a cheat";

/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
pub const ADMINISTRATORS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// What has one of the [`super::PROXIES`] names in the game folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occupant {
    Free,
    /// OptiScaler as the host placed it.
    Ours,
    /// Another program's file: a game DLL or another mod's loader.
    Foreign,
    /// An OptiScaler the host did not place.
    OptiScaler,
}

/// The name the full package's OptiScaler loads under: the one the host
/// used before, else the first free of [`FG_PROXIES`]; a game that only uses
/// Vulkan (`vulkan_only`, [`vulkan_only`]) never loads dxgi.dll, so it gets
/// winmm.dll or version.dll. A name another program holds is never taken.
pub fn proxy(
    vulkan_only: bool,
    occupant: impl Fn(&str) -> Occupant,
) -> Result<&'static str, String> {
    let names: &[&'static str] = if vulkan_only {
        &FG_PROXIES[1..]
    } else {
        &FG_PROXIES
    };
    let occupants: Vec<_> = names.iter().map(|name| (*name, occupant(name))).collect();
    occupants
        .iter()
        .find(|(_, o)| *o == Occupant::Ours)
        .or_else(|| occupants.iter().find(|(_, o)| *o == Occupant::Free))
        .map(|(name, _)| *name)
        .ok_or_else(|| {
            format!(
                "other programs' files hold every name OptiScaler can load under ({})",
                names.join(", ")
            )
        })
}

/// Whether a program's bytes name `name` (ASCII lower-case) in ASCII or
/// UTF-16, whatever its case.
fn names(bytes: &[u8], name: &str) -> bool {
    let wide: Vec<u8> = name.bytes().flat_map(|b| [b, 0]).collect();
    let matches = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    };
    matches(name.as_bytes()) || matches(&wide)
}

/// Whether a game's program uses Vulkan and never DXGI: it names
/// vulkan-1.dll and none of dxgi.dll, d3d11.dll and d3d12.dll, as the
/// release's installer tells them.
pub fn vulkan_only(program: &[u8]) -> bool {
    names(program, "vulkan-1.dll")
        && !["dxgi.dll", "d3d11.dll", "d3d12.dll"]
            .iter()
            .any(|name| names(program, name))
}

/// Whether an OptiScaler DLL can load a frame generation provider from its
/// `custom` folder: OptiScaler builds that can log "Custom DLSSG {}
/// initialized". AMDNR's fork and older OptiScaler releases cannot.
pub fn custom_providers(dll: &[u8]) -> bool {
    dll.windows(12).any(|window| window == b"Custom DLSSG")
}

/// Whether a DLL's bytes identify it as an OptiScaler binary.
pub fn is_optiscaler_binary(bytes: &[u8]) -> bool {
    bytes.windows(10).any(|w| w == b"OptiScaler")
}

/// Whether a file is a sign that a game has DLSS frame generation for the
/// provider to take the place of: NVIDIA's DLSS frame generation DLL or
/// Streamline's plugin for it.
pub fn dlss_frame_generation(name: &str) -> bool {
    ["nvngx_dlssg.dll", "sl.dlss_g.dll"]
        .iter()
        .any(|file| name.eq_ignore_ascii_case(file))
}

/// Another mod in the game's folder that replaces the same calls (another
/// OptiScaler, or an upscaler or frame-generation replacement), by one of its
/// file names. ReShade and other loaders that only share a DLL name are not
/// in the list: OptiScaler takes another name beside them.
pub fn other_mod(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        "optiscaler.dll" | "optiscaler.ini" | "optiscaler.asi" | "optiscaler" | "nvngx.ini" => {
            "OptiScaler"
        }
        "fakenvapi.dll" | "fakenvapi.ini" => "fakenvapi (an OptiScaler add-on)",
        "specialk64.dll" | "specialk32.dll" => "Special K",
        _ if starts("dlss-enabler") => "DLSS Enabler",
        _ if starts("dlssg_to_fsr3") => "dlssg-to-fsr3",
        _ if starts("uniscaler") => "Uniscaler",
        _ => return None,
    })
}

/// The anti-cheat a file or folder name in a game's install belongs to.
/// OptiScaler injects a DLL into the game, which kernel and online
/// anti-cheat treat as cheating; such games are refused.
pub fn anti_cheat(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        _ if starts("easyanticheat")
            || starts("eac_launcher")
            || lower == "start_protected_game.exe" =>
        {
            "Easy Anti-Cheat"
        }
        "battleye" => "BattlEye",
        _ if starts("beservice") || starts("beclient") || lower.ends_with("_be.exe") => "BattlEye",
        "vgc.exe" | "vgk.sys" | "riot vanguard" => "Riot Vanguard",
        _ if lower.contains("randgrid") => "Ricochet",
        _ if starts("mhyprot") => "mhyprot",
        "gameguard" | "nprotect" | "gameguard.des" => "nProtect GameGuard",
        _ if starts("npgg") && lower.ends_with(".des") => "nProtect GameGuard",
        "x3.xem" => "XIGNCODE3",
        _ if starts("xigncode") || starts("xhunter") => "XIGNCODE3",
        "pbsvc.exe" | "punkbuster" => "PunkBuster",
        _ if starts("pnkbstr") => "PunkBuster",
        _ if starts("equ8") => "EQU8",
        _ if starts("faceit") => "FACEIT",
        "anticheatexpert" => "Anti-Cheat Expert",
        _ if starts("sguard") || starts("ace-base") => "Anti-Cheat Expert",
        _ if starts("eaanticheat") => "EA Javelin",
        _ if lower.contains("denuvo") && lower.contains("anti") => "Denuvo Anti-Cheat",
        _ if starts("neacsafe") || starts("neaccontroller") => "NetEase Anti-Cheat",
        "hshield" | "ehsvc.dll" => "AhnLab HackShield",
        _ => return None,
    })
}

/// Classify a program filename that is a store/library launcher or script host
/// / interpreter rather than a game's own executable.
pub fn classify_non_game_exe(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    let launcher = [
        "steam",
        "upc",
        "ubisoftconnect",
        "battle.net",
        "eadesktop",
        "origin",
        "galaxyclient",
        "amazon games",
        "amazon games ui",
        "itch",
        "heroic",
        "xboxpcapp",
        "gamelaunchhelper",
    ]
    .contains(&stem)
        || stem.contains("launcher")
        || stem.starts_with("playnite.");
    let interpreter = [
        "cmd",
        "powershell",
        "powershell_ise",
        "pwsh",
        "wscript",
        "cscript",
        "mshta",
        "rundll32",
        "conhost",
        "explorer",
        "py",
        "pyw",
        "node",
        "java",
        "javaw",
        "ruby",
        "rubyw",
        "perl",
        "php",
        "bash",
        "sh",
        "wsl",
        "dotnet",
    ]
    .contains(&stem)
        || stem.starts_with("python");
    if launcher {
        Some("a launcher")
    } else if interpreter {
        Some("a script host or interpreter")
    } else {
        None
    }
}

/// Whether a lower-cased top-level `.exe` name is a crash reporter, setup,
/// redistributable, updater, or helper rather than the game's executable.
pub fn is_helper_program(lower_name: &str) -> bool {
    [
        "crash",
        "report",
        "launcher",
        "unins",
        "setup",
        "install",
        "redist",
        "vcredist",
        "vc_redist",
        "dxsetup",
        "dotnet",
        "helper",
        "updater",
        "anticheat",
        "_be.exe",
        "beservice",
        "start_protected",
        "cefprocess",
        "webhelper",
        "touchup",
        "cleanup",
    ]
    .iter()
    .any(|part| lower_name.contains(part))
}

/// Whether a folder name is a shared container holding multiple programs.
pub fn is_shared_folder_name(name: &str) -> bool {
    ["common", "Program Files", "Program Files (x86)"]
        .iter()
        .any(|other| name.eq_ignore_ascii_case(other))
}

/// Who may change a folder, as [`security_verdict`] judges it.
#[derive(Debug)]
pub struct Security {
    pub link: bool,
    pub owner: String,
    /// Those its permissions give the [`Rights`] asked about.
    pub writers: Vec<String>,
}

/// What a folder ACL query asks who may do to a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rights {
    /// Change it or what it holds.
    Change,
    /// Move or delete what it holds, as for a drive's root, which lets
    /// users add folders, which moves nothing.
    MoveContents,
    /// That, or move or delete the folder itself, as for a folder above the
    /// cache: C:\ProgramData lets users add folders too.
    Move,
}

/// Whether only administrators may change `path_display`, resolving unknown
/// SIDs via `account_name`.
pub fn security_verdict(
    path_display: &str,
    security: &Security,
    account_name: impl Fn(&str) -> String,
) -> Result<(), String> {
    if security.link {
        return Err(format!("{path_display} is a link to elsewhere"));
    }
    if !ADMINISTRATORS.contains(&security.owner.as_str()) {
        return Err(format!(
            "{path_display} is owned by {}, not by SYSTEM or Administrators",
            account_name(&security.owner)
        ));
    }
    match security
        .writers
        .iter()
        .find(|sid| !ADMINISTRATORS.contains(&sid.as_str()))
    {
        Some(other) => Err(format!(
            "{path_display} can be changed by {}",
            account_name(other)
        )),
        None => Ok(()),
    }
}

/// `missing` less the runtime files in `found`.
pub fn without_runtime(missing: Vec<&'static str>, found: &[&'static str]) -> Vec<&'static str> {
    missing
        .into_iter()
        .filter(|file| !found.contains(file))
        .collect()
}

/// What a game's own check found before any switch: what the install asks
/// of the game itself, without the download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameCheck {
    Fits,
    /// Refused: `why` is the code a device says in its words
    /// (`anti_cheat`, `no_frame_generation`, `game`).
    Refused {
        why: &'static str,
        reason: String,
    },
    /// The host can't tell which program is the game's.
    Choose(String),
}

/// The code of a game's refusal ([`GameCheck::Refused`]).
pub fn game_why(reason: &str) -> &'static str {
    if reason.starts_with(NO_DLSS) {
        "no_frame_generation"
    } else if reason.contains(ANTI_CHEAT) {
        "anti_cheat"
    } else {
        "game"
    }
}

/// What a switch is doing for one app, or how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Job {
    pub action: &'static str,
    /// `download`, `place`, `remove`, `done` or `failed`.
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The game's .exe could not be found: the user picks it.
    pub choose_exe: bool,
    /// Placed files that changed since and stayed in the folder.
    pub kept: Vec<String>,
}

impl Job {
    /// Start a fresh job in `phase` with zero progress.
    pub fn new(action: &'static str, phase: &'static str) -> Self {
        Self {
            action,
            phase,
            done: 0,
            total: 0,
            error: None,
            choose_exe: false,
            kept: Vec::new(),
        }
    }

    /// Whether the job is still running (neither `done` nor `failed`).
    pub fn is_running(&self) -> bool {
        !matches!(self.phase, "done" | "failed")
    }
}

/// Why the switch cannot be turned on or off now.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotAccepted,
    Unavailable(String),
    NotFound,
    Busy,
    /// A stream runs. Turning on, the download would take its bandwidth and
    /// the copy into the game's folder its disk; turning off, the streamed
    /// game may hold the provider's DLL loaded, which can't be deleted.
    Streaming,
}

/// How a removal from a game folder ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Removed {
    /// The files that changed since, or that someone else put in the
    /// folders the host created, and stayed.
    Done(Vec<String>),
    /// The folder's drive or share is not there now.
    Unreachable(String),
}

/// Pure admission checks for turning Neural Rendering on for an app.
pub fn enable_preconditions(
    accepted: bool,
    capability: Result<(), String>,
    app_found: bool,
    already_placed: bool,
    streaming: bool,
) -> Result<(), Refusal> {
    if !accepted {
        return Err(Refusal::NotAccepted);
    }
    capability.map_err(Refusal::Unavailable)?;
    if !app_found {
        return Err(Refusal::NotFound);
    }
    if already_placed {
        return Err(Refusal::Unavailable(ALREADY_ON.into()));
    }
    if streaming {
        return Err(Refusal::Streaming);
    }
    Ok(())
}

/// Whether a status read should run the background per-game check: only when
/// idle, not streaming, the PC has a capable GPU, and the switch is off.
pub fn should_run_game_check(idle: bool, streaming: bool, gpu_ok: bool, enabled: bool) -> bool {
    idle && !streaming && gpu_ok && !enabled
}

/// Pure inputs to build the console/Nightjar Neural Rendering status payload.
pub struct StatusInput<'a> {
    pub accepted: bool,
    pub capability: Result<&'a Capable, (&'static str, &'a str)>,
    pub found: bool,
    pub setting: Option<&'a Value>,
    pub legacy: Option<&'a Value>,
    pub game: Option<&'a GameCheck>,
    pub job: Option<&'a Job>,
}

/// Build the console and Nightjar status JSON payload for one app.
pub fn status_json(input: StatusInput<'_>) -> Value {
    json!({
        "accepted": input.accepted,
        "available": input.capability.is_ok(),
        "gpu": input.capability.ok().map(|capable| &capable.gpu),
        "gpu_note": input.capability.ok().and_then(Capable::note),
        "reason": input.capability.err().map(|(_, reason)| reason),
        // Which half failed: `gpu` or `runtime` (the Visual C++ runtime).
        "unavailable": input.capability.err().map(|(why, _)| why),
        "found": input.found,
        "enabled": input.setting.is_some(),
        "exe": input.setting.and_then(|s| s.get("exe")).cloned(),
        "release": input.setting.and_then(|s| s.get("release")).cloned(),
        "mode": input.setting.and_then(|s| s.get("mode")).cloned(),
        "game": match input.game {
            Some(GameCheck::Refused { why, reason }) => json!({"why": why, "reason": reason}),
            Some(GameCheck::Choose(reason)) => json!({"why": "choose_program", "reason": reason}),
            Some(GameCheck::Fits) | None => Value::Null,
        },
        "earlier_upscaling": input.legacy.map(|legacy| json!({
            "exe": legacy.get("exe"),
            "release": legacy.get("release"),
        })),
        "job": input.job,
    })
}
