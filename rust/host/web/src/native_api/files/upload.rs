//! Streaming file uploads, connection closing/draining, and `.part` disk writer for Native API transfers.
use super::{
    PACE_CHECK, QUEUE, Recheck, UploadSlot, allowed, begin_upload, failure, name_of, open_folder,
    paced, transfer_dir,
};
use crate::native_api::{ApiFailure, Failure, Reply, caller, json};
use axum::{
    Extension,
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_session::native::FILE_UPLOAD;
use butterpollo_stream::{state::Shared, tls::Connection};
use butterpollo_win_io::user_folder::{PartFile, UserFolderError};
use rubylight_native_api::{error::ErrorCode, files::Uploaded};
use std::{
    io::Write,
    pin::Pin,
    sync::PoisonError,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

/// `POST /files?name=`. A caller that isn't a paired device is answered and
/// the connection closed at once. Any other refusal answers at once too;
/// the rest of the body is then read and dropped in the background for a
/// while ([`drain`]), so the client, which sends all of it before it reads
/// the answer, gets the answer rather than a reset.
pub(in crate::native_api) async fn upload(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Response {
    let (parts, mut body) = request.into_parts();
    if let Err(refusal) = caller(&h, &c) {
        // Reading on would let anyone send the host data.
        refused_upload(&c, &refusal);
        return closing(ApiFailure(refusal).into_response());
    }
    let mut response = match receive(&h, &c, parts.uri.query(), &parts.headers, &mut body).await {
        Ok(response) => response,
        Err(failure) => {
            refused_upload(&c, &failure.0);
            failure.into_response()
        }
    };
    if !body.is_end_stream() {
        tokio::spawn(drain(h, body));
        response = closing(response);
    }
    response
}

fn refused_upload(c: &Connection, failure: &Failure) {
    tracing::warn!(
        "File transfer refused an upload from {}: {}",
        c.peer.ip(),
        failure.message
    );
}

/// The connection closes after this answer.
pub(in crate::native_api) fn closing(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

/// Reads and drops the rest of a refused upload, through the pacer, for at
/// most `drain_bytes` or `drain_time`. Dropping the body then closes the
/// connection, as its answer said.
pub(in crate::native_api) async fn drain(h: Shared, mut body: Body) {
    let limits = h.file_transfers.limits();
    let until = tokio::time::Instant::now() + limits.drain_time;
    let mut drained = 0u64;
    while drained < limits.drain_bytes {
        let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(until, next_chunk(&mut body)).await
        else {
            break;
        };
        drained += chunk.len() as u64;
        let wait = h.file_transfers.pace(&h, chunk.len() as u64);
        if tokio::time::Instant::now() + wait >= until {
            break;
        }
        tokio::time::sleep(wait).await;
    }
}

async fn next_chunk(body: &mut Body) -> Option<Result<Bytes, axum::Error>> {
    loop {
        match std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await? {
            Ok(frame) => {
                if let Ok(data) = frame.into_data()
                    && !data.is_empty()
                {
                    return Some(Ok(data));
                }
            }
            Err(error) => return Some(Err(error)),
        }
    }
}

fn content_length(headers: &HeaderMap) -> Result<u64, Failure> {
    let value = headers.get(header::CONTENT_LENGTH).ok_or_else(|| {
        Failure::new(
            ErrorCode::LengthRequired,
            "Send the file with a Content-Length.",
        )
    })?;
    value
        .to_str()
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| {
            Failure::new(
                ErrorCode::InvalidRequest,
                "The Content-Length is not a number.",
            )
        })
}

fn incomplete(message: impl Into<String>) -> Failure {
    Failure::new(ErrorCode::IncompleteUpload, message)
}

async fn receive(
    h: &Shared,
    c: &Connection,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &mut Body,
) -> Reply {
    let client = allowed(h, c, FILE_UPLOAD, "Upload files")?;
    let name = name_of(query)?;
    butterpollo_session::file_transfer::check_name(&name)
        .map_err(|error| failure("check the name", error.into()))?;
    let dir = transfer_dir(h);
    let recheck = Recheck::new(h, c, FILE_UPLOAD, "Upload files");
    let uploaded = accept(
        h,
        butterpollo_session::state::device_uuid(&client.uuid),
        headers,
        body,
        recheck,
        failure,
        move |slot, length| {
            let folder = open_folder(dir.as_deref())?;
            slot.hold_room(&folder, length)?;
            folder.create_part(&name)
        },
    )
    .await?;
    tracing::info!(
        "File transfer received {} ({} bytes) from {}",
        uploaded.name,
        uploaded.size,
        client.name
    );
    Ok(json(StatusCode::CREATED, &uploaded))
}

/// Takes an upload of its `Content-Length` within the file transfer limits,
/// through the pacer while `recheck` holds. `create` holds room on the
/// target's volume and starts the `.part` file, on a blocking thread; its
/// failures and the writer's are answered as `failure` has them.
pub(in crate::native_api) async fn accept(
    h: &Shared,
    device: &str,
    headers: &HeaderMap,
    body: &mut Body,
    mut recheck: Recheck,
    failure: fn(&str, UserFolderError) -> Failure,
    create: impl FnOnce(&UploadSlot, u64) -> Result<PartFile, UserFolderError> + Send + 'static,
) -> Result<Uploaded, Failure> {
    let length = content_length(headers)?;
    let max = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer_max_bytes();
    if length > max {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            format!(
                "The file is larger than the {} MiB this PC accepts.",
                max >> 20
            ),
        ));
    }
    let slot = begin_upload(h, device)?;
    let (ready_tx, ready) = oneshot::channel();
    let (tx, rx) = mpsc::channel(QUEUE);
    let writer =
        tokio::task::spawn_blocking(move || write_part(slot, length, create, ready_tx, rx));
    match ready.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(failure("start the upload", error)),
        Err(_) => {
            return Err(match writer.await {
                Err(error) => Failure::internal("start the upload", error),
                Ok(_) => Failure::internal("start the upload", "the writer stopped"),
            });
        }
    }

    let limits = h.file_transfers.limits();
    let started = Instant::now();
    // Time spent on the pacer and the disk, which the device isn't slow for.
    let mut held = Duration::ZERO;
    let mut last = started;
    let mut received = 0u64;
    let read: Result<(), Failure> = loop {
        if let Err(refusal) = recheck.check() {
            break Err(refusal);
        }
        let stalled = last + limits.idle;
        // When the average falls below `min_rate`, once `grace` is over.
        let slow = started
            + held
            + limits.grace.max(Duration::from_millis(
                received.saturating_mul(1000) / limits.min_rate,
            ));
        let until = stalled.min(slow).min(Instant::now() + PACE_CHECK);
        let chunk = match tokio::time::timeout_at(until.into(), next_chunk(body)).await {
            Err(_) => {
                let now = Instant::now();
                if now >= stalled {
                    break Err(incomplete(format!(
                        "The upload stalled after {received} of {length} bytes."
                    )));
                }
                if now >= slow {
                    break Err(incomplete(format!(
                        "The upload arrived slower than {} KiB/s; {received} of {length} bytes came.",
                        limits.min_rate / 1024
                    )));
                }
                continue;
            }
            Ok(None) if received == length => break Ok(()),
            Ok(None) => {
                break Err(incomplete(format!(
                    "The upload ended after {received} of {length} bytes."
                )));
            }
            Ok(Some(Err(error))) => {
                break Err(incomplete(format!(
                    "The upload broke off after {received} of {length} bytes: {error}."
                )));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if received > length {
            break Err(Failure::new(
                ErrorCode::InvalidRequest,
                "The body is longer than its Content-Length.",
            ));
        }
        let holding = Instant::now();
        let wait = h.file_transfers.pace(h, chunk.len() as u64);
        if let Err(refusal) = paced(wait, &mut recheck).await {
            break Err(refusal);
        }
        if tx.send(ToDisk::Chunk(chunk)).await.is_err() {
            // The writer failed; its error is the answer.
            break Ok(());
        }
        last = Instant::now();
        held += last - holding;
    };
    if read.is_ok() {
        // A writer gone already has its error.
        let _ = tx.send(ToDisk::Finish).await;
    }
    drop(tx);
    let written = writer
        .await
        .map_err(|error| Failure::internal("write the upload", error))?;
    let name = match (read, written) {
        (_, Err(error)) => return Err(failure("write the upload", error)),
        (Err(refusal), Ok(_)) => return Err(refusal),
        (Ok(()), Ok(Some(name))) => name,
        (Ok(()), Ok(None)) => {
            return Err(Failure::internal(
                "write the upload",
                "the writer stopped before the end",
            ));
        }
    };
    Ok(Uploaded {
        name,
        size: received,
    })
}

enum ToDisk {
    Chunk(Bytes),
    /// The whole body arrived: rename the `.part` file.
    Finish,
}

/// Holds room for the upload and creates its `.part` file (`create`), says
/// whether that worked on `ready`, then writes the queued chunks. The `.part`
/// file lives and dies on this blocking thread, as it is opened as the user:
/// [`ToDisk::Finish`] renames it and returns the name, while a queue closed
/// early (a refused or cancelled upload) or a failed write deletes it. The
/// device's slot and the room it held free only then.
fn write_part(
    slot: UploadSlot,
    length: u64,
    create: impl FnOnce(&UploadSlot, u64) -> Result<PartFile, UserFolderError>,
    ready: oneshot::Sender<Result<(), UserFolderError>>,
    mut rx: mpsc::Receiver<ToDisk>,
) -> Result<Option<String>, UserFolderError> {
    let mut part = match create(&slot, length) {
        Ok(part) => part,
        Err(error) => {
            #[cfg(any(test, feature = "test-support"))]
            let hold = slot.host.file_transfers.refusal_hold_duration();
            // Free the device's slot first: once the refusal is answered,
            // its next upload must not find the slot still taken.
            drop(slot);
            let _ = ready.send(Err(error));
            #[cfg(any(test, feature = "test-support"))]
            std::thread::sleep(hold);
            return Ok(None);
        }
    };
    if ready.send(Ok(())).is_err() {
        return discard(part);
    }
    loop {
        match rx.blocking_recv() {
            Some(ToDisk::Chunk(chunk)) => {
                part.write_all(&chunk)?;
                slot.wrote(chunk.len() as u64);
            }
            Some(ToDisk::Finish) => return part.finish().map(Some),
            None => return discard(part),
        }
    }
}

fn discard(part: PartFile) -> Result<Option<String>, UserFolderError> {
    if let Err(error) = part.discard() {
        tracing::warn!("File transfer could not delete an incomplete upload: {error}");
    }
    Ok(None)
}
