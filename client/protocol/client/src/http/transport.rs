//! Blocking HTTP/1.1 over TCP, or over TLS with the client certificate and a pinned server
//! certificate, as NvHTTP.java configures OkHttp.

use super::wire::{read_body, read_head, read_response, read_response_with_headers, Reader};
use super::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

static USER_AGENT: OnceLock<String> = OnceLock::new();

/// Names the app's version in every request's `User-Agent`, `Nightjar/<version>`; until it is
/// set (tools and tests) the header is plain `Nightjar`. The first call wins.
pub fn set_app_version(version: &str) {
    let _ = USER_AGENT.set(user_agent_for(version));
}

/// `Nightjar/<version>` with only printable ASCII kept, so a version can never end the header.
fn user_agent_for(version: &str) -> String {
    let version: String = version.chars().filter(|c| c.is_ascii_graphic()).collect();
    if version.is_empty() {
        "Nightjar".into()
    } else {
        format!("Nightjar/{version}")
    }
}

pub(super) fn user_agent() -> &'static str {
    USER_AGENT.get().map_or("Nightjar", String::as_str)
}

/// A host address: an IP literal (IPv6 may carry a numeric `%scope`) or a host name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Host {
    /// Without brackets or scope.
    pub name: String,
    pub scope_id: u32,
}

impl Host {
    pub fn parse(address: &str) -> Result<Self, Error> {
        let mut text = address.trim();
        if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            text = inner;
        }
        if text.is_empty() {
            return Err(Error::InvalidArgument("empty host address".into()));
        }
        let (name, scope) = match text.split_once('%') {
            Some((name, scope)) if name.contains(':') => (name, Some(scope)),
            _ => (text, None),
        };
        let scope_id = match scope {
            Some(scope) => scope
                .parse()
                .map_err(|_| Error::InvalidArgument(format!("unsupported IPv6 scope: {scope}")))?,
            None => 0,
        };
        // An IPv4-mapped IPv6 literal is used as plain IPv4.
        let name = match name.parse::<Ipv6Addr>() {
            Ok(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => v6.to_string(),
            },
            Err(_) => name.to_owned(),
        };
        Ok(Self { name, scope_id })
    }

    /// The host as it appears in a URL or Host header.
    pub fn url_host(&self) -> String {
        if self.name.contains(':') {
            format!("[{}]", self.name)
        } else {
            self.name.clone()
        }
    }

    fn socket_addrs(&self, port: u16) -> Result<Vec<SocketAddr>, Error> {
        if let Ok(ip) = self.name.parse::<IpAddr>() {
            return Ok(vec![match ip {
                IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, port, 0, self.scope_id)),
                v4 => SocketAddr::new(v4, port),
            }]);
        }
        Ok((self.name.as_str(), port).to_socket_addrs()?.collect())
    }
}

/// Percent-encodes a query component.
pub(crate) fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'*') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A random version 4 UUID, which NvHTTP adds to every request.
pub(crate) fn random_uuid() -> String {
    let mut b = super::crypto::random::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = super::der::hex_encode(&b).to_ascii_lowercase();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    pub reason: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    pub connect: Duration,
    /// None: no read timeout, for requests that wait on the user or a launching game.
    pub read: Option<Duration>,
}

/// Open sockets, so another thread can abort requests (NvHTTP.cancelPendingRequests).
#[derive(Default)]
pub(crate) struct Cancel {
    cancelled: AtomicBool,
    next: AtomicU64,
    sockets: Mutex<Vec<(u64, Arc<Abort>)>>,
}

/// Thread-safe handle for cancelling a [`super::HostClient`]'s pending or future requests.
#[derive(Clone)]
pub struct HostClientCanceller(pub(super) Arc<Cancel>);

impl HostClientCanceller {
    /// Aborts any in-flight socket I/O and rejects later requests on the linked client.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Whether [`Self::cancel`] or [`super::HostClient::cancel_pending_requests`] was triggered.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

impl Cancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let sockets: Vec<_> = self.sockets.lock().unwrap().drain(..).collect();
        for (_, socket) in sockets {
            socket.abort();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(super) fn register(&self, socket: &Arc<Abort>) -> Result<u64, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.sockets.lock().unwrap().push((id, socket.clone()));
        if self.is_cancelled() {
            socket.abort();
            self.unregister(id);
            return Err(Error::Cancelled);
        }
        Ok(id)
    }

