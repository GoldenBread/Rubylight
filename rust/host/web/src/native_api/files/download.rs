//! Streaming file downloads for Native API `/files/content` and `/folders/{id}/content`.
use super::{CHUNK, DownloadSlot, PACE_CHECK, QUEUE, Recheck, paced_blocking};
use crate::native_api::Failure;
use axum::{
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_stream::state::Shared;
use std::{
    fs::File,
    io::{self, Read},
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tokio::sync::{mpsc, oneshot};

/// Answers with the `length` bytes of `file`, read on a blocking thread
/// through the pacer while `recheck` holds. The slot frees once the file is
/// closed.
pub(in crate::native_api) fn send_file(
    h: Shared,
    slot: DownloadSlot,
    file: File,
    length: u64,
    mut recheck: Recheck,
) -> Response {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (fail, failed) = oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let idle = h.file_transfers.limits().idle;
    tokio::task::spawn_blocking(move || {
        // Dropped after the file, so the slot frees once it is closed.
        let _slot = slot;
        let mut file = file;
        let refused =
            |refusal: Failure| io::Error::new(io::ErrorKind::PermissionDenied, refusal.message);
        let sent = read_exactly(&mut file, length, |chunk| {
            let wait = h.file_transfers.pace(&h, chunk.len() as u64);
            paced_blocking(wait, &mut recheck).map_err(refused)?;
            let stalled = Instant::now() + idle;
            let permit = loop {
                match runtime.block_on(tokio::time::timeout(PACE_CHECK, tx.reserve())) {
                    Ok(Ok(permit)) => break permit,
                    // The client went away.
                    Ok(Err(_)) => return Ok(false),
                    Err(_) if Instant::now() >= stalled => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("the device took nothing for {} s", idle.as_secs()),
                        ));
                    }
                    Err(_) => recheck.check().map_err(refused)?,
                }
            };
            permit.send(chunk);
            Ok(true)
        });
        if let Err(error) = sent {
            tracing::warn!("File transfer stopped a download: {error}");
            // Sent before the queue closes, so the body ends with it after
            // the chunks queued.
            let _ = fail.send(error);
        }
    });
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        Chunks {
            rx,
            failed,
            current: Bytes::new(),
        },
        CHUNK,
    ));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response()
}

/// Hands `length` bytes of `file` to `send`, and fails if the file holds
/// fewer or more. The last chunk waits until the end of the file is seen.
/// Stops quietly when `send` reports that the client went away (`false`),
/// and with `send`'s error.
pub(super) fn read_exactly(
    file: &mut impl Read,
    length: u64,
    mut send: impl FnMut(Bytes) -> io::Result<bool>,
) -> io::Result<()> {
    let changed = |how: &str| io::Error::other(format!("the file {how} while it was sent"));
    let mut left = length;
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return if left == 0 {
                Ok(())
            } else {
                Err(changed("shrank"))
            };
        }
        if read as u64 > left {
            return Err(changed("grew"));
        }
        left -= read as u64;
        if left == 0 {
            let mut probe = [0; 1];
            let more = loop {
                match file.read(&mut probe) {
                    Ok(more) => break more,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            };
            if more != 0 {
                return Err(changed("grew"));
            }
        }
        if !send(Bytes::copy_from_slice(&buffer[..read]))? || left == 0 {
            return Ok(());
        }
    }
}

/// The download's chunks as a reader. A download that failed ends the body
/// with its error once the chunks queued are read.
struct Chunks {
    rx: mpsc::Receiver<Bytes>,
    failed: oneshot::Receiver<io::Error>,
    current: Bytes,
}
impl tokio::io::AsyncRead for Chunks {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while this.current.is_empty() {
            match std::task::ready!(this.rx.poll_recv(cx)) {
                Some(chunk) => this.current = chunk,
                None => {
                    return Poll::Ready(match this.failed.try_recv() {
                        Ok(error) => Err(error),
                        Err(_) => Ok(()),
                    });
                }
            }
        }
        let n = buf.remaining().min(this.current.len());
        buf.put_slice(&this.current.split_to(n));
        Poll::Ready(Ok(()))
    }
}
