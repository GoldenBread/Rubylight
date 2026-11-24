//! Size-limited log files. When `name` would exceed its limit it becomes
//! `name.1`, older files move up to `name.<keep>` and the oldest is dropped,
//! so a long-running service cannot fill the disk.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How long the log writes between timed checks that its path is still there
/// and tries of dropped lines, besides the checks every `EXISTS_CHECK` and
/// `RETRY_DROPPED` bytes: a quiet log takes long to write 1 MiB.
const CHECK_EVERY: Duration = Duration::from_secs(10);

/// How many bytes the log writes between checks that its path is still there.
const EXISTS_CHECK: u64 = 1 << 20;
/// How many bytes of dropped lines go by between tries to write again.
const RETRY_DROPPED: u64 = 1 << 20;
/// The log's own line after a cut, with the bytes cut for `{}`.
const CUT_NOTE: &str =
    "--- log cut back: {} bytes of earlier lines removed, the file could not be rotated ---";
/// The log's own line when writing goes on after lines were dropped.
const DROP_NOTE: &str =
    "--- log lines dropped: {} bytes, the file could neither be rotated nor cut back ---";

pub struct RotatingFile {
    path: PathBuf,
    file: File,
    /// The same file with write access, which an append-only handle lacks on
    /// Windows; only the backstop uses it, to cut the file back.
    cut: Option<File>,
    size: u64,
    limit: u64,
    keep: usize,
    /// The open file is no longer at the live path (moved to `name.0` by a
    /// rotation, or deleted or moved away by another program) and the live
    /// path could not be opened again yet.
    detached: bool,
    /// The open file was deleted or moved away by another program: it has no
    /// name of the log's, so nothing can roll it over.
    orphan: bool,
    /// The backstop could not cut the file back: lines are dropped.
    dropping: bool,
    /// Bytes of lines dropped since then.
    dropped: u64,
    /// `check` asked for dropped lines to be tried again on the next write.
    retry: bool,
    /// `CHECK_EVERY`; tests set it to zero.
    every: Duration,
    /// When the last timed check ran.
    checked: Instant,
    /// How often the backstop cut the file being written back.
    #[cfg(test)]
    backstops: u32,
}
impl RotatingFile {
    /// The host's default: 32 MiB per file and four older files.
    pub fn open_default(path: &Path) -> io::Result<Self> {
        Self::open(path, 32 * 1024 * 1024, 4)
    }
    pub fn open(path: &Path, limit: u64, keep: usize) -> io::Result<Self> {
        let (file, cut) = open_pair(path)?;
        let mut log = Self {
            path: path.to_owned(),
            file,
            cut,
            size: std::fs::metadata(path).map_or(0, |m| m.len()),
            limit: limit.max(1),
            keep: keep.max(1),
            detached: false,
            orphan: false,
            dropping: false,
            dropped: 0,
            retry: false,
            every: CHECK_EVERY,
            checked: Instant::now(),
            #[cfg(test)]
            backstops: 0,
        };
        if log.size >= log.limit {
            // A failed rotation is not fatal: keep appending to the current
            // file, as `write` does, rather than keep the host from starting.
            let _ = log.rotate();
        }
        Ok(log)
    }
    /// `name.<n>`; also the rotated files a log viewer or bundle may include.
    pub fn rotated(path: &Path, n: usize) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }
    fn rotate(&mut self) -> io::Result<()> {
        // The open file moves first, to `name.0`: when a reader without delete
        // sharing holds it, no older file has moved or gone yet, so a rotation
        // that fails on every write costs no history. Windows allows renaming
        // the open file: Rust opens files shareable for deletion, as do readers
        // that follow the log. A `name.0` left by a rotation that failed below
        // moves up before the open file takes its place.
        let staged = Self::rotated(&self.path, 0);
        if !staged.exists() {
            match std::fs::rename(&self.path, &staged) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Another program deleted or moved the log away: the open
                    // file is no longer the log. Nothing was moved aside, so
                    // no older file moves up; the live path opens again.
                    self.detached = true;
                    self.orphan = true;
                    self.reattach(false);
                    return Ok(());
                }
                result => result?,
            }
            // Until the live path opens, writes go on into `name.0`, and the
            // next write tries to open it again.
            self.detached = true;
            (self.file, self.cut) = open_pair(&self.path)?;
            self.detached = false;
            self.size = 0;
        }
        self.shift()
    }
    /// Drops the oldest file and moves `name.<n>` up to `name.<n+1>`.
    fn shift(&self) -> io::Result<()> {
        let _ = std::fs::remove_file(Self::rotated(&self.path, self.keep));
        for n in (0..self.keep).rev() {
            match std::fs::rename(
                Self::rotated(&self.path, n),
                Self::rotated(&self.path, n + 1),
            ) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
    /// While the live path cannot be opened, the file being written rolls over
    /// as the log would: it moves up with the older files and writing goes on
    /// in a fresh `name.0`, so the files stay within their limit.
    fn roll_detached(&mut self) -> io::Result<()> {
        let staged = Self::rotated(&self.path, 0);
        if staged.exists() {
            self.shift()?;
        }
        (self.file, self.cut) = open_pair(&staged)?;
        self.size = 0;
        self.orphan = false;
        Ok(())
    }
    /// While detached: the live path first; if it still fails, `name.0` when
    /// the file being written is full or has no name of the log's (a deleted
    /// file would take lines nobody can read until the size limit).
    fn reattach(&mut self, full: bool) {
        self.reopen();
        if self.detached && (full || self.orphan) {
            let _ = self.roll_detached();
        }
    }
    /// Opens the live path again after a rotation moved the open file away but
    /// could not open it; then the moved file takes its place among the older
    /// ones. While it still fails, writing goes on into the moved file.
    fn reopen(&mut self) {
        let opened = open_pair(&self.path).or_else(|error| {
            // The log's folder was deleted too: it is made again, as the host
            // does at start.
            let folder = self
                .path
                .parent()
                .filter(|_| error.kind() == io::ErrorKind::NotFound);
            std::fs::create_dir_all(folder.ok_or(error)?)?;
            open_pair(&self.path)
        });
        let Ok((file, cut)) = opened else {
            return;
        };
        self.size = file.metadata().map_or(0, |m| m.len());
        self.file = file;
        self.cut = cut;
        self.detached = false;
        self.orphan = false;
        if Self::rotated(&self.path, 0).exists() {
            // Moves only the older files: `name.0` is there, so the reopened
            // file stays the live one.
            let _ = self.rotate();
        }
        if self.size >= self.limit {
            // A file already at the limit (another writer, a restored file)
            // is rotated, as at start, rather than left to the backstop.
            let _ = self.rotate();
        }
    }
    /// Checks, every `CHECK_EVERY` of writing, that the log's path is still
    /// there and lets dropped lines be tried again with this line.
    fn check(&mut self) {
        if !self.detached && !self.path.exists() {
            self.detached = true;
            self.orphan = true;
        }
        self.retry = self.dropping;
    }
    /// One line of the log's own, written straight to the file (never through
    /// `write`, so never into the backstop) and counted in its size.
    fn note(&mut self, template: &str, bytes: u64) {
        let line = format!("{}\n", template.replace("{}", &bytes.to_string()));
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.size += line.len() as u64;
        }
    }
}
fn append(path: &Path) -> io::Result<File> {
    #[cfg(test)]
    if tests::FAIL_OPEN.with(|fail| fail.borrow().iter().any(|failing| failing == path)) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    OpenOptions::new().create(true).append(true).open(path)
}
/// The append handle every line goes through, and beside it the same file
/// with write access for the backstop (opened only with the file, never per
/// line; without it the backstop drops lines instead).
fn open_pair(path: &Path) -> io::Result<(File, Option<File>)> {
    let file = append(path)?;
    Ok((file, OpenOptions::new().write(true).open(path).ok()))
}
fn truncate(cut: Option<&File>) -> io::Result<()> {
    #[cfg(test)]
    if tests::FAIL_TRUNCATE.with(std::cell::Cell::get) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    cut.ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?
        .set_len(0)
}
impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // If rotation fails (a reader without delete sharing holds the old
        // file), keep writing to the current file rather than lose lines.
        let length = buf.len() as u64;
        // A monotonic clock read per line (no system call on Windows); the
        // check itself, one stat, runs at most every `CHECK_EVERY`.
        let now = Instant::now();
        if now.duration_since(self.checked) >= self.every {
            self.checked = now;
            self.check();
        }
        if self.dropping && !std::mem::take(&mut self.retry) {
            // Lines are dropped (see below) until about 1 MiB more would have
            // been written, or `check` asks for a try.
            let after = self.dropped + length;
            if self.dropped / RETRY_DROPPED == after / RETRY_DROPPED {
                self.dropped = after;
                return Ok(buf.len());
            }
        }
        let full = self.size > 0 && self.size + length > self.limit;
        if self.detached {
            self.reattach(full);
        } else if full {
            let _ = self.rotate();
        }
        if self.size > 0 && self.size + length > 2 * self.limit && !self.cut_back() {
            self.dropping = true;
            self.dropped += length;
            return Ok(buf.len());
        }
        if std::mem::take(&mut self.dropping) {
            let dropped = std::mem::take(&mut self.dropped);
            self.note(DROP_NOTE, dropped);
        }
        let written = self.file.write(buf)?;
        let before = self.size;
        self.size += written as u64;
        // A log another program deleted or moved away is noticed within
        // `EXISTS_CHECK` bytes, not only at its size limit: one check per
        // interval rather than per line. The next write opens it again.
        if !self.detached
            && before / EXISTS_CHECK != self.size / EXISTS_CHECK
            && !self.path.exists()
        {
            self.detached = true;
            self.orphan = true;
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl RotatingFile {
    /// The backstop, which a log that rotates never reaches: nothing could
    /// replace the file being written (a program holds it or the oldest file
    /// without delete sharing, a folder blocks the rotation, neither the log
    /// path nor `name.0` opens), and it would pass twice the size limit. It is
    /// cut back to nothing, dropping its lines, so the disk stays bounded.
    /// Whether that worked; a file that cannot be cut back (a mapped file)
    /// makes `write` drop lines instead.
    fn cut_back(&mut self) -> bool {
        #[cfg(test)]
        {
            self.backstops += 1;
        }
        if truncate(self.cut.as_ref()).is_err() {
            return false;
        }
        let cut = std::mem::take(&mut self.size);
        self.note(CUT_NOTE, cut);
        true
    }
}

#[cfg(test)]
mod tests;