    pub(super) fn unregister(&self, id: u64) {
        self.sockets.lock().unwrap().retain(|(i, _)| *i != id);
    }
}

/// A connection's socket as its cancels hold it. The abort is sticky: every read and write
/// through [`DeadlineSocket`] checks it before it starts, so none begins blocking after a cancel.
pub(crate) struct Abort {
    socket: TcpStream,
    aborted: AtomicBool,
    /// While a read or write may block on the socket.
    busy: AtomicBool,
}

/// The most [`Abort::abort`] waits for a call it aborted to come back.
#[cfg(windows)]
const ABORT_WAIT: Duration = Duration::from_secs(1);

impl Abort {
    pub(crate) fn new(socket: &TcpStream) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self { socket: socket.try_clone()?, aborted: AtomicBool::new(false), busy: AtomicBool::new(false) }))
    }

    /// Shuts the socket, so any later call fails at once, and aborts the call in progress. A
    /// call that passed its check just before the abort may reach the socket only after
    /// `CancelIoEx` ran; it is aborted again until it returns.
    pub(crate) fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        let _ = self.socket.shutdown(Shutdown::Both);
        #[cfg(windows)]
        {
            let until = std::time::Instant::now() + ABORT_WAIT;
            loop {
                abort_blocked_io(&self.socket);
                if !self.busy.load(Ordering::SeqCst) || std::time::Instant::now() >= until {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Runs `call` on the socket unless it was aborted; `abort` sees it as busy meanwhile.
    pub(crate) fn guard<T>(&self, call: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
        self.busy.store(true, Ordering::SeqCst);
        let result = if self.aborted.load(Ordering::SeqCst) {
            Err(std::io::Error::new(ErrorKind::ConnectionAborted, "the request was cancelled"))
        } else {
            call()
        };
        self.busy.store(false, Ordering::SeqCst);
        result
    }
}

/// Windows does not wake a read or write blocked on a socket that was shut down until the peer
/// answers, which a stalled host never does; this aborts it at once, through any handle of the
/// socket.
#[cfg(windows)]
pub(super) fn abort_blocked_io(socket: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    unsafe extern "system" {
        fn CancelIoEx(file: *mut std::ffi::c_void, overlapped: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: `socket` keeps the handle open; a null OVERLAPPED cancels all of its I/O.
    unsafe {
        CancelIoEx(socket.as_raw_socket() as *mut std::ffi::c_void, std::ptr::null_mut());
    }
}

pub(crate) struct Request<'a> {
    pub host: &'a Host,
    pub port: u16,
    /// Path and query, without the leading slash.
    pub target: &'a str,
    pub tls: Option<Arc<ClientConfig>>,
    pub timeouts: Timeouts,
    pub body_limit: usize,
}

pub(crate) fn get(request: &Request<'_>, cancel: &Cancel) -> Result<Response, Error> {
    send(request, cancel, None)
}

/// A POST body and its media type.
#[derive(Clone, Copy)]
pub(crate) struct Body<'a> {
    pub bytes: &'a [u8],
    pub content_type: &'static str,
}

impl<'a> Body<'a> {
    /// The clipboard and the other classic actions send plain text.
    pub fn text(bytes: &'a [u8]) -> Self {
        Self { bytes, content_type: "text/plain; charset=utf-8" }
    }

    /// The native API.
    pub fn json(bytes: &'a [u8]) -> Self {
        Self { bytes, content_type: "application/json" }
    }

    /// A POST without a body or a media type, `Content-Length: 0`: the sleep route refuses
    /// anything else.
    pub fn empty() -> Self {
        Self { bytes: &[], content_type: "" }
    }
}

pub(crate) fn send(request: &Request<'_>, cancel: &Cancel, body: Option<Body<'_>>) -> Result<Response, Error> {
    send_inner(request, cancel, body, None, &mut |_, _, _| Ok(()))
}

type HeaderHook<'a> = dyn FnMut(u16, &[(String, String)], SocketAddr) -> Result<(), Error> + 'a;

pub(crate) fn probe(request: &Request<'_>, cancel: &Cancel, timeout: Duration, headers: &mut HeaderHook<'_>) -> Result<Response, Error> {
    send_inner(request, cancel, None, Some(timeout), headers)
}

pub(crate) fn probe_stream(
    request: &Request<'_>,
    cancel: &Cancel,
    timeout: Duration,
    headers: &mut HeaderHook<'_>,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), Error>,
) -> Result<u64, Error> {
    let deadline = Deadline::At(std::time::Instant::now() + timeout);
    let head_str = request_head(request, None);
    let too_large = || Error::Malformed(format!("host response exceeds the {} byte limit", request.body_limit));
    connection(request, &[cancel], deadline, Some(write_timeout(request)), &head_str, None, |stream, peer| {
        stream.flush().map_err(io_tls_error)?;
        let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
        let head = read_head(&mut reader)?;
        headers(head.status, &head.headers, peer)?;
        let mut total = 0usize;
        if head.header("Transfer-Encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
            loop {
                let line = reader.line()?;
                let size_text = line.split(';').next().unwrap_or_default().trim();
                let mut left = size_text.bytes().all(|b| b.is_ascii_hexdigit())
                    .then(|| usize::from_str_radix(size_text, 16).ok()).flatten()
                    .ok_or_else(|| Error::Malformed("invalid chunk size".into()))?;
                if left == 0 { break; }
                if left > request.body_limit.saturating_sub(total) { return Err(too_large()); }
                while left > 0 {
                    if reader.buffer.is_empty() && !reader.fill()? {
                        return Err(Error::Malformed("connection closed before the HTTP body ended".into()));
                    }
                    let take = left.min(reader.buffer.len());
                    let chunk: Vec<u8> = reader.buffer.drain(..take).collect();
                    on_chunk(&chunk)?;
                    total += take;
                    left -= take;
                }
                if !reader.line()?.is_empty() { return Err(Error::Malformed("invalid chunk terminator".into())); }
            }
        } else if let Some(len) = head.content_length()? {
            let len = usize::try_from(len).ok().filter(|l| *l <= request.body_limit).ok_or_else(too_large)?;
            while total < len {
                if reader.buffer.is_empty() && !reader.fill()? {
                    return Err(Error::Malformed("connection closed before the HTTP body ended".into()));
                }
                let take = (len - total).min(reader.buffer.len());
                let chunk: Vec<u8> = reader.buffer.drain(..take).collect();
                on_chunk(&chunk)?;
                total += take;
            }
        } else {
            loop {
                if !reader.buffer.is_empty() {
                    let take = reader.buffer.len();
                    if take > request.body_limit.saturating_sub(total) { return Err(too_large()); }
                    let chunk = std::mem::take(&mut reader.buffer);
                    on_chunk(&chunk)?;
                    total += take;
                }
                if !reader.fill()? { break; }
            }
        }
        Ok(total as u64)
    })
}

fn send_inner(request: &Request<'_>, cancel: &Cancel, body: Option<Body<'_>>, timeout: Option<Duration>, headers: &mut HeaderHook<'_>) -> Result<Response, Error> {
    let deadline = timeout.map_or(Deadline::None, |timeout| Deadline::At(std::time::Instant::now() + timeout));
    let head = request_head(request, body);
    connection(request, &[cancel], deadline, Some(write_timeout(request)), &head, None, |stream, peer| {
        if let Some(body) = body { stream.write_all(body.bytes).map_err(io_tls_error)?; }
        stream.flush().map_err(io_tls_error)?;
        read_response_with_headers(stream, request.body_limit, &mut |status, fields: &[(String, String)]| headers(status, fields, peer))
    })
}

/// How long one write may block on the buffered requests.
pub(super) fn write_timeout(request: &Request<'_>) -> Duration {
    request.timeouts.connect.max(Duration::from_secs(5))
}

/// A body streamed from a reader: exactly `length` bytes, announced as the Content-Length.
pub(crate) struct Upload<'a> {
    pub reader: &'a mut dyn Read,
    pub length: u64,
    pub content_type: &'static str,
    /// `POST` for file transfer, `PUT` for a shared folder.
    pub method: &'static str,
}

/// Bytes moved so far and the total: once at 0, then at most every `PROGRESS_BYTES` and
/// `PROGRESS_INTERVAL`, whichever comes later, and once at the total.
pub(crate) type Progress<'a> = dyn FnMut(u64, u64) + 'a;

const PROGRESS_BYTES: u64 = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Spaces the progress calls, so a fast link does not call into the app for every chunk.
struct Throttle {
    total: u64,
    reported: u64,
    at: std::time::Instant,
}

impl Throttle {
    /// Reports 0 of `total`.
    fn start(progress: &mut Progress<'_>, total: u64) -> Self {
        progress(0, total);
        Self { total, reported: 0, at: std::time::Instant::now() }
    }

    fn update(&mut self, progress: &mut Progress<'_>, done: u64) {
        if self.due(done, std::time::Instant::now()) {
            progress(done, self.total);
        }
    }

    fn due(&mut self, done: u64, now: std::time::Instant) -> bool {
        let due = done > self.reported && (done == self.total
            || (done - self.reported >= PROGRESS_BYTES && now.saturating_duration_since(self.at) >= PROGRESS_INTERVAL));
        if due {
            self.reported = done;
            self.at = now;
        }
        due
    }
}

/// The most an upload reads from its file at a time.
const UPLOAD_CHUNK: usize = 64 * 1024;

/// POSTs `upload` in chunks without holding the whole body, then reads the answer like `send`.
/// `transfer` cancels only this upload, at once, even while a write blocks; the read timeout
/// also bounds every write, and the time without progress over all of them, so a host that
/// stops reading fails the upload once. A reader that ends before `length` fails loudly.
///
/// The body waits for the host's `100 Continue`: a host that refuses the upload by its head
/// (413, 403) answers and closes without reading it, and on Windows the reset of a body sent
/// anyway discards that answer before it is read. A host (or proxy) that says nothing for
/// [`CONTINUE_WAIT`] gets the body anyway, as curl sends it.
pub(crate) fn upload(request: &Request<'_>, cancel: &Cancel, upload: Upload<'_>, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let mut head = head_with(upload.method, request, Some((upload.content_type, upload.length)));
    head.insert_str(head.len() - 2, "Expect: 100-continue\r\n");
    let continue_by = ContinueBy::default();
    connection(request, &[cancel, transfer], Deadline::idle(request), request.timeouts.read, &head, Some(&continue_by), |stream, _| {
        let answer = await_continue(stream, request.body_limit, &continue_by);
        continue_by.set(None);
        match answer? {
            Some(refusal) => Ok(refusal),
            None => send_upload(stream, upload, request.body_limit, progress, transfer),
        }
    })
}

/// How long an upload waits for `100 Continue` before it sends the body anyway.
const CONTINUE_WAIT: Duration = Duration::from_secs(1);

/// When an upload stops waiting for `100 Continue`: armed once its head is out, it cuts the
/// socket's reads short until `upload` disarms it.
type ContinueBy = std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>>;

/// What a read waiting for `100 Continue` fails with once [`CONTINUE_WAIT`] is over.
#[derive(Debug)]
struct ContinueElapsed;

impl std::fmt::Display for ContinueElapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no 100 Continue")
    }
}

