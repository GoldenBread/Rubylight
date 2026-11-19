//! service.log, size-limited as the host's logs are
//! (`butterpollo_core::logfile`): when it would exceed its limit it becomes
//! `service.log.1`, older files move up to `service.log.<keep>` and the
//! oldest is dropped, so a long-running service cannot fill the disk.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub struct RotatingFile {
    path: PathBuf,
    file: File,
    size: u64,
    limit: u64,
    keep: usize,
    /// The open file was moved to `name.1` and the live path could not be
    /// opened again.
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
        let _ = std::fs::remove_file(Self::rotated(&self.path, self.keep));
        for n in (1..self.keep).rev() {
            match std::fs::rename(
                Self::rotated(&self.path, n),
                Self::rotated(&self.path, n + 1),
            ) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        // Windows allows renaming the open file: Rust opens files shareable
        // for deletion, as do readers that follow the log.
        std::fs::rename(&self.path, Self::rotated(&self.path, 1))?;
        // Until the live path opens, writes go on into `name.1`, and the
        // next write tries to open it again.
        self.detached = true;
        self.file = append(&self.path)?;
        self.detached = false;
        self.size = 0;
        Ok(())
    }
    /// Opens the live path again after a rotation moved the open file away but
    /// could not open it. While it still fails, writing goes on into the moved
    /// file, which no rotation moves meanwhile.
    fn reopen(&mut self) {
        let Ok(file) = append(&self.path) else {
            return;
        };
        self.size = file.metadata().map_or(0, |m| m.len());
        self.file = file;
        self.detached = false;
    }
}
fn append(path: &Path) -> io::Result<File> {
    #[cfg(test)]
    if tests::FAIL_OPEN.with(std::cell::Cell::get) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    OpenOptions::new().create(true).append(true).open(path)
}
impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // If rotation fails (a reader without delete sharing holds the old
        // file), keep writing to the current file rather than lose lines.
        if self.detached {
            self.reopen();
        } else if self.size > 0 && self.size + buf.len() as u64 > self.limit {
            let _ = self.rotate();
        }
        let written = self.file.write(buf)?;
        self.size += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    thread_local! {
        /// Makes opening a log path fail, as a denied or locked path would.
        pub(super) static FAIL_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
        FAIL_OPEN.with(|fail| fail.set(true));
        write(&mut log, 2);
        FAIL_OPEN.with(|fail| fail.set(false));
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

    /// A reader that holds an oversized log without delete sharing makes the
    /// rotation at open fail; the log still opens and appends, so the service
    /// starts.
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
