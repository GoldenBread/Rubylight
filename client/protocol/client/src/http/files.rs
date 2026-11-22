//! File transfer under the native API (`/api/native/v1/files`): HostClient calls.
//! Wire DTOs live in `nightjar_policy::http`.

pub use nightjar_policy::http::{
    FileError, FileList, FileRefusal, RemoteFile, TransferCancel, Uploaded,
};
use super::native::error_of;
use super::native_api::PREFIX;
use super::transport::{self, encode_query, Request, Timeouts, Upload};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};
use serde::Deserialize;
use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::time::Duration;

const LIST_PATH: &str = "files";
const CONTENT_PATH: &str = "files/content";
pub(super) const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_JSON_BYTES: usize = 1024 * 1024;
pub(super) const TRANSFER_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Deserialize)]
struct ErrorBody {
    error: RawError,
}

#[derive(Deserialize)]
struct RawError {
    code: String,
    #[serde(default)]
    message: String,
}

/// A non-2xx answer: a documented refusal, or the native API's error for anything else.
fn refusal(response: transport::Response, path: &str) -> FileError {
    if let Ok(ErrorBody { error }) = serde_json::from_slice(&response.body)
        && let Some(refusal) = nightjar_policy::http::FileRefusal::from_answer(response.status, &error.code)
    {
        return FileError::Refused {
            refusal,
            status: response.status,
            message: error.message,
        };
    }
    FileError::Host(error_of(response, path))
}

/// Reads an answer to `path`: `success` is the one status that is not a refusal.
pub(super) fn decode<T: serde::de::DeserializeOwned>(
    response: transport::Response,
    path: &str,
    success: u16,
) -> Result<T, FileError> {
    if response.status == success {
        return serde_json::from_slice(&response.body)
            .map_err(|e| Error::Malformed(format!("native API {path}: {e}")).into());
    }
    if (200..=299).contains(&response.status) {
        return Err(Error::Status {
            code: i32::from(response.status),
            message: response.reason,
        }
        .into());
    }
    Err(refusal(response, path))
}

/// Reads an upload's answer: 201, and the host stored as many bytes as were sent.
pub(super) fn uploaded(response: transport::Response, path: &str, length: u64) -> Result<Uploaded, FileError> {
    let uploaded: Uploaded = decode(response, path, 201)?;
    if uploaded.size != length {
        return Err(Error::Malformed(format!(
            "native API {path}: the host stored {} of {length} bytes",
            uploaded.size
        ))
        .into());
    }
    Ok(uploaded)
}

/// Reads a download's answer: only a 200 delivered the `length` bytes in the sink.
pub(super) fn downloaded(response: transport::Response, path: &str, length: u64) -> Result<u64, FileError> {
    match response.status {
        200 => Ok(length),
        201..=299 => Err(Error::Status {
            code: i32::from(response.status),
            message: response.reason,
        }
        .into()),
        _ => Err(refusal(response, path)),
    }
}

/// The local side of a transfer. Keeps its first error, so that a full disk is not taken for a
/// network failure.
pub(super) struct Local<T> {
    inner: T,
    error: Option<std::io::Error>,
}

impl<T> Local<T> {
    pub(super) fn new(inner: T) -> Self {
        Self { inner, error: None }
    }

    fn keep(&mut self, error: std::io::Error) -> std::io::Error {
        if error.kind() == ErrorKind::Interrupted {
            return error;
        }
        let copy = std::io::Error::new(error.kind(), error.to_string());
        self.error.get_or_insert(error);
        copy
    }

    /// `error`, unless the local side failed first. A cancel wins: what failed after it does
    /// not matter.
    pub(super) fn or(self, error: Error) -> FileError {
        match (self.error, error) {
            (_, Error::Cancelled) => FileError::Host(Error::Cancelled),
            (Some(local), _) => FileError::Local(local),
            (None, error) => FileError::Host(error),
        }
    }
}

impl<R: Read> Read for Local<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(bytes).map_err(|e| self.keep(e))
    }
}

impl<W: Write> Write for Local<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes).map_err(|e| self.keep(e))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().map_err(|e| self.keep(e))
    }
}