impl std::error::Error for ContinueElapsed {}

fn continue_elapsed() -> std::io::Error {
    std::io::Error::new(ErrorKind::TimedOut, ContinueElapsed)
}

fn is_continue_elapsed(error: &Error) -> bool {
    matches!(error, Error::Io(e) if e.get_ref().is_some_and(|inner| inner.is::<ContinueElapsed>()))
}

/// Reads the host's answer to an `Expect: 100-continue` head: `None` once it says `100 Continue`,
/// or says nothing at all by `continue_by`, else its refusal, read whole. It cannot have
/// accepted a body it did not get.
fn await_continue(stream: &mut dyn Stream, body_limit: usize, continue_by: &ContinueBy) -> Result<Option<Response>, Error> {
    let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
    loop {
        let head = match read_head(&mut reader) {
            Ok(head) => head,
            Err(error) if is_continue_elapsed(&error) => {
                continue_by.set(None);
                if reader.buffer.is_empty() {
                    return Ok(None);
                }
                // Part of an answer arrived: the rest has the read timeout.
                continue;
            }
            Err(error) => return Err(error),
        };
        match head.status {
            100 => break,
            102..=199 => continue,
            200..=299 => return Err(Error::Malformed(format!("the host answered {} before the upload was sent", head.status))),
            _ => {
                let body = read_body(&mut reader, &head, body_limit)?;
                return Ok(Some(head.into_response(body)));
            }
        }
    }
    if !reader.buffer.is_empty() {
        return Err(Error::Malformed("the host sent more than 100 Continue before the upload".into()));
    }
    Ok(None)
}

