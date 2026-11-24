//! The permission KWin asks of a program before it offers `zkde_screencast_unstable_v1`: a
//! `.desktop` file whose `Exec=` names the program's executable and whose
//! `X-KDE-Wayland-Interfaces=` lists the protocol. The rule and the file KWin accepts follow
//! `kwingrab.cpp` of Vibepollo (Nonary/Vibepollo, GPL-3.0), which carries the KWin capture of
//! Sunshine and Apollo. Every function here takes its directories and text as parameters, so
//! the tests touch only a temporary folder; [`ensure_for_current_exe`] reads the environment.

use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The protocol the file has to list.
pub const INTERFACE: &str = "zkde_screencast_unstable_v1";

/// The environment variable that turns KWin's permission checks off.
const NO_CHECKS: &str = "KWIN_WAYLAND_NO_PERMISSION_CHECKS";

/// The start of the name of the files this host writes.
const PREFIX: &str = "rubylight-kwin-screencast-";

/// What [`ensure`] found or did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// KWin's permission checks are off.
    ChecksOff,
    /// This file already grants the permission.
    Granted(PathBuf),
    /// This file was written just now: KWin needs a moment to read it.
    Written(PathBuf),
}

impl Outcome {
    /// A sentence for an error: the file situation.
    pub fn describe(&self) -> String {
        match self {
            Self::ChecksOff => format!("KWin's permission checks are off ({NO_CHECKS}=1)"),
            Self::Granted(path) => format!("{} grants the permission", path.display()),
            Self::Written(path) => format!(
                "{} was written just now and KWin may not have read it yet; restart the host or set {NO_CHECKS}=1",
                path.display()
            ),
        }
    }
}

/// The program a desktop file's `Exec=` line names: its first word, unquoted.
pub fn exec_of(contents: &str) -> Option<String> {
    let line = contents
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("Exec="))?
        .trim();
    let mut word = String::new();
    let mut chars = line.chars();
    match line.chars().next()? {
        '"' => {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => word.extend(chars.next()),
                    c => word.push(c),
                }
            }
        }
        _ => word.extend(chars.take_while(|c| !c.is_whitespace())),
    }
    (!word.is_empty()).then_some(word)
}

/// The protocols a desktop file's `X-KDE-Wayland-Interfaces=` line lists.
pub fn interfaces_of(contents: &str) -> Vec<String> {
    contents
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("X-KDE-Wayland-Interfaces="))
        .map(|list| {
            list.split(';')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a desktop file grants the screencast protocol to the executable `exe`.
pub fn grants(contents: &str, exe: &str) -> bool {
    exec_of(contents).is_some_and(|exec| exec == exe)
        && interfaces_of(contents).iter().any(|item| item == INTERFACE)
}

/// The first desktop file in `dirs` that grants the permission to `exe`.
pub fn find_granting(dirs: &[PathBuf], exe: &str) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        files.sort();
        files
            .into_iter()
            .find(|path| fs::read_to_string(path).is_ok_and(|contents| grants(&contents, exe)))
    })
}

/// The `applications` folders to look in, user first: `$XDG_DATA_HOME` (else `~/.local/share`)
/// and each of `$XDG_DATA_DIRS` (else `/usr/local/share:/usr/share`). The first is where a file
/// is written. `None` when there is no home to write in.
pub fn applications_dirs(
    data_home: Option<&str>,
    home: Option<&str>,
    data_dirs: Option<&str>,
) -> Option<(PathBuf, Vec<PathBuf>)> {
    let user = match data_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(home.filter(|dir| !dir.is_empty())?).join(".local/share"),
    }
    .join("applications");
    let system = data_dirs
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or("/usr/local/share:/usr/share");
    let mut all = vec![user.clone()];
    all.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join("applications")),
    );
    Some((user, all))
}

/// The name of the file for `exe`: the prefix and 16 hex digits of a hash of the path (FNV-1a,
/// which stays the same from one build to the next).
pub fn file_name(exe: &str) -> String {
    let hash = exe.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{PREFIX}{hash:016x}.desktop")
}

/// The text of the file that grants the permission to `exe`.
pub fn contents(exe: &str) -> String {
    let exec = if exe.contains(|c: char| c.is_whitespace() || "\"\\".contains(c)) {
        format!("\"{}\"", exe.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        exe.to_owned()
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=Rubylight screen capture permission\nExec={exec}\nX-KDE-Wayland-Interfaces={INTERFACE}\nNoDisplay=true\n"
    )
}

/// Removes the files of ours in `dir` whose program is gone.
fn remove_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
        let ours = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(".desktop"));
        let alive = fs::read_to_string(&path)
            .ok()
            .and_then(|text| exec_of(&text))
            .is_some_and(|exec| Path::new(&exec).exists());
        if ours && !alive && fs::remove_file(&path).is_ok() {
            tracing::info!(file = %path.display(), "removed a stale KWin screencast permission file");
        }
    }
}

/// Makes sure KWin will offer the screencast protocol to `exe`: finds a granting file in
/// `dirs` (the first of which is the user's) or writes one there, after removing stale ones.
pub fn ensure(exe: &str, dirs: &[PathBuf], checks_off: bool) -> Result<Outcome> {
    if checks_off {
        return Ok(Outcome::ChecksOff);
    }
    if let Some(path) = find_granting(dirs, exe) {
        return Ok(Outcome::Granted(path));
    }
    let user = dirs.first().context("no folder for the permission file")?;
    fs::create_dir_all(user).with_context(|| format!("creating {}", user.display()))?;
    remove_stale(user);
    let path = user.join(file_name(exe));
    fs::write(&path, contents(exe)).with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(file = %path.display(), "wrote the KWin screencast permission file");
    Ok(Outcome::Written(path))
}