impl HostClient {
    pub(super) fn files_request<'a>(
        &'a self,
        target: &'a str,
        limit: usize,
        read: Duration,
    ) -> Result<Request<'a>, Error> {
        Ok(Request {
            host: &self.host,
            port: self.https_port(true)?,
            target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(read),
            },
            body_limit: limit,
        })
    }

    fn files_target(&self, path: &str, name: Option<&str>) -> (String, String) {
        let path = format!("{}/{path}", PREFIX.trim_start_matches('/'));
        let query = name.map(|name| format!("name={}", encode_query(name)));
        let target = self.url_target(&path, query.as_deref());
        (path, target)
    }

    /// The host's transfer folder. Needs the device's `file_download` permission.
    pub fn list_files(&self) -> Result<FileList, FileError> {
        let (path, target) = self.files_target(LIST_PATH, None);
        let request = self.files_request(&target, MAX_LIST_BYTES, READ_TIMEOUT)?;
        decode(transport::get(&request, &self.cancel)?, &path, 200)
    }

    /// Uploads exactly `length` bytes of `file` as `name`, reporting progress at 0, at most
    /// every 256 KiB and 100 ms, and at `length`. `stop` ends the upload with `Error::Cancelled`;
    /// the host then drops the partial file. Needs the device's `file_upload` permission.
    pub fn upload_file(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        let (path, target) = self.files_target(LIST_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut file = Local::new(file);
        let upload = Upload {
            reader: &mut file,
            length,
            content_type: "application/octet-stream",
            method: "POST",
        };
        let cancel = Arc::new(transport::Cancel::default());
        let hook = stop.register_hook(Arc::new({ let cancel = cancel.clone(); move || cancel.cancel() }))?;
        let res = transport::upload(&request, &self.cancel, upload, progress, &cancel);
        stop.unregister_hook(hook);
        match res {
            Ok(response) => uploaded(response, &path, length),
            Err(error) => Err(file.or(error)),
        }
    }

    /// Downloads `name`, which the list gave as `listed` bytes, into `sink` and returns its
    /// length, reporting progress like `upload_file`. A body shorter than its Content-Length is
    /// an error, as is one longer than `listed` or cancelling `stop`; the caller then discards
    /// what `sink` got. Needs the device's `file_download` permission.
    pub fn download_file(
        &self,
        name: &str,
        listed: u64,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        let (path, target) = self.files_target(CONTENT_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        download_listed(&request, &self.cancel, &path, listed, sink, progress, stop)
    }
}

/// A download's sink that takes no more than the size the listing gave, so a host that sends
/// more stops at that size instead of filling the disk.
struct Listed<'a> {
    inner: &'a mut dyn Write,
    left: u64,
    over: bool,
}

impl Write for Listed<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.left {
            self.over = true;
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "more than the listed size",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.left -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Runs the download `request` of `path`, listed at `listed` bytes, into `sink`: what
/// [`HostClient::download_file`] and `download_folder_file` share.
pub(super) fn download_listed(
    request: &Request<'_>,
    cancel: &transport::Cancel,
    path: &str,
    listed: u64,
    sink: &mut dyn Write,
    progress: &mut dyn FnMut(u64, u64),
    stop: &TransferCancel,
) -> Result<u64, FileError> {
    let mut sink = Local::new(Listed {
        inner: sink,
        left: listed,
        over: false,
    });
    let mut length = 0;
    let mut counted = |done, total| {
        length = done;
        progress(done, total);
    };
    let stop_cancel = Arc::new(transport::Cancel::default());
    let hook = stop.register_hook(Arc::new({ let stop_cancel = stop_cancel.clone(); move || stop_cancel.cancel() }))?;
    let res = transport::download(request, cancel, &mut sink, &mut counted, &stop_cancel);
    stop.unregister_hook(hook);
    match res {
        Ok(response) => downloaded(response, path, length),
        Err(_) if sink.inner.over => Err(Error::Malformed(format!(
            "native API {path}: the host sent more than the {listed} bytes it listed"
        ))
        .into()),
        Err(error) => Err(sink.or(error)),
    }
}

#[cfg(test)]
mod tests;