/// Why an upload stopped sending.
enum Unsent {
    /// The reader failed or ended early, or the transfer was cancelled.
    Local(Error),
    /// The connection failed.
    Write(std::io::Error),
}

fn send_upload(stream: &mut dyn Stream, upload: Upload<'_>, body_limit: usize, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    match write_upload(stream, upload, progress, transfer) {
        Ok(()) => read_response(stream, body_limit),
        Err(Unsent::Local(error)) => Err(error),
        // A host that stopped reading has no answer either: waiting for one would only double
        // the timeout.
        Err(Unsent::Write(error)) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Err(io_tls_error(error)),
        // The host may refuse (413, 403) before it reads the body and close: its answer says
        // more than the failed write. It never accepts a body it did not get whole.
        Err(Unsent::Write(error)) => match read_response(stream, body_limit) {
            Ok(response) if !(200..=299).contains(&response.status) => Ok(response),
            _ => Err(io_tls_error(error)),
        },
    }
}

fn write_upload<W: Write + ?Sized>(stream: &mut W, upload: Upload<'_>, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<(), Unsent> {
    let Upload { reader, length, .. } = upload;
    let mut chunk = vec![0; UPLOAD_CHUNK];
    let mut sent = 0;
    let mut throttle = Throttle::start(progress, length);
    while sent < length {
        if transfer.is_cancelled() {
            return Err(Unsent::Local(Error::Cancelled));
        }
        let want = usize::try_from(length - sent).map_or(UPLOAD_CHUNK, |left| left.min(UPLOAD_CHUNK));
        let read = match reader.read(&mut chunk[..want]) {
            Ok(0) => return Err(Unsent::Local(Error::InvalidArgument(format!(
                "the upload ended after {sent} of {length} bytes")))),
            Ok(read) => read,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(Unsent::Local(Error::Io(e))),
        };
        stream.write_all(&chunk[..read]).map_err(Unsent::Write)?;
        sent += read as u64;
        throttle.update(progress, sent);
    }
    stream.flush().map_err(Unsent::Write)
}

