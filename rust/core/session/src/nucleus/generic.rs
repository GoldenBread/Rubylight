//! Rubylight's own one-player Nucleus Co-op handler, for background streams
//! of games no hub handler covers.
//!
//! Nucleus Co-op is the only way a game streams in the background, and it
//! starts a game only through a handler. For a one-player game without one
//! the host writes this handler into Nucleus' `handlers` folder and adds the
//! game to Nucleus' game list `content\userprofile.json`, the way Nucleus'
//! own "add game" does (2.4.2 reads no other list). Co-op still needs a hub handler, and a hub handler for the same
//! executable always wins: [`super::handler::Catalog`] skips ours.
//!
//! Every `Game.*` option below is one Nucleus Co-op 2.4.2 reads
//! (`GenericGameInfo.cs`, `GameHookInfo.cs`, `ProtoInputOptions.cs`).
use sha2::{Digest, Sha256};

mod checks;
mod userprofile;

pub use checks::{
    anti_cheat, epic_arguments, epic_exit, launcher, on_this_pc, refused_path, unreal_shipping,
};
#[cfg(test)]
use checks::{anti_cheat_of, shared_folder};
#[cfg(all(test, windows))]
use checks::stub_name;
pub use userprofile::{add_game, remove_game};

/// The start of our handlers' GUIDs and file names.
pub const PREFIX: &str = "Rubylight-";
/// The first line of every handler Rubylight writes: the file is ours to
/// rewrite. A file without it is never touched.
pub const MARKER: &str = "// Rubylight one-player handler";
/// The line after [`MARKER`] that names the account that wrote the handler
/// (its SID): only that account's sweep removes it.
pub const OWNER: &str = "// Written for ";
/// Bumped when the template changes; a handler with another version is
/// rewritten.
const VERSION: u32 = 3;

/// Whether a handler's text is one Rubylight wrote.
pub fn owns(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').starts_with(MARKER)
}

fn normalized(path: &str) -> String {
    path.trim().replace('/', "\\")
}
fn parent(path: &str) -> &str {
    path.rfind('\\').map_or("", |at| &path[..at])
}
fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}
/// `path` below `folder`, as a relative path; None when it is not below.
fn below<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    let folder = folder.trim_end_matches('\\');
    let rest = path.get(folder.len()..)?;
    (!folder.is_empty() && path[..folder.len()].eq_ignore_ascii_case(folder))
        .then_some(rest)
        .and_then(|rest| rest.strip_prefix('\\'))
}

/// The game a handler is written for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The game's .exe, as the app starts it.
    pub exe_path: String,
    /// The app's name, which Nucleus shows.
    pub name: String,
    /// The game's folder, which [`anti_cheat`] looks through: its Steam
    /// install folder, an Unreal game's folder above `<Project>\Binaries`
    /// (the scan stops at `<Project>` when no `Engine` is beside it), else
    /// the .exe's.
    pub root: String,
    /// The Steam app the game was found from. Nucleus Co-op starts with it
    /// as `SteamAppId`, so the game's .exe runs as Steam would start it
    /// rather than restarting through Steam.
    pub steam_app: Option<u32>,
    /// The anti-cheat the game is known to ship from elsewhere than its
    /// files: Steam's Valve Anti-Cheat flag.
    pub anti_cheat: Option<&'static str>,
    /// What the game starts with after its .exe: the app's arguments, or
    /// its Steam launch option's (Portal is `hl2.exe -game portal`).
    pub arguments: String,
    /// How Nucleus Co-op injects ProtoInput's input hooks into the game.
    pub inject: Inject,
    /// The stream's size, which an Unreal game started in a window renders
    /// at ([`Setup::windowed_arguments`]); None when no stream asked.
    pub size: Option<(u32, u32)>,
    /// Whether the game's .exe imports DirectInput
    /// ([`super::imports::direct_input`]), which reads the PC's keyboard
    /// through raw input; ProtoInput then takes the game's raw input.
    pub direct_input: bool,
}

