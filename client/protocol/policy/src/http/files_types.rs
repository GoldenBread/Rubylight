//! File transfer wire types and refusal/cancel handles.

use super::Error;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

type CancelHook = (u64, Arc<dyn Fn() + Send + Sync>);

/// Cancels one upload or download from another thread, at once, even while it waits on the
/// host: any registered abort hooks run immediately.
#[derive(Default)]
pub struct TransferCancel {
    cancelled: AtomicBool,
    next: AtomicU64,
    hooks: Mutex<Vec<CancelHook>>,
}

impl TransferCancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let hooks: Vec<_> = self.hooks.lock().unwrap().drain(..).collect();
        for (_, hook) in hooks {
            hook();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn register_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) -> Result<u64, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.hooks.lock().unwrap().push((id, hook.clone()));
        if self.is_cancelled() {
            hook();
            self.unregister_hook(id);
            return Err(Error::Cancelled);
        }
        Ok(id)
    }

    pub fn unregister_hook(&self, id: u64) {
        self.hooks.lock().unwrap().retain(|(i, _)| *i != id);
    }
}

/// One file in the host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteFile {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileList {
    /// Where the files land on the host, for display.
    pub folder: String,
    pub free_bytes: u64,
    /// The largest upload the host takes.
    pub max_file_bytes: u64,
    pub files: Vec<RemoteFile>,
}

/// A finished upload: the name the host stored it under, which gains " (n)" when the name was
/// taken, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Uploaded {
    pub name: String,
    pub size: u64,
}

/// A refusal the host documents, by HTTP status and error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileRefusal {
    /// 403 `forbidden`: this device lacks the permission.
    Forbidden,
    /// 403 `file_transfer_disabled`: file transfer is off on the host.
    FileTransferDisabled,
    /// 400 `invalid_name`.
    InvalidName,
    /// 400 `incomplete_upload`: the body ended before its Content-Length.
    IncompleteUpload,
    /// 411 `length_required`.
    LengthRequired,
    /// 413 `too_large`: over `max_file_bytes`.
    TooLarge,
    /// 507 `insufficient_storage`.
    InsufficientStorage,
    /// 409 `no_console_user`: nobody is signed in on the host.
    NoConsoleUser,
    /// 409 `transfer_busy`: another upload from this device is running.
    TransferBusy,
    /// 404 `not_found`: no such file.
    NotFound,
    /// 403 `folders_disabled`: the host shares no folders.
    FoldersDisabled,
    /// 403 `folder_unreadable`: Windows denies the host's signed-in user reading the shared
    /// folder, or a folder or file in it.
    FolderUnreadable,
}

impl FileRefusal {
    const ALL: [(u16, &'static str, FileRefusal); 12] = [
        (403, "forbidden", FileRefusal::Forbidden),
        (
            403,
            "file_transfer_disabled",
            FileRefusal::FileTransferDisabled,
        ),
        (400, "invalid_name", FileRefusal::InvalidName),
        (400, "incomplete_upload", FileRefusal::IncompleteUpload),
        (411, "length_required", FileRefusal::LengthRequired),
        (413, "too_large", FileRefusal::TooLarge),
        (
            507,
            "insufficient_storage",
            FileRefusal::InsufficientStorage,
        ),
        (409, "no_console_user", FileRefusal::NoConsoleUser),
        (409, "transfer_busy", FileRefusal::TransferBusy),
        (404, "not_found", FileRefusal::NotFound),
        (403, "folders_disabled", FileRefusal::FoldersDisabled),
        (403, "folder_unreadable", FileRefusal::FolderUnreadable),
    ];

    pub fn from_answer(status: u16, code: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(s, c, _)| *s == status && *c == code)
            .map(|(_, _, refusal)| *refusal)
    }

    /// The host's error code.
    pub fn code(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, _, refusal)| *refusal == self)
            .map_or("", |(_, code, _)| code)
    }
}

/// Why a file call gave no success.
#[derive(Debug)]
pub enum FileError {
    /// The host answered with a documented refusal.
    Refused {
        refusal: FileRefusal,
        status: u16,
        message: String,
    },
    /// Reading the file to upload, or writing the download, failed on this device.
    Local(std::io::Error),
    Host(Error),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Refused {
                status, message, ..
            } => write!(f, "host error {status}: {message}"),
            FileError::Local(e) => write!(f, "local file: {e}"),
            FileError::Host(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileError::Local(e) => Some(e),
            FileError::Host(e) => Some(e),
            FileError::Refused { .. } => None,
        }
    }
}

impl From<Error> for FileError {
    fn from(e: Error) -> Self {
        FileError::Host(e)
    }
}
