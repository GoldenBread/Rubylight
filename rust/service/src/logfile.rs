//! service.log, size-limited as the host's logs are
//! (`butterpollo_core::logfile`): when it would exceed its limit it becomes
//! `service.log.1`, older files move up to `service.log.<keep>` and the
//! oldest is dropped, so a long-running service cannot fill the disk.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How many bytes the log writes between checks that its path is still there.
const EXISTS_CHECK: u64 = 1 << 20;

pub struct RotatingFile {
    path: PathBuf,
    file: File,
    size: u64,
    limit: u64,
    keep: usize,
    /// The open file is no longer at the live path (moved to `name.0` by a
    /// rotation, or deleted or moved away by another program) and the live
    /// path could not be opened again yet.
    detached: bool,
}
impl RotatingFile {
    /// The host's default: 32 MiB per file and four older files.
    pub fn open_default(path: &Path) -> io::Result<Self> {
        Self::open(path, 32 * 1024 * 1024, 4)
    }
    pub fn open(path: &Path, limit: u64, keep: usize) -> io::Result<Self> {
        let mut log = Self {
            path: path.to_owned(),
            file: append(path)?,
            size: std::fs::metadata(path).map_or(0, |m| m.len()),
            limit: limit.max(1),
            keep: keep.max(1),
            detached: false,
        };
        if log.size >= log.limit {
            // A failed rotation is not fatal: keep appending to the current
            // file, as `write` does, rather than keep the service from starting.
            let _ = log.rotate();
        }
        Ok(log)
    }
    /// `name.<n>`.
    fn rotated(path: &Path, n: usize) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }
    fn rotate(&mut self) -> io::Result<()> {
        // As the host's log (`butterpollo_core::logfile`): the open file moves
        // first, to `name.0`: when a reader without delete sharing holds it, no
        // older file has moved or gone yet, so a rotation that fails on every
        // write costs no history. Windows allows renaming the open file: Rust
        // opens files shareable for deletion, as do readers that follow the
        // log. A `name.0` left by a rotation that failed below moves up before
        // the open file takes its place.
        let staged = Self::rotated(&self.path, 0);
        if !staged.exists() {
            match std::fs::rename(&self.path, &staged) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Another program deleted or moved the log away: the open
                    // file is no longer the log. Nothing was moved aside, so
                    // no older file moves up; the live path opens again.
                    self.detached = true;
                    self.reopen();
                    return Ok(());
                }
                result => result?,
            }
            // Until the live path opens, writes go on into `name.0`, and the
            // next write tries to open it again.
            self.detached = true;
            self.file = append(&self.path)?;
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
        self.file = append(&staged)?;
        self.size = 0;
        Ok(())
    }
    /// Opens the live path again after a rotation moved the open file away but
    /// could not open it; then the moved file takes its place among the older
    /// ones. While it still fails, writing goes on into the moved file.
    fn reopen(&mut self) {
        let Ok(file) = append(&self.path) else {
            return;
        };
        self.size = file.metadata().map_or(0, |m| m.len());
        self.file = file;
        self.detached = false;
        if Self::rotated(&self.path, 0).exists() {
            // Moves only the older files: `name.0` is there, so the reopened
            // file stays the live one.
            let _ = self.rotate();
        }
    }
}
/// How often a service log that could not be opened is tried again.
const LOG_RETRY: Duration = Duration::from_secs(60);
/// The service's log, `service.log` in `directory`. Opening it never fails: a
/// log the service cannot use (read-only, a folder in its place, held by a
/// program without write sharing, a file in place of the folder) must not keep
/// the service, and with it the host, from starting. Until the log opens, its
/// lines are dropped, and it is tried again at most every `LOG_RETRY`.
pub fn service_log(directory: &Path) -> ServiceLog {
    ServiceLog::new(directory, LOG_RETRY)
}
pub struct ServiceLog {
    directory: PathBuf,
    file: Option<RotatingFile>,
    every: Duration,
    next: Instant,
}
impl ServiceLog {
    fn new(directory: &Path, every: Duration) -> Self {
        let mut log = Self {
            directory: directory.to_owned(),
            file: None,
            every,
            next: Instant::now(),
        };
        log.try_open();
        log
    }
    fn try_open(&mut self) {
        self.file = std::fs::create_dir_all(&self.directory)
            .and_then(|()| RotatingFile::open_default(&self.directory.join("service.log")))
            .ok();
        self.next = Instant::now() + self.every;
    }
}
impl Write for ServiceLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.file.is_none() && Instant::now() >= self.next {
            self.try_open();
        }
        match &mut self.file {
            Some(file) => file.write(buf),
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}
fn append(path: &Path) -> io::Result<File> {
    #[cfg(test)]
    if tests::FAIL_OPEN.with(|fail| fail.borrow().as_deref() == Some(path)) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    OpenOptions::new().create(true).append(true).open(path)
}
impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // If rotation fails (a reader without delete sharing holds the old
        // file), keep writing to the current file rather than lose lines.
        let full = |log: &Self| log.size > 0 && log.size + buf.len() as u64 > log.limit;
        if self.detached {
            self.reopen();
            if self.detached && full(self) {
                let _ = self.roll_detached();
            }
        } else if full(self) {
            let _ = self.rotate();
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
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    thread_local! {
        /// Makes opening this path fail, as a denied or locked path would.
        pub(super) static FAIL_OPEN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    /// A rotation that moves the open file but cannot open the live path again
    /// keeps the lines in the moved file, and the next write opens the live
    /// path again instead of leaving it missing.
    #[test]
    fn a_rotation_that_cannot_reopen_the_log_opens_it_on_the_next_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        let write = |log: &mut RotatingFile, line: usize| {
            log.write_all(format!("{line:039}\n").as_bytes()).unwrap()
        };
        for line in 0..2 {
            write(&mut log, line);
        }
        FAIL_OPEN.with(|fail| fail.replace(Some(path.clone())));
        write(&mut log, 2);
        FAIL_OPEN.with(|fail| fail.replace(None));
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

    /// The service's log setup, as `main` runs it.
    fn setup(directory: &Path) -> Box<dyn Write> {
        Box::new(service_log(directory))
    }

    /// A `service.log` the service cannot open does not keep it from starting.
    #[test]
    fn a_read_only_log_does_not_stop_the_service() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service.log");
        std::fs::write(&path, b"").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();
        setup(directory.path()).write_all(b"line\n").unwrap();
        // Writable again only so the temporary folder can be removed.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
    }

    #[test]
    fn a_folder_in_place_of_the_log_does_not_stop_the_service() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("service.log")).unwrap();
        setup(directory.path()).write_all(b"line\n").unwrap();
    }

    #[test]
    fn a_file_in_place_of_the_logs_folder_does_not_stop_the_service() {
        let directory = tempfile::tempdir().unwrap();
        let logs = directory.path().join("logs");
        std::fs::write(&logs, b"").unwrap();
        setup(&logs).write_all(b"line\n").unwrap();
    }

    /// A program that holds the log without write sharing does not keep the
    /// service from starting; once it lets go, the log is opened again.
    #[cfg(windows)]
    #[test]
    fn a_log_held_without_write_sharing_does_not_stop_the_service() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service.log");
        std::fs::write(&path, b"").unwrap();
        // FILE_SHARE_READ only.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        setup(directory.path()).write_all(b"line\n").unwrap();
        let mut log = ServiceLog::new(directory.path(), Duration::ZERO);
        log.write_all(b"dropped\n").unwrap();
        drop(reader);
        log.write_all(b"kept\n").unwrap();
        drop(log);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept\n");
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
        let path = directory.path().join("service.log");
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
        let path = directory.path().join("service.log");
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
        let path = directory.path().join("service.log");
        let (limit, keep) = (100, 2);
        let mut log = RotatingFile::open(&path, limit, keep).unwrap();
        let write = |log: &mut RotatingFile, n: usize| {
            log.write_all(format!("{}\n", line(n)).as_bytes()).unwrap()
        };
        for n in 0..2 {
            write(&mut log, n);
        }
        FAIL_OPEN.with(|fail| fail.replace(Some(path.clone())));
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
        FAIL_OPEN.with(|fail| fail.replace(None));
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

    #[test]
    fn logs_roll_over_and_keep_a_bounded_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service.log");
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
        let path = directory.path().join("service.log");
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
        for line in 5..12 {
            write(&mut log, line);
        }
        assert_eq!(older(&path), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 8);
        // Once the reader lets go, the next write rotates again.
        drop(reader);
        write(&mut log, 12);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(
            std::fs::read_to_string(RotatingFile::rotated(&path, 1))
                .unwrap()
                .lines()
                .count(),
            8
        );
    }

    /// A reader that holds an oversized log without delete sharing makes the
    /// rotation at open fail; the log still opens and appends.
    #[cfg(windows)]
    #[test]
    fn an_oversized_log_whose_rotation_fails_still_opens_and_appends() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("service.log");
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