/// What Nucleus Co-op 2.4.2 says in a message box when ProtoInput's hooks
/// can't be injected into a running game or one it starts
/// (`ProtoInputLauncher.InjectRuntime`, `InjectStartup`), just before it
/// ends the game.
pub const INJECT_FAILED: [&str; 2] = [
    "ProtoInput failed to runtime inject",
    "ProtoInput failed to startup inject",
];

/// How Nucleus Co-op injects ProtoInput's input hooks into the game. Nucleus
/// ends a game it can't inject into ([`INJECT_FAILED`]), so a game that
/// refuses one way gets the next at its next start (#59), down to none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Inject {
    /// Into the running game, through EasyHook.
    #[default]
    Runtime,
    /// As Nucleus starts the game, before any of its code runs: in before a
    /// game that guards itself once it runs (one whose mods load as DLLs
    /// at its start).
    Startup,
    /// Into the running game from one of its own threads, for a game that
    /// refuses a thread made in it.
    Stealth,
    /// None, and no focus messages from Nucleus either: nothing would hold
    /// back what they wake in the game (#59).
    None,
}

impl Inject {
    pub const ALL: [Inject; 4] = [Self::Runtime, Self::Startup, Self::Stealth, Self::None];

    /// The way tried after this one is refused; None after the last.
    pub fn next(self) -> Option<Inject> {
        let at = Self::ALL.iter().position(|&way| way == self)?;
        Self::ALL.get(at + 1).copied()
    }

    /// Its name in the host's record of each game's way.
    pub fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Startup => "startup",
            Self::Stealth => "stealth",
            Self::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Inject> {
        Self::ALL.into_iter().find(|way| way.name() == name)
    }
}

impl Setup {
    /// The setup for `exe_path`; `steam` is the Steam app it was found from
    /// and the folder Steam installed it to, when it was.
    pub fn new(exe_path: &str, name: &str, steam: Option<(u32, &str)>) -> Self {
        let exe_path = normalized(exe_path);
        let exe_folder = parent(&exe_path).to_string();
        let root = steam
            .map(|(_, install)| normalized(install))
            .filter(|folder| below(&exe_path, folder).is_some())
            .map(|folder| folder.trim_end_matches('\\').to_string())
            .or_else(|| unreal_root(&exe_folder))
            .unwrap_or(exe_folder);
        Self {
            exe_path,
            name: name.trim().to_string(),
            root,
            steam_app: steam.map(|(app, _)| app),
            anti_cheat: None,
            arguments: String::new(),
            inject: Inject::Runtime,
            size: None,
            direct_input: false,
        }
    }

    /// `Game.GUID`: stable for the .exe's path, a folder name for Nucleus'
    /// profiles and instances, and a single command-line word.
    pub fn guid(&self) -> String {
        let stem = file_name(&self.exe_path);
        let stem = stem
            .rsplit_once('.')
            .map_or(stem, |(stem, _)| stem)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .take(40)
            .collect::<String>();
        let hash = Sha256::digest(self.exe_path.to_lowercase().as_bytes());
        format!("{PREFIX}{stem}-{}", hex::encode(&hash[..4]))
    }

    /// The game's arguments with `-windowed` added for an Unreal game (its
    /// .exe in `<Project>\Binaries\Win64`): fullscreen, it covers the monitor
    /// nearest its window, the PC's, however often it is parked beside the
    /// desktop; in a window it stays parked (#56). The switch overrides the
    /// game's saved mode for this start only. Arguments that already choose
    /// a mode stay as they are. In a window it renders at the stream's
    /// [`Setup::size`] (`-ResX`, `-ResY`, and `-ForceRes`, without which a
    /// window larger than the primary display shrinks to fit it), not at its
    /// saved resolution,
    /// which showed a 16:9 picture on a 20:9 phone (#56), unless the
    /// arguments choose a resolution.
    pub fn windowed_arguments(&self) -> String {
        let arguments = self.arguments.trim();
        let words: Vec<String> = arguments
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |flags: &[&str]| {
            words
                .iter()
                .any(|word| flags.iter().any(|flag| word.starts_with(flag)))
        };
        if unreal_root(parent(&self.exe_path)).is_none() || has(&["-fullscreen", "-borderless"]) {
            return arguments.to_string();
        }
        let mut started = arguments.to_string();
        let mut add = |word: &str| {
            if !started.is_empty() {
                started.push(' ');
            }
            started.push_str(word);
        };
        if !has(&["-windowed"]) {
            add("-windowed");
        }
        if let Some((width, height)) = self.size
            && !has(&["-resx", "-resy"])
        {
            add(&format!("-ResX={width} -ResY={height}"));
            // Unreal shrinks a window larger than the primary display to fit
            // it (a 2400x1080 phone beside a 1920x1080 monitor) unless told
            // to keep the size.
            if !has(&["-forceres"]) {
                add("-ForceRes");
            }
        }
        started
    }