/// GETs a body into `sink` as it arrives. A 2xx must carry a Content-Length and deliver all of
/// it; its `Response` has an empty body. Any other answer is read whole, up to `body_limit`, for
/// its error. `transfer` cancels only this download, at once, even while a read blocks. The read
/// timeout bounds the time without progress.
pub(crate) fn download(request: &Request<'_>, cancel: &Cancel, sink: &mut dyn Write, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let head = request_head(request, None);
    connection(request, &[cancel, transfer], Deadline::idle(request), Some(write_timeout(request)), &head, None, |stream, _| {
        receive(stream, request.body_limit, sink, progress, transfer)
    })
}

pub(crate) fn download_optional(
    request: &Request<'_>,
    cancel: &Cancel,
    sink: &mut dyn Write,
    progress: &mut dyn FnMut(u64, Option<u64>),
    transfer: &Cancel,
) -> Result<Response, Error> {
    let head = request_head(request, None);
    connection(request, &[cancel, transfer], Deadline::idle(request), Some(write_timeout(request)), &head, None, |stream, _| {
        let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
        let head = read_head(&mut reader)?;
        if !(200..=299).contains(&head.status) {
            let body = read_body(&mut reader, &head, request.body_limit)?;
            return Ok(head.into_response(body));
        }
        if head.header("Transfer-Encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
            let mut done = 0u64;
            progress(0, None);
            loop {
                let line = reader.line()?;
                let size_text = line.split(';').next().unwrap_or_default().trim();
                let mut left = size_text.bytes().all(|b| b.is_ascii_hexdigit())
                    .then(|| usize::from_str_radix(size_text, 16).ok()).flatten()
                    .ok_or_else(|| Error::Malformed("invalid chunk size".into()))?;
                if left == 0 { break; }
                while left > 0 {
                    if transfer.is_cancelled() { return Err(Error::Cancelled); }
                    if reader.buffer.is_empty() && !reader.fill()? {
                        return Err(Error::Malformed("the download ended early".into()));
                    }
                    let take = left.min(reader.buffer.len());
                    sink.write_all(&reader.buffer[..take])?;
                    reader.buffer.drain(..take);
                    done += take as u64;
                    left -= take;
                    progress(done, None);
                }
                if !reader.line()?.is_empty() { return Err(Error::Malformed("invalid chunk terminator".into())); }
            }
            sink.flush()?;
            return Ok(head.into_response(Vec::new()));
        }
        let length = head.content_length()?.ok_or_else(|| Error::Malformed("a download without a Content-Length".into()))?;
        let mut done = 0u64;
        let mut throttle = Throttle::start(&mut |d, t| progress(d, Some(t)), length);
        while done < length {
            if transfer.is_cancelled() { return Err(Error::Cancelled); }
            if reader.buffer.is_empty() && !reader.fill()? {
                return Err(Error::Malformed(format!("the download ended after {done} of {length} bytes")));
            }
            let n = usize::try_from(length - done).map_or(reader.buffer.len(), |left| left.min(reader.buffer.len()));
            sink.write_all(&reader.buffer[..n])?;
            reader.buffer.drain(..n);
            done += n as u64;
            throttle.update(&mut |d, t| progress(d, Some(t)), done);
        }
        if !reader.buffer.is_empty() || reader.fill()? {
            return Err(Error::Malformed(format!("the download is longer than its {length} bytes")));
        }
        sink.flush()?;
        Ok(head.into_response(Vec::new()))
    })
}

