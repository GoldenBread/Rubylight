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
mod tests {
    use super::*;
    thread_local! {
        /// Makes opening these paths fail, as a denied or locked path would.
        pub(super) static FAIL_OPEN: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
        /// Makes cutting the file back fail, as a mapped file would.
        pub(super) static FAIL_TRUNCATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// A rotation that moves the open file but cannot open the live path again
    /// keeps the lines in the moved file, and the next write opens the live
    /// path again instead of leaving it missing.
    #[test]
    fn a_rotation_that_cannot_reopen_the_log_opens_it_on_the_next_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        let write = |log: &mut RotatingFile, line: usize| {
            log.write_all(format!("{line:039}\n").as_bytes()).unwrap()
        };
        for line in 0..2 {
            write(&mut log, line);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone()]));
        write(&mut log, 2);
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        assert!(!path.exists());
        for line in 3..5 {
            write(&mut log, line);
        }
        drop(log);
        let lines = |path: &Path| {
            std::fs::read_to_string(path)
                .map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert_eq!(lines(&path), [3, 4].map(|n| format!("{n:039}")));
        // No line is lost: the ones written before the live path came back are
        // in the older file.
        assert_eq!(
            lines(&RotatingFile::rotated(&path, 1)),
            [0, 1, 2].map(|n| format!("{n:039}"))
        );
    }

    fn line(n: usize) -> String {
        format!("{n:039}")
    }
    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// A log another program deleted or moved away while it was open comes
    /// back at its size limit, and the older files stay as they were: nothing
    /// was moved aside, so nothing moves up.
    #[test]
    fn a_log_deleted_while_open_comes_back_at_its_size_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        for n in 0..4 {
            log.write_all(format!("{}\n", line(n)).as_bytes()).unwrap();
        }
        let older = lines(&RotatingFile::rotated(&path, 1));
        assert!(!older.is_empty());
        std::fs::remove_file(&path).unwrap();
        // 80 bytes in the deleted file: the next line passes the limit.
        log.write_all(format!("{}\n", line(4)).as_bytes()).unwrap();
        log.write_all(format!("{}\n", line(5)).as_bytes()).unwrap();
        drop(log);
        assert_eq!(lines(&path), [line(4), line(5)]);
        assert_eq!(lines(&RotatingFile::rotated(&path, 1)), older);
    }

    /// A deleted log is noticed long before its size limit: within the
    /// interval at which the log checks that its path is still there.
    #[test]
    fn a_log_deleted_while_open_comes_back_before_its_size_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 1 << 30, 2).unwrap();
        log.write_all(b"before\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        let chunk = vec![b'x'; 4095];
        for _ in 0..(EXISTS_CHECK / 4096 + 1) {
            log.write_all(&chunk).unwrap();
            log.write_all(b"\n").unwrap();
        }
        log.write_all(b"after\n").unwrap();
        drop(log);
        // What was written after the check is in the log again.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("\nafter\n"));
        assert!(text.len() < 2 * 4096 + 7, "{} bytes", text.len());
    }

    /// While the live path cannot be opened for good, writing goes on into
    /// `name.0`, which rolls over as the log would: the files stay within the
    /// limit, every kept line is there once and in order, and once the live
    /// path opens the history moves up as usual.
    #[test]
    fn a_log_that_cannot_reopen_stays_within_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let (limit, keep) = (100, 2);
        let mut log = RotatingFile::open(&path, limit, keep).unwrap();
        let write = |log: &mut RotatingFile, n: usize| {
            log.write_all(format!("{}\n", line(n)).as_bytes()).unwrap()
        };
        for n in 0..2 {
            write(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone()]));
        let files = |path: &Path| -> Vec<PathBuf> {
            (0..=keep)
                .rev()
                .map(|n| RotatingFile::rotated(path, n))
                .collect()
        };
        for n in 2..40 {
            write(&mut log, n);
            let total: u64 = files(&path)
                .into_iter()
                .chain([path.clone()])
                .filter_map(|file| std::fs::metadata(file).ok())
                .map(|m| m.len())
                .sum();
            // The file being written may pass the limit by the line that
            // opened it, as a rotation that failed once would.
            assert!(total <= (keep as u64 + 1) * limit + 40, "{total} bytes");
            let kept: Vec<String> = files(&path).iter().flat_map(|file| lines(file)).collect();
            let first = n + 1 - kept.len();
            assert_eq!(kept, (first..=n).map(line).collect::<Vec<_>>());
        }
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        write(&mut log, 40);
        drop(log);
        assert_eq!(lines(&path), [line(40)]);
        assert!(!RotatingFile::rotated(&path, 0).exists());
        let older: Vec<String> = (1..=keep)
            .rev()
            .flat_map(|n| lines(&RotatingFile::rotated(&path, n)))
            .collect();
        assert_eq!(older.last(), Some(&line(39)));
    }

    const LIMIT: u64 = 100;
    const KEEP: usize = 2;
    /// What any state may hold on disk: `name.0` to `name.<keep>` at the size
    /// limit and the file being written at its hard bound.
    const BOUND: u64 = (KEEP as u64 + 1) * LIMIT + 2 * LIMIT;
    fn write_line(log: &mut RotatingFile, n: usize) {
        log.write_all(format!("{}\n", line(n)).as_bytes()).unwrap();
    }
    /// The bytes in the log's files; a folder in their place counts nothing.
    fn on_disk(path: &Path) -> u64 {
        std::iter::once(path.to_owned())
            .chain((0..=KEEP).map(|n| RotatingFile::rotated(path, n)))
            .filter_map(|file| std::fs::metadata(file).ok())
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
            .sum()
    }
    fn size(path: &Path) -> u64 {
        std::fs::metadata(path).map_or(0, |m| m.len())
    }
    /// `found` is lines `first..=last` for some `first`: none lost in between,
    /// none twice.
    fn run_ending_at(found: &[String], last: usize) {
        // A file that was cut back starts with the log's own line saying so.
        let found = match found.split_first() {
            Some((first, rest)) if first.starts_with("--- log ") => rest,
            _ => found,
        };
        assert!(!found.iter().any(|line| line.starts_with("--- log ")));
        assert!(!found.is_empty(), "no line up to {last}");
        let first = last + 1 - found.len();
        assert_eq!(found, (first..=last).map(line).collect::<Vec<_>>());
    }

    /// A healthy log rotates at its limit and never reaches the backstop.
    #[test]
    fn a_healthy_log_rotates_at_its_limit_and_is_never_cut_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..50 {
            write_line(&mut log, n);
            assert!(size(&path) <= LIMIT);
        }
        assert_eq!(log.backstops, 0);
        run_ending_at(&lines(&RotatingFile::rotated(&path, 1)), 47);
        run_ending_at(&lines(&path), 49);
    }

    /// A program that holds the log without delete sharing keeps it from
    /// rotating; at twice its limit it is cut back instead of growing.
    #[cfg(windows)]
    #[test]
    fn a_log_held_without_delete_sharing_is_cut_back_at_twice_its_limit() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..5 {
            write_line(&mut log, n);
        }
        let older = [1, 2].map(|n| lines(&RotatingFile::rotated(&path, n)));
        // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        for n in 5..40 {
            write_line(&mut log, n);
            assert!(size(&path) <= 2 * LIMIT);
            assert!(on_disk(&path) <= BOUND);
            run_ending_at(&lines(&path), n);
        }
        assert_eq!(
            [1, 2].map(|n| lines(&RotatingFile::rotated(&path, n))),
            older
        );
        drop(reader);
    }

    /// A program that holds the oldest file without delete sharing keeps the
    /// older files from moving up; the log is cut back at twice its limit.
    #[cfg(windows)]
    #[test]
    fn a_held_older_file_keeps_the_log_within_twice_its_limit() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..6 {
            write_line(&mut log, n);
        }
        let older = [1, 2].map(|n| lines(&RotatingFile::rotated(&path, n)));
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(RotatingFile::rotated(&path, KEEP))
            .unwrap();
        for n in 6..40 {
            write_line(&mut log, n);
            assert!(size(&path) <= 2 * LIMIT);
            assert!(on_disk(&path) <= BOUND);
            run_ending_at(&lines(&path), n);
        }
        assert_eq!(
            [1, 2].map(|n| lines(&RotatingFile::rotated(&path, n))),
            older
        );
        assert_eq!(lines(&RotatingFile::rotated(&path, 0)), [line(4), line(5)]);
        drop(reader);
    }

    /// A folder made at the log's path moves up with the rotations until it
    /// blocks them for good; the log is cut back at twice its limit.
    #[test]
    fn a_folder_at_the_log_path_keeps_the_log_within_twice_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        for n in 2..60 {
            write_line(&mut log, n);
            assert!(on_disk(&path) <= BOUND);
            if path.is_file() {
                assert!(size(&path) <= 2 * LIMIT);
            }
        }
        assert!(RotatingFile::rotated(&path, KEEP).is_dir());
        run_ending_at(&lines(&path), 59);
    }

    /// When neither the log path nor `name.0` opens, the file being written
    /// is cut back at twice the limit, and the log comes back once they open.
    #[test]
    fn a_log_that_can_open_nothing_stays_within_twice_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone(), RotatingFile::rotated(&path, 0)]));
        for n in 2..40 {
            write_line(&mut log, n);
            assert!(on_disk(&path) <= BOUND);
        }
        run_ending_at(&lines(&RotatingFile::rotated(&path, 1)), 39);
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        write_line(&mut log, 40);
        assert_eq!(lines(&path), [line(40)]);
    }

    /// A file that cannot be cut back either (a mapped file) drops lines, and
    /// is tried again only once per MiB that would have been written.
    #[test]
    fn a_file_that_cannot_be_cut_back_drops_lines_and_tries_again_per_mib() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone(), RotatingFile::rotated(&path, 0)]));
        FAIL_TRUNCATE.with(|fail| fail.set(true));
        for n in 2..40 {
            write_line(&mut log, n);
            assert!(on_disk(&path) <= BOUND);
        }
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        FAIL_TRUNCATE.with(|fail| fail.set(false));
        let mut n = 40;
        while !path.exists() {
            write_line(&mut log, n);
            n += 1;
            assert!(
                n < 40 + RETRY_DROPPED as usize / 40 + 2,
                "never tried again"
            );
        }
        // About 1 MiB of lines went by before the next try.
        assert!(
            n > 40 + RETRY_DROPPED as usize / 40 - 40,
            "tried again after {n}"
        );
        // Writing goes on with one line saying how much was dropped.
        let found = lines(&path);
        assert_eq!(found.len(), 2, "{found:?}");
        let dropped: u64 = found[0]
            .strip_prefix("--- log lines dropped: ")
            .and_then(|rest| rest.split(' ').next())
            .and_then(|bytes| bytes.parse().ok())
            .unwrap_or_else(|| panic!("{found:?}"));
        assert_eq!(dropped % 40, 0);
        assert!(dropped > RETRY_DROPPED - 40 * 40, "{dropped}");
        assert_eq!(found[1], line(n - 1));
    }

    /// The log's folder deleted while the log is open comes back with the log.
    #[test]
    fn a_log_whose_folder_was_deleted_comes_back() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("logs");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..4 {
            write_line(&mut log, n);
        }
        for entry in std::fs::read_dir(&folder).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(&folder).unwrap();
        // 80 bytes in the deleted file: the next line passes the limit.
        write_line(&mut log, 4);
        write_line(&mut log, 5);
        assert_eq!(lines(&path), [line(4), line(5)]);
    }

    /// A deleted log whose path cannot be opened again yet goes on in `name.0`
    /// at once rather than in the deleted file.
    #[test]
    fn a_deleted_log_that_cannot_reopen_goes_on_in_name_0_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        std::fs::remove_file(&path).unwrap();
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone()]));
        write_line(&mut log, 2);
        write_line(&mut log, 3);
        assert_eq!(lines(&RotatingFile::rotated(&path, 0)), [line(2), line(3)]);
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        write_line(&mut log, 4);
        assert_eq!(lines(&path), [line(4)]);
        assert_eq!(lines(&RotatingFile::rotated(&path, 1)), [line(2), line(3)]);
    }

    /// A cut leaves one line of the log's own at the start of the cut file,
    /// saying how much went; it counts towards the file's size.
    #[test]
    fn a_cut_back_log_says_so_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone(), RotatingFile::rotated(&path, 0)]));
        // The file being written reaches 200 bytes with line 4; line 5 cuts it.
        for n in 2..7 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        assert_eq!(log.backstops, 1);
        let written = RotatingFile::rotated(&path, 1);
        assert_eq!(
            lines(&written),
            [CUT_NOTE.replace("{}", "200"), line(5), line(6)]
        );
        assert_eq!(log.size, size(&written));
    }

    /// A log at the log path that is already over twice the limit when it
    /// opens again is rotated, not cut back.
    #[test]
    fn a_reopened_log_over_its_limit_is_rotated_not_cut_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        for n in 0..2 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone()]));
        write_line(&mut log, 2);
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        // Another writer put a file of 250 bytes at the log path meanwhile.
        std::fs::write(&path, vec![b'x'; 250]).unwrap();
        write_line(&mut log, 3);
        assert_eq!(log.backstops, 0);
        assert_eq!(lines(&path), [line(3)]);
        assert_eq!(
            std::fs::read(RotatingFile::rotated(&path, 1)).unwrap(),
            vec![b'x'; 250]
        );
        assert_eq!(
            lines(&RotatingFile::rotated(&path, 2)),
            [line(0), line(1), line(2)]
        );
    }

    /// A log over twice its limit whose rotation fails at start is rotated by
    /// the first line once the holder let go.
    #[cfg(windows)]
    #[test]
    fn an_oversized_log_whose_holder_let_go_rotates_on_the_first_line() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        std::fs::write(&path, vec![b'x'; 250]).unwrap();
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        drop(reader);
        log.write_all(b"after\n").unwrap();
        assert_eq!(log.backstops, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
        assert_eq!(
            std::fs::read(RotatingFile::rotated(&path, 1)).unwrap(),
            vec![b'x'; 250]
        );
    }

    /// A log over twice its limit that still cannot be rotated on its first
    /// line is cut back, and says so.
    #[cfg(windows)]
    #[test]
    fn an_oversized_log_that_still_cannot_rotate_is_cut_back_and_says_so() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        std::fs::write(&path, vec![b'x'; 250]).unwrap();
        let _reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        log.write_all(b"after\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{}\nafter\n", CUT_NOTE.replace("{}", "250"))
        );
    }

    /// A quiet log deleted while open comes back with the first line written
    /// once the check interval has passed, without 1 MiB of lines.
    #[test]
    fn a_quiet_log_deleted_while_open_comes_back_after_the_check_interval() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 1 << 30, KEEP).unwrap();
        log.every = Duration::ZERO;
        log.write_all(b"before\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        log.write_all(b"after\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
    }

    /// Dropped lines are tried again once the check interval has passed, not
    /// only after 1 MiB of them.
    #[test]
    fn dropped_lines_are_tried_again_after_the_check_interval() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, LIMIT, KEEP).unwrap();
        log.every = Duration::ZERO;
        for n in 0..2 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone(), RotatingFile::rotated(&path, 0)]));
        FAIL_TRUNCATE.with(|fail| fail.set(true));
        for n in 2..10 {
            write_line(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        FAIL_TRUNCATE.with(|fail| fail.set(false));
        write_line(&mut log, 10);
        assert_eq!(lines(&path).last(), Some(&line(10)));
    }

    /// A log deleted before its size limit, whose path cannot be opened again
    /// yet, goes on in `name.0` once the existence check notices it.
    #[test]
    fn a_log_deleted_before_its_limit_that_cannot_reopen_goes_on_in_name_0() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 1 << 30, KEEP).unwrap();
        log.write_all(b"before\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        FAIL_OPEN.with(|fail| fail.replace(vec![path.clone()]));
        let chunk = vec![b'x'; 4095];
        for _ in 0..(EXISTS_CHECK / 4096 + 1) {
            log.write_all(&chunk).unwrap();
            log.write_all(b"\n").unwrap();
        }
        log.write_all(b"after\n").unwrap();
        FAIL_OPEN.with(|fail| fail.replace(Vec::new()));
        let staged = std::fs::read_to_string(RotatingFile::rotated(&path, 0)).unwrap();
        assert!(staged.ends_with("\nafter\n"));
        assert!(staged.len() < 2 * 4096 + 7, "{} bytes", staged.len());
        assert!(!path.exists());
    }

    #[test]
    fn logs_roll_over_and_keep_a_bounded_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        for line in 0..10 {
            log.write_all(format!("{line:039}\n").as_bytes()).unwrap();
        }
        drop(log);
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        assert!(size(&path).unwrap() <= 100);
        assert!(size(&RotatingFile::rotated(&path, 1)).unwrap() <= 100);
        assert!(size(&RotatingFile::rotated(&path, 2)).is_some());
        assert!(size(&RotatingFile::rotated(&path, 3)).is_none());
        // The newest line is in the current file.
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with(&format!("{:039}\n", 9))
        );
        // An oversized file rolls over when opened again.
        std::fs::write(&path, vec![b'x'; 150]).unwrap();
        RotatingFile::open(&path, 100, 2).unwrap();
        assert_eq!(size(&path), Some(0));
        assert_eq!(size(&RotatingFile::rotated(&path, 1)), Some(150));
    }

    /// A reader that holds the log without delete sharing blocks the rotation on
    /// every write; the older files stay as they were and no line is lost.
    #[cfg(windows)]
    #[test]
    fn a_rotation_that_cannot_move_the_open_file_keeps_the_older_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        let write = |log: &mut RotatingFile, line: usize| {
            log.write_all(format!("{line:039}\n").as_bytes()).unwrap()
        };
        for line in 0..5 {
            write(&mut log, line);
        }
        let older =
            |path: &Path| [1, 2].map(|n| std::fs::read(RotatingFile::rotated(path, n)).ok());
        let before = older(&path);
        assert!(before.iter().all(Option::is_some));
        // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        // Up to twice the limit, where the backstop would cut the file back.
        for line in 5..9 {
            write(&mut log, line);
        }
        assert_eq!(older(&path), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 5);
        // Once the reader lets go, the next write rotates again.
        drop(reader);
        write(&mut log, 9);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(
            std::fs::read_to_string(RotatingFile::rotated(&path, 1))
                .unwrap()
                .lines()
                .count(),
            5
        );
    }

    /// A reader that holds an oversized log without delete sharing makes the
    /// rotation at open fail; the log still opens and appends.
    #[cfg(windows)]
    #[test]
    fn an_oversized_log_whose_rotation_fails_still_opens_and_appends() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.log");
        std::fs::write(&path, vec![b'x'; 150]).unwrap();
        // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE.
        let _reader = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        log.write_all(b"after\n").unwrap();
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("after\n"));
    }
}