/// [`ensure`] for this program, with the folders of the environment.
pub fn ensure_for_current_exe() -> Result<Outcome> {
    let var = |name| std::env::var(name).ok();
    let checks_off = var(NO_CHECKS).as_deref() == Some("1");
    let exe = std::env::current_exe().context("finding this program's path")?;
    let exe = exe.to_str().context("this program's path is not text")?;
    let (_, dirs) = applications_dirs(
        var("XDG_DATA_HOME").as_deref(),
        var("HOME").as_deref(),
        var("XDG_DATA_DIRS").as_deref(),
    )
    .context("no HOME to write the permission file under")?;
    ensure(exe, &dirs, checks_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rubylight-kwin-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn exec_first_word_unquoted() {
        assert_eq!(
            exec_of("[Desktop Entry]\nExec=/usr/bin/x --a %U\n").as_deref(),
            Some("/usr/bin/x")
        );
        assert_eq!(
            exec_of("Exec=\"/opt/My App/x\" y").as_deref(),
            Some("/opt/My App/x")
        );
        assert_eq!(exec_of("Exec=\"/a\\\"b\"").as_deref(), Some("/a\"b"));
        assert_eq!(exec_of("Name=x\nExec=\n"), None);
        assert_eq!(exec_of("Name=x"), None);
    }

    #[test]
    fn interfaces_split_on_semicolons() {
        let list = interfaces_of(
            "X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input; zkde_screencast_unstable_v1;\n",
        );
        assert_eq!(
            list,
            ["org_kde_kwin_fake_input", "zkde_screencast_unstable_v1"]
        );
        assert!(interfaces_of("Name=x").is_empty());
    }

    #[test]
    fn grants_needs_both_exec_and_interface() {
        let text = contents("/usr/bin/butterpollo");
        assert!(grants(&text, "/usr/bin/butterpollo"));
        assert!(!grants(&text, "/usr/bin/other"));
        assert!(!grants(
            "Exec=/usr/bin/butterpollo\nX-KDE-Wayland-Interfaces=other\n",
            "/usr/bin/butterpollo"
        ));
        assert!(grants(&contents("/opt/a b/x"), "/opt/a b/x"));
    }

    #[test]
    fn directories_follow_the_xdg_rules() {
        let (user, all) = applications_dirs(None, Some("/home/u"), None).unwrap();
        assert_eq!(user, Path::new("/home/u/.local/share/applications"));
        assert_eq!(
            all[1..],
            [
                PathBuf::from("/usr/local/share/applications"),
                PathBuf::from("/usr/share/applications")
            ]
        );
        let (user, all) = applications_dirs(Some("/d"), None, Some("/a::/b")).unwrap();
        assert_eq!(user, Path::new("/d/applications"));
        assert_eq!(all.len(), 3);
        assert!(applications_dirs(None, None, None).is_none());
    }

    #[test]
    fn file_name_is_stable_and_per_executable() {
        assert_eq!(
            file_name("/usr/bin/butterpollo"),
            file_name("/usr/bin/butterpollo")
        );
        assert_ne!(file_name("/a"), file_name("/b"));
        let name = file_name("/a");
        assert!(name.starts_with(PREFIX) && name.ends_with(".desktop"));
        assert_eq!(name.len(), PREFIX.len() + 16 + ".desktop".len());
    }

    #[test]
    fn a_system_file_grants_and_nothing_is_written() {
        let root = temp("system");
        let (user, system) = (root.join("user"), root.join("system"));
        fs::create_dir_all(&system).unwrap();
        fs::write(system.join("any.desktop"), contents("/usr/bin/butterpollo")).unwrap();
        let outcome = ensure(
            "/usr/bin/butterpollo",
            &[user.clone(), system.clone()],
            false,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Granted(system.join("any.desktop")));
        assert!(!user.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_grant_is_written_once_and_stale_files_go() {
        let root = temp("write");
        let user = root.join("user");
        fs::create_dir_all(&user).unwrap();
        let live = root.join("live");
        fs::write(&live, "").unwrap();
        let live = live.to_str().unwrap().to_owned();
        let stale = user.join(format!("{PREFIX}0000000000000000.desktop"));
        fs::write(&stale, contents("/nonexistent/path/x")).unwrap();
        let unrelated = user.join("other.desktop");
        fs::write(&unrelated, contents("/nonexistent/path/y")).unwrap();
        let dirs = [user.clone(), root.join("none")];
        let Outcome::Written(path) = ensure(&live, &dirs, false).unwrap() else {
            panic!("expected a written file");
        };
        assert_eq!(path, user.join(file_name(&live)));
        assert!(grants(&fs::read_to_string(&path).unwrap(), &live));
        assert!(!stale.exists());
        assert!(unrelated.exists());
        assert_eq!(ensure(&live, &dirs, false).unwrap(), Outcome::Granted(path));
        assert_eq!(ensure(&live, &dirs, true).unwrap(), Outcome::ChecksOff);
        fs::remove_dir_all(root).unwrap();
    }
}