fn receive(stream: &mut dyn Stream, body_limit: usize, sink: &mut dyn Write, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
    let head = read_head(&mut reader)?;
    if !(200..=299).contains(&head.status) {
        let body = read_body(&mut reader, &head, body_limit)?;
        return Ok(head.into_response(body));
    }
    if head.header("Transfer-Encoding").is_some() {
        return Err(Error::Malformed("a download must not be chunked".into()));
    }
    let length = head
        .content_length()?
        .ok_or_else(|| Error::Malformed("a download without a Content-Length".into()))?;
    let mut done = 0;
    let mut throttle = Throttle::start(progress, length);
    while done < length {
        if transfer.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if reader.buffer.is_empty() && !reader.fill()? {
            return Err(Error::Malformed(format!("the download ended after {done} of {length} bytes")));
        }
        let n = usize::try_from(length - done).map_or(reader.buffer.len(), |left| left.min(reader.buffer.len()));
        sink.write_all(&reader.buffer[..n])?;
        reader.buffer.drain(..n);
        done += n as u64;
        throttle.update(progress, done);
    }
    // Bytes past the Content-Length mean the host and this client disagree on the file.
    if !reader.buffer.is_empty() || reader.fill()? {
        return Err(Error::Malformed(format!("the download is longer than its {length} bytes")));
    }
    sink.flush()?;
    Ok(head.into_response(Vec::new()))
}

/// A connected stream, plain or TLS.
trait Stream: Read + Write {}

impl<T: Read + Write> Stream for T {}

/// How long a request may take, beyond the timeout of each call.
#[derive(Clone, Copy)]
enum Deadline {
    None,
    /// The whole request, connecting included, ends by then.
    At(std::time::Instant),
    /// A transfer fails once nothing moved for this long, whichever calls waited.
    Idle(Duration),
}

impl Deadline {
    /// The read timeout, as the most a transfer may stall.
    fn idle(request: &Request<'_>) -> Self {
        request.timeouts.read.map_or(Deadline::None, Deadline::Idle)
    }
}

