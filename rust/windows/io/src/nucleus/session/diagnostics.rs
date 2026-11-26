//! Crashlog tailing, hook-refusal and Mandatory ASLR handling, and startup
//! failure diagnostics for Nucleus Co-op sessions.
use super::super::files;
use super::super::instances::MessageBox;
use super::super::user_copy;
use super::runner::Runner;
use super::{EXEMPT_ASLR, FAILED_PASSES, MANDATORY_ASLR};
use butterpollo_session::audio_scope::Identity;
use butterpollo_session::nucleus::coop::{End, Phase};
use butterpollo_session::nucleus::generic::{self, Inject};
use butterpollo_session::nucleus::profile::TITLE;
use std::collections::BTreeSet;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Nucleus' `crashlog.txt` in the folder it runs from, where it appends
/// each exception it catches (`[time] exception`, then its stack) and goes
/// on, as it was when the session launched it. It is read as the account
/// that reads that folder: the user for their own copy, which is theirs to
/// change, so the host never reads through a link they made there.
pub(super) struct Crashlog {
    pub(super) path: PathBuf,
    /// Its length at the launch: what follows is this session's.
    pub(super) from: u64,
}

impl Crashlog {
    /// The longest line quoted.
    pub(super) const LIMIT: usize = 300;
    /// The most read, from its end: the newest exception is there, and a
    /// file made huge is never read whole.
    pub(super) const TAIL: u64 = 64 * 1024;

    /// As it is at the launch, read as `reader` (None: as the host).
    pub(super) fn at_launch(folder: &Path, reader: Option<&OwnedHandle>) -> Crashlog {
        let path = folder.join("crashlog.txt");
        let from = Self::read(reader, &path, || std::fs::metadata(&path).map(|m| m.len()));
        Crashlog {
            from: from.unwrap_or(0),
            path,
        }
    }

    /// `work` on the file at `path` as `reader`: None when there is no
    /// file, or with a warning when it could not be read.
    fn read<T>(
        reader: Option<&OwnedHandle>,
        path: &Path,
        work: impl FnOnce() -> std::io::Result<T>,
    ) -> Option<T> {
        let why = match user_copy::as_user(reader, work) {
            Ok(Ok(value)) => return Some(value),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Ok(Err(error)) => error.to_string(),
            Err(error) => format!("{error:#}"),
        };
        tracing::warn!(error = why, path = %path.display(), "The co-op helper's crashlog not read");
        None
    }

    /// What was written since the launch, at most [`Self::TAIL`] from its
    /// end; a shorter file was started again since, and counts whole.
    pub(super) fn since_launch(&self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(&self.path)?;
        let length = file.metadata()?.len();
        let from = if self.from <= length { self.from } else { 0 };
        file.seek(SeekFrom::Start(from.max(length.saturating_sub(Self::TAIL))))?;
        let mut bytes = Vec::new();
        file.take(Self::TAIL).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// The newest exception written since the launch, read as `reader`:
    /// its first line.
    pub(super) fn newest(&self, reader: Option<&OwnedHandle>) -> Option<String> {
        let bytes = Self::read(reader, &self.path, || self.since_launch())?;
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let line = lines
            .iter()
            .rev()
            .find(|line| line.starts_with('['))
            .or(lines.last())?;
        Some(match line.char_indices().nth(Self::LIMIT) {
            Some((at, _)) => format!("{}...", &line[..at]),
            None => line.to_string(),
        })
    }
}

impl Runner {
    /// Rubylight's own handler's game refused ProtoInput's hooks: Nucleus
    /// shows [`generic::INJECT_FAILED`] as it starts the game or once the
    /// game runs, and ends it when the box is answered, which nobody does
    /// during a background stream (#59). The game is kept so its next
    /// handler injects them the next way ([`generic::Inject`]), or none
    /// after the last, and the session ends saying so. Any handler's game
    /// may also wait on a box of its own about Mandatory ASLR
    /// ([`Self::mandatory_aslr`]).
    pub(super) fn refused_hooks(
        &mut self,
        job: &BTreeSet<u32>,
        roots: &[Option<Identity>],
    ) -> Option<End> {
        let boxes = match self.desktop.message_boxes(job) {
            Ok(boxes) => boxes,
            Err(error) => {
                let why = format!("{error:#}");
                if self.unseen.as_ref() != Some(&why) {
                    tracing::warn!(
                        why,
                        "the PC's message boxes not listed, so a game refusing the co-op helper's input hooks or waiting on a box is not seen"
                    );
                }
                self.unseen = Some(why);
                return None;
            }
        };
        let new = |shown: &&MessageBox| !self.before.contains(&shown.window);
        if self.own_game.is_some()
            && let Some(shown) = boxes.iter().filter(new).find(|shown| {
                generic::INJECT_FAILED
                    .iter()
                    .any(|failed| shown.text.contains(failed))
            })
        {
            let why = self.refuse_way(
                &shown.summary(),
                "the co-op helper ends a game it can't hook",
            );
            return Some(self.failure(why));
        }
        let shown = boxes
            .iter()
            .filter(new)
            .find(|shown| shown.text.contains(MANDATORY_ASLR))?
            .clone();
        Some(self.mandatory_aslr(&shown, job, roots))
    }