    /// The handler's file name in Nucleus' `handlers` folder.
    pub fn file(&self) -> String {
        format!("{}.js", self.guid())
    }

    /// The handler's text, as an earlier build wrote it: without an owner.
    pub fn handler(&self) -> String {
        self.handler_of(None)
    }

    /// The handler's text, written for the account `owner` (a SID).
    pub fn handler_of(&self, owner: Option<&str>) -> String {
        let name = if self.name.is_empty() {
            file_name(&self.exe_path)
        } else {
            &self.name
        };
        let mut head = vec![format!(
            "{MARKER} v{VERSION}: Rubylight writes this file for background streams and rewrites it when it changes. A hub handler for this game takes its place."
        )];
        if let Some(owner) = owner {
            head.push(format!("{OWNER}{owner}"));
        }
        head.extend([
            format!("Game.GUID = {};", js(&self.guid())),
            format!("Game.GameName = {};", js(name)),
            format!("Game.ExecutableName = {};", js(file_name(&self.exe_path))),
        ]);
        let arguments = self.arguments.trim();
        if !arguments.is_empty() {
            head.push(format!("Game.StartArguments = {};", js(arguments)));
        }
        let options = [
            "Game.MaxPlayers = 1;",
            "Game.MaxPlayersOneMonitor = 1;",
            "// The slot's pad only: keyboard and mouse stay with the person at the PC, and so does the taskbar.",
            "Game.SupportsKeyboard = false;",
            "Game.HideTaskbar = false;",
            "// The game runs from its own folder and nothing is written there: no links, copies, DLLs or tools that need administrator rights.",
            "Game.Hook.CustomDllEnabled = false;",
        ];
        let way: &[&str] = match self.inject {
            Inject::Runtime => &[
                "Game.ProtoInput.InjectStartup = false;",
                "Game.ProtoInput.InjectRuntime_EasyHookMethod = true;",
            ],
            Inject::Startup => &[
                "// The game refused ProtoInput's hooks once it ran, so the co-op helper injects them as it starts the game.",
                "Game.ProtoInput.InjectStartup = true;",
            ],
            Inject::Stealth => &[
                "// The game refused ProtoInput's hooks once it ran and as it started, so the co-op helper injects them from one of its own threads.",
                "Game.ProtoInput.InjectStartup = false;",
                "Game.ProtoInput.InjectRuntime_EasyHookStealthMethod = true;",
            ],
            Inject::None => &[],
        };
        let focus = [
            "// The game keeps running and reading its pad without focus, and never sees the PC's keyboard or moves its cursor.",
            "Game.FakeFocus = true;",
        ];
        let hooks = [
            "Game.ProtoInput.FocusHooks = true;",
            "Game.ProtoInput.XinputHook = true;",
            "Game.ProtoInput.UseOpenXinput = true;",
            "Game.ProtoInput.GetKeyStateHook = true;",
            "Game.ProtoInput.GetAsyncKeyStateHook = true;",
            "Game.ProtoInput.GetKeyboardStateHook = true;",
            "Game.ProtoInput.GetCursorPosHook = true;",
            "Game.ProtoInput.SetCursorPosHook = true;",
            "Game.ProtoInput.ClipCursorHook = true;",
            "// Even in front, the game's window messages carry no key, click, wheel or cursor position from the PC's keyboard and mouse (#56).",
            "Game.ProtoInput.MessageFilterHook = true;",
            "Game.ProtoInput.KeyboardButtonFilter = true;",
            "Game.ProtoInput.MouseButtonFilter = true;",
            "Game.ProtoInput.MouseWheelFilter = true;",
            "Game.ProtoInput.MouseMoveFilter = true;",
        ];
        // Only for a game that reads DirectInput: ProtoInput takes all of a
        // game's raw input and passes on none of a pad's, so a game reading
        // its pad through raw input (SDL, Rewired) would lose it.
        let raw_input: &[&str] = if self.direct_input {
            &[
                "// Nor its raw input, through which DirectInput reads the PC's keyboard (NieR:Automata, #56): ProtoInput takes the game's raw input and passes on only the slot's keyboard and mouse, and a pad's slot has none. The game reads DirectInput, so its pad does not come through raw input.",
                "Game.ProtoInput.RegisterRawInputHook = true;",
                "Game.ProtoInput.GetRawInputDataHook = true;",
            ]
        } else {
            &[]
        };
        // No FakeFocus: Nucleus would tell the game every second that it is
        // active, and only ProtoInput's cursor hooks keep a game that believes
        // it from taking the mouse back from the person at the PC, towards
        // its window parked right of the desktop (#59). It never gave such a
        // game its pad, which it reads only when really in front.
        let unhooked = [
            "// The game refused ProtoInput's input hooks every way the co-op helper injects them, so none are injected and a keyboard, mouse or pad on the PC can reach it too. It knows when it has focus, so it lets go of the PC's mouse when it loses it.",
            // Nucleus 2.4.2 lists XInput pads on its setup screen only for a
            // handler with ProtoInput's XInput hook or this switch
            // (`DevicesFunctions.UpdateDevices`). Without either it never
            // sees the profile's pad and stays on that screen (#59). The
            // switch alone injects nothing: Nucleus copies its XInput DLLs
            // only with Hook.CustomDllEnabled or XInputPlusDll, both off.
            "Game.Hook.XInputEnabled = true;",
        ];
        let options: Vec<&&str> = match self.inject {
            Inject::None => options.iter().chain(&unhooked).collect(),
            _ => options
                .iter()
                .chain(&focus)
                .chain(way)
                .chain(&hooks)
                .chain(raw_input)
                .collect(),
        };
        let lines: Vec<&str> = head
            .iter()
            .map(String::as_str)
            .chain(options.into_iter().copied())
            .collect();
        let mut text = lines.join("\r\n");
        text.push_str("\r\n");
        text
    }
}