/// Connects, sends `head` and hands the stream to `exchange`, which writes any body and reads
/// the answer; any of `cancels` can abort it from another thread. Whatever fails once one is
/// set (a reset, or a body cut short by the shutdown) is `Error::Cancelled`.
fn connection<T>(request: &Request<'_>, cancels: &[&Cancel], deadline: Deadline, write_timeout: Option<Duration>,
    head: &str, continue_by: Option<&ContinueBy>, exchange: impl FnOnce(&mut dyn Stream, SocketAddr) -> Result<T, Error>) -> Result<T, Error> {
    let cancelled = || cancels.iter().any(|cancel| cancel.is_cancelled());
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let connect_by = match deadline { Deadline::At(at) => Some(at), _ => None };
    let result = connect(request.host, request.port, request.timeouts.connect, connect_by).and_then(|tcp| {
        let abort = Abort::new(&tcp)?;
        let mut ids = Vec::with_capacity(cancels.len());
        let result = cancels
            .iter()
            .try_for_each(|cancel| cancel.register(&abort).map(|id| ids.push(id)))
            .and_then(|()| open(request, tcp, abort, deadline, write_timeout, head, continue_by))
            .and_then(|(mut stream, peer)| exchange(&mut *stream, peer));
        for (cancel, id) in cancels.iter().zip(ids) {
            cancel.unregister(id);
        }
        result
    });
    match result {
        Err(_) if cancelled() => Err(Error::Cancelled),
        other => other,
    }
}

pub(crate) fn connect(host: &Host, port: u16, timeout: Duration, deadline: Option<std::time::Instant>) -> Result<TcpStream, Error> {
    connect_each(&host.socket_addrs(port)?, timeout, deadline, TcpStream::connect_timeout)
}

/// Tries the addresses in turn, each for `timeout` but never past `deadline`; a deadline that
/// runs out before an address answers fails with `TimedOut`.
fn connect_each<S>(addresses: &[SocketAddr], timeout: Duration, deadline: Option<std::time::Instant>,
    mut attempt: impl FnMut(&SocketAddr, Duration) -> std::io::Result<S>) -> Result<S, Error> {
    let mut last = None;
    for address in addresses {
        let budget = match deadline {
            None => timeout,
            Some(deadline) => match deadline.checked_duration_since(std::time::Instant::now()).filter(|left| !left.is_zero()) {
                Some(left) => timeout.min(left),
                None => return Err(Error::Io(ErrorKind::TimedOut.into())),
            },
        };
        match attempt(address, budget) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(
        last.unwrap_or_else(|| ErrorKind::NotFound.into()),
    ))
}

/// Sets the timeouts, completes any TLS handshake, writes `head` and arms `continue_by`.
fn open(request: &Request<'_>, tcp: TcpStream, abort: Arc<Abort>, deadline: Deadline, write_timeout: Option<Duration>,
    head: &str, continue_by: Option<&ContinueBy>) -> Result<(Box<dyn Stream>, SocketAddr), Error> {
    let peer = tcp.peer_addr()?;
    tcp.set_read_timeout(request.timeouts.read)?;
    tcp.set_write_timeout(write_timeout)?;
    tcp.set_nodelay(true)?;
    let (deadline, idle) = match deadline {
        Deadline::None => (None, None),
        Deadline::At(at) => (Some(at), None),
        Deadline::Idle(idle) => (Some(std::time::Instant::now() + idle), Some(idle)),
    };
    let tcp = DeadlineSocket { socket: tcp, abort, deadline, idle, read_timeout: request.timeouts.read, write_timeout,
        continue_by: continue_by.cloned(), capped: false };
    let mut stream: Box<dyn Stream> = match &request.tls {
        None => Box::new(tcp),
        Some(config) => {
            let name = ServerName::try_from(request.host.name.clone()).map_err(|_| {
                Error::InvalidArgument(format!("invalid host name: {}", request.host.name))
            })?;
            let connection = ClientConnection::new(config.clone(), name).map_err(tls_error)?;
            let mut stream = StreamOwned::new(connection, tcp);
            while stream.conn.is_handshaking() {
                stream
                    .conn
                    .complete_io(&mut stream.sock)
                    .map_err(io_tls_error)?;
            }
            Box::new(stream)
        }
    };
    stream.write_all(head.as_bytes()).map_err(io_tls_error)?;
    if let Some(continue_by) = continue_by {
        continue_by.set(Some(std::time::Instant::now() + CONTINUE_WAIT));
    }
    Ok((stream, peer))
}

fn request_head(request: &Request<'_>, body: Option<Body<'_>>) -> String {
    head(request, body.map(|body| (body.content_type, body.bytes.len() as u64)))
}

/// A GET, or a POST of `(media type, length)`.
fn head(request: &Request<'_>, body: Option<(&str, u64)>) -> String {
    head_with(if body.is_some() { "POST" } else { "GET" }, request, body)
}