    /// The game waits on its own box about Mandatory ASLR. Windows forcing
    /// it on the game shows that box whatever way the hooks went in, so the
    /// way is kept and the player is told how to exempt it; a game Windows
    /// doesn't force it on refused the way, which is given up as for
    /// Nucleus' box. When the policy can't be read, the way is kept too.
    /// A box Windows shows for the game (MB_SERVICE_NOTIFICATION, owned by
    /// csrss.exe, whose policy can't be read) is the game instance's: its
    /// process is read instead.
    fn mandatory_aslr(
        &mut self,
        shown: &MessageBox,
        job: &BTreeSet<u32>,
        roots: &[Option<Identity>],
    ) -> End {
        let said = shown.summary();
        let game = self.desktop.owner(shown.window).and_then(|owner| {
            if job.contains(&owner) {
                return Ok(owner);
            }
            roots
                .iter()
                .flatten()
                .next()
                .map(|root| root.pid)
                .ok_or_else(|| anyhow::anyhow!("no instance of the game runs"))
        });
        let forced = game.and_then(|pid| Ok((pid, self.desktop.forced_aslr(pid)?)));
        let program = forced
            .as_ref()
            .ok()
            .and_then(|(pid, _)| self.desktop.image_path(*pid))
            .or_else(|| self.own_game.as_ref().map(|(_, exe)| exe.clone()))
            .and_then(|path| {
                Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "the game's .exe".into());
        let exempt = EXEMPT_ASLR.replace("{program}", &program);
        let waits = format!(
            "{} showed \"{said}\" and waits there for an answer",
            self.title
        );
        let why = match forced {
            Ok((_, true)) => format!(
                "{waits}: Windows forces randomization for images (Mandatory ASLR) on it. To start it, you can {exempt}"
            ),
            Ok((_, false)) => match &self.own_game {
                Some((folder, exe))
                    if files::inject_for(folder, exe).ok() != Some(Inject::None) =>
                {
                    self.refuse_way(&said, "it waits there for an answer")
                }
                Some(_) => format!("{waits}, even without the co-op helper's input hooks"),
                None => waits,
            },
            Err(error) => format!(
                "{waits}; Rubylight could not read whether Windows forces randomization for images (Mandatory ASLR) on it: {error:#}. If it does, you can {exempt}"
            ),
        };
        self.failure(why)
    }

    /// Give up the way Rubylight's own handler's game refused, and say so.
    fn refuse_way(&self, said: &str, ends: &str) -> String {
        let Some((folder, exe)) = &self.own_game else {
            return format!("{} showed \"{said}\", and {ends}", self.title);
        };
        let refused = format!(
            "{} refused the co-op helper's input hooks (it showed \"{said}\"), and {ends}",
            self.title
        );
        match files::refuse_inject(folder, exe) {
            Ok(Inject::None) => {
                format!("{refused}: start the stream again, and Rubylight starts it without them")
            }
            Ok(_) => format!(
                "{refused}: start the stream again, and Rubylight has the co-op helper hook it another way"
            ),
            Err(error) => {
                format!("{refused}; Rubylight could not keep that for its next start: {error:#}")
            }
        }
    }

    /// Player `slot`'s instance exited.
    pub(super) fn exited(&mut self, slot: usize) -> End {
        let mut why = format!("Player {}'s instance of {} exited", slot + 1, self.title);
        // Soon after its start: a game Epic's launcher restarts leaves at once.
        if self.inject_by.is_none_or(|by| Instant::now() < by)
            && self
                .own_game
                .as_ref()
                .is_some_and(|(_, exe)| files::epic_game(exe))
        {
            why = butterpollo_session::nucleus::generic::epic_exit(&why, &self.title);
        }
        self.failure(why)
    }

    /// A pass that could not look at the desktop: the session ends after
    /// [`FAILED_PASSES`] in a row.
    pub(super) fn failed_pass(&mut self, error: anyhow::Error) -> Option<End> {
        self.failed += 1;
        let why = format!("{error:#}");
        tracing::warn!(error = why, failed = self.failed, "Co-op pass failed");
        (self.failed >= FAILED_PASSES).then(|| self.failure(why))
    }

    /// What Nucleus shows instead of the profile: a message box it shows
    /// before its window (its folder check, a missing runtime) waits for an
    /// answer no one gives during a background stream (#59).
    pub(super) fn look(&mut self, job: &BTreeSet<u32>) {
        let looked = match self.desktop.message_boxes(job) {
            Ok(mut boxes) => {
                boxes.retain(|shown| !self.before.contains(&shown.window));
                self.showing = boxes;
                self.desktop
                    .windows()
                    .map(|windows| windows.iter().any(|w| w.visible && job.contains(&w.pid)))
            }
            Err(error) => {
                self.showing.clear();
                Err(error)
            }
        };
        match looked {
            Ok(shown) => {
                self.shown |= shown;
                self.unseen = None;
            }
            Err(error) => {
                let why = format!("{error:#}");
                if self.unseen.as_ref() != Some(&why) {
                    tracing::warn!(why, "The co-op helper's windows not listed");
                }
                self.unseen = Some(why);
            }
        }
        for shown in &self.showing {
            if !self.logged.contains(shown) {
                tracing::warn!(
                    "The co-op helper shows \"{}\" on the PC's screen",
                    shown.summary()
                );
                self.logged.push(shown.clone());
            }
        }
    }

    /// Why Nucleus did not show the profile.
    fn unshown(&self) -> String {
        if let Some(shown) = self.showing.first() {
            return format!(
                "The co-op helper shows \"{}\" on the PC's screen",
                shown.summary()
            );
        }
        let why = if self.shown {
            self.unlisted().unwrap_or_else(|| {
                format!(
                    "The co-op helper shows no profile {TITLE} for {}; open the co-op helper and load the profile {TITLE} once",
                    self.title
                )
            })
        } else if let Some(unseen) = &self.unseen {
            format!("The co-op helper's windows could not be listed: {unseen}")
        } else {
            "The co-op helper showed no window".to_string()
        };
        self.quoting_crashlog(why)
    }

    /// Why Nucleus did not start the game after the profile was clicked.
    /// Nucleus shows Play on a loaded profile, or plays one that plays by
    /// itself, once each of its players has a controller
    /// (`GameProfile.FindProfilePlayers`), and hides the profile list as it
    /// plays.
    /// What came after the last click says why: a form closed since it, or
    /// else whether the list is still there.
    fn unstarted(&self) -> String {
        let boxed = self.last_form.as_ref().filter(|_| self.boxed);
        let why = if let Some(form) = boxed {
            format!(
                "The co-op helper did not start {}: after the last click on the profile {TITLE} it showed {} over its window ({} closed since the launch)",
                self.title,
                said(form),
                self.closed.len()
            )
        } else if self.played {
            format!(
                "The co-op helper did not start {} after Rubylight pressed Play on the profile {TITLE}",
                self.title
            )
        } else if !self.hid {
            let then = if self.plays_itself {
                "which starts the game"
            } else {
                "which shows Play"
            };
            format!(
                "The co-op helper did not start {}: it still shows its setup screen with the profile {TITLE}, {then} once every player has a controller there",
                self.title
            )
        } else {
            format!(
                "The co-op helper did not start {} after loading the profile {TITLE}",
                self.title
            )
        };
        self.quoting_crashlog(why)
    }

    /// `why`, with the newest exception of Nucleus' crashlog.txt since the
    /// launch.
    fn quoting_crashlog(&self, why: String) -> String {
        let reader = self.reader.as_ref();
        match self.crashlog.as_ref().and_then(|log| log.newest(reader)) {
            Some(line) => format!("{why}. Its crashlog.txt says: {line}"),
            None => why,
        }
    }

    /// Why Nucleus Co-op shows no entry for the game, when it doesn't: no
    /// profile of a game it doesn't list ever appears (#59).
    fn unlisted(&self) -> Option<String> {
        let (folder, guid) = self.game.as_ref()?;
        let list = files::list(folder);
        let listed = user_copy::as_user(self.reader.as_ref(), || files::listed_by_nucleus(folder))
            .and_then(|listed| listed);
        match listed {
            Ok(listed) if listed.contains(guid) => None,
            Ok(_) => Some(format!(
                "The co-op helper does not list {}: {} has no entry for it with its handler and an .exe that is there",
                self.title,
                list.display()
            )),
            Err(error) => Some(format!(
                "The co-op helper's game list can't be read: {error:#}"
            )),
        }
    }

    fn failure(&self, why: String) -> End {
        if self.shared.session.lock().unwrap().phase == Phase::Running {
            End::Crashed(why)
        } else {
            End::Failed(why)
        }
    }

    pub(super) fn timeout(&self) -> String {
        let (clicked, roots, _) = self.progress;
        match (&self.unclicked, clicked, roots) {
            // The box is why: it covers the list or holds the start.
            (_, _, 0) if !self.showing.is_empty() => self.unshown(),
            (Some(why), false, 0) => {
                format!("The co-op helper did not start {}: {why}", self.title)
            }
            (None, false, 0) => self.unshown(),
            // The last press, of Play or the profile's entry, failed: what stopped it is why.
            (Some(why), true, 0) if !self.plays_itself && !self.played => {
                format!("The co-op helper did not start {}: {why}", self.title)
            }
            (_, _, 0) => self.unstarted(),
            _ if roots < self.players => self.quoting_crashlog(format!(
                "The co-op helper started {roots} of {} instances of {}",
                self.players, self.title
            )),
            _ => {
                let slot = self.windows.iter().position(Option::is_none).unwrap_or(0);
                format!(
                    "Player {}'s instance of {} showed no window",
                    slot + 1,
                    self.title
                )
            }
        }
    }
}

/// What a form Nucleus showed after a click said. Nucleus Co-op 2.4.2's
/// own box (`NucleusMessageBox`) draws its text and is untitled when it
/// says the profile can't be loaded (a click before its setup screen found
/// the PC's screens, [`super::SETTLE`]) or that the game's folder is missing: it
/// reads as nothing, and is named for what it is.
pub(super) fn said(form: &MessageBox) -> String {
    match form.summary() {
        summary if summary.is_empty() => "its own untitled message box, whose text can't be read (in the co-op helper 2.4.2 it says the profile can't be loaded, or that the game's folder is missing)".to_string(),
        summary => format!("\"{summary}\""),
    }
}