/// An Unreal game's folder for an .exe in `<root>\<Project>\Binaries\Win64`:
/// the game's content sits beside `Binaries`.
fn unreal_root(exe_folder: &str) -> Option<String> {
    let platform = file_name(exe_folder);
    let binaries = parent(exe_folder);
    let is = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if !(["Win64", "Win32", "WinGDK"].iter().any(|p| is(platform, p))
        && is(file_name(binaries), "Binaries"))
    {
        return None;
    }
    let holder = parent(binaries);
    let root = parent(holder);
    // Never a drive's root: a game right on a drive (`D:\Game\Binaries`)
    // is its own.
    if drive_root(root) {
        return (!drive_root(holder)).then(|| holder.to_string());
    }
    Some(root.to_string())
}

/// Whether `folder` is a drive's root (`D:`, `\\?\D:`) or no folder at all.
fn drive_root(folder: &str) -> bool {
    let folder = folder.trim_end_matches('\\');
    let folder = folder
        .strip_prefix(r"\\?\")
        .or_else(|| folder.strip_prefix(r"\\.\"))
        .unwrap_or(folder);
    folder.len() <= 2 || !folder.contains('\\')
}

/// A JavaScript string literal, in the escapes `handler::parse` reads.
/// Nucleus' ES5 engine (Jint) ends a line at U+2028 and U+2029, even in a
/// string, so they are escaped.
fn js(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars().filter(|c| !c.is_control()) {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests;