fn head_with(method: &str, request: &Request<'_>, body: Option<(&str, u64)>) -> String {
    let mut head = format!(
        "{method} /{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nUser-Agent: {}\r\n",
        request.target,
        request.host.url_host(),
        request.port,
        user_agent()
    );
    if let Some((content_type, length)) = body {
        if !content_type.is_empty() {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("\r\n");
    head
}

struct DeadlineSocket {
    socket: TcpStream,
    /// Every read and write goes through it, so a cancel ends them all.
    abort: Arc<Abort>,
    deadline: Option<std::time::Instant>,
    /// Moves `deadline` this far ahead after every byte.
    idle: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    /// While armed, reads end by then with [`ContinueElapsed`].
    continue_by: Option<ContinueBy>,
    /// Whether the last read had its timeout cut short by `continue_by`.
    capped: bool,
}

impl DeadlineSocket {
    fn remaining(&self) -> std::io::Result<Option<Duration>> {
        self.deadline.map(|deadline| deadline.checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero()).ok_or_else(|| ErrorKind::TimedOut.into())).transpose()
    }

    /// Moves the idle deadline once a call moved at least `whole` bytes: any read, but only a
    /// write the socket took whole. A blocking write comes back short only when its timeout ran
    /// out with part of it sent, as Linux reports it (Windows fails it). The space freed during
    /// that wait then takes the next writes at once: counting the short write would let them
    /// restart the timeout, again and again, for a host that reads nothing.
    fn moved(&mut self, result: std::io::Result<usize>, whole: usize) -> std::io::Result<usize> {
        if let (Ok(moved), Some(idle)) = (&result, self.idle)
            && *moved > 0
            && *moved >= whole
        {
            self.deadline = Some(std::time::Instant::now() + idle);
        }
        result
    }
}

impl Read for DeadlineSocket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let left = self.remaining()?;
        let wait = self.continue_by.as_ref().and_then(|by| by.get())
            .map(|by| by.saturating_duration_since(std::time::Instant::now()));
        if wait.is_some_and(|wait| wait.is_zero()) {
            return Err(continue_elapsed());
        }
        let limit = [self.read_timeout, left].into_iter().flatten().min();
        let capped = wait.filter(|wait| limit.is_none_or(|limit| *wait < limit));
        if left.is_some() || capped.is_some() || self.capped {
            self.socket.set_read_timeout(capped.or(limit))?;
            self.capped = capped.is_some();
        }
        let read = self.abort.guard(|| self.socket.read(bytes));
        if capped.is_some() && matches!(&read, Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)) {
            return Err(continue_elapsed());
        }
        self.moved(read, 1)
    }
}

impl Write for DeadlineSocket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.socket.set_write_timeout(Some(self.write_timeout.map_or(left, |timeout| timeout.min(left))))?;
        }
        let written = self.abort.guard(|| self.socket.write(bytes));
        self.moved(written, bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { self.socket.flush() }
}

pub(crate) fn tls_error(error: rustls::Error) -> Error {
    match error {
        rustls::Error::InvalidCertificate(_) => Error::CertificateMismatch,
        other => Error::Tls(other.to_string()),
    }
}

pub(crate) fn io_tls_error(error: std::io::Error) -> Error {
    let inner = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .cloned();
    match inner {
        Some(e) => tls_error(e),
        None => Error::Io(error),
    }
}

pub use super::upgrade::Upgraded;
pub(crate) use super::upgrade::upgrade;

/// Accepts exactly the pinned certificate, whatever its name or issuer.
#[derive(Debug)]
struct PinnedCertificate {
    pinned: Vec<u8>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pinned.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// TLS that presents the client certificate and trusts only `server_certificate`.
pub(crate) fn tls_config(
    server_certificate: &[u8],
    client_certificate: &[u8],
    client_key_pkcs8: &[u8],
) -> Result<Arc<ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinnedCertificate {
        pinned: server_certificate.to_vec(),
        provider: provider.clone(),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(
            vec![CertificateDer::from(client_certificate.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_key_pkcs8.to_vec())),
        )
        .map_err(tls_error)?;
    // Sunshine and its forks (Vibepollo) identify the paired client in their certificate
    // callback, which a resumed session skips: they answer it with internal_error (#39).
    // Moonlight never resumes either, so every request does a full handshake.
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests;
