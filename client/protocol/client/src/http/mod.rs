//! Host HTTP: serverinfo, PIN pairing, the app list and box art, launch, resume and cancel.
//!
//! Ported from moonlight-android's NvHTTP, PairingManager, NvApp and Nightjar's
//! HostCapabilities (GPL-3.0, Cameron Gutman and contributors, extended for Nightjar by
//! RamazanKara). Plain HTTP serves serverinfo and pairing; everything else is HTTPS with the
//! client certificate, trusting only the host certificate pinned at pairing. The API is
//! blocking; call it from a worker thread.

mod clipboard_image;
mod coop;
pub(crate) mod crypto;
pub(crate) mod der;
mod device_approval;
mod files;
mod folders;
pub mod foundation;
mod hardware_metrics;
pub(crate) mod identity;
mod internet_play;
mod launch;
mod native;
mod network_path;
mod neural;
pub mod pace;
mod pairing;
mod pc_power;
mod power;
mod probe;
mod remote_check;
mod session_guests;
pub mod transfers;
pub(crate) mod transport;
mod upgrade;
mod usb;
pub(crate) mod wire;

#[cfg(test)]
mod fuzz_tests;

pub use clipboard_image::{
    ImageClipboardError, MAX_CLIPBOARD_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE,
    PNG_SIGNATURE, png_size,
};
pub use coop::{
    Coop, CoopEnd, CoopEndReason, CoopGame, CoopJoined, CoopPhase, CoopPlayer, CoopProblem,
    CoopSession,
};
pub use device_approval::{
    FEATURE_PAIRING_APPROVE, PairingApprove, PairingApproved, PairingDeny, PairingRequest,
    PairingRequests, valid_pairing_pin,
};
pub use files::{FileError, FileList, FileRefusal, RemoteFile, TransferCancel, Uploaded};
pub use folders::{
    EntryKind, FolderAccess, FolderEntries, FolderEntry, MAX_FOLDER_DEPTH, PART_SUFFIX,
    SharedFolder, child_path, collision_name, file_name, has_part_suffix, parent_path,
};
pub use hardware_metrics::{
    CPU_METRICS, GPU_METRICS, GpuMetrics, HardwareMetrics, MAX_HARDWARE_METRICS_BYTES, Readings,
};
pub use identity::ClientIdentity;
pub use internet_play::{InternetPlay, MAX_INTERNET_PLAY_BYTES};
pub use launch::{
    CONFIRM_REPLACE, CORE_LAUNCH_PARAMETERS, HostDisplay, LaunchRequest, Launched,
    asks_to_confirm_replace, asks_to_confirm_terminate, is_role_completed, is_terminate_completed,
    parse_launch_reply, random_ri_key,
};
pub use native::{
    AddedGame, FilePermissions, GameExtra, MovedTo, NativeCapabilities, NativeLibrary, PlayingOn,
    StreamMove,
};
pub use network_path::{MAX_NETWORK_PATH_BYTES, NetworkPath};
pub use neural::{
    FRAME_GENERATION, Neural, NeuralAction, NeuralJob, NeuralMode, NeuralPhase, NeuralSwitch,
    NeuralWhy, NeuralWrite,
};
pub use pairing::{PairFailure, PairResult, PairStep};
pub use pc_power::{
    FEATURE_RESTART, FEATURE_SHUTDOWN, FEATURE_UPDATE, HostUpdate, LateRefusal,
    POWER_TOKEN_REFRESH, PcPower, PcPowerOffer, PowerAvailability, host_words, update_arrived,
};
pub use power::{PowerError, PowerReply, Refusal};
pub use probe::{
    JUMBO_PROBE_FPS, JUMBO_PROBE_KBPS, LINK_PROBE_BYTES, PROBE_FLOOR_KBPS, PROBE_START_SHARE,
    jumbo_probe_passes, jumbo_probe_size, probe_start_kbps,
};
pub use remote_check::{ConnectionTestStats, REMOTE_CHECK_PROBES, RemoteCheck, RemoteOutcome};
pub use session_guests::{
    FEATURE_GUEST_INVITES, FEATURE_SESSION_GUESTS, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE,
    GUEST_PERMISSIONS, GUEST_VIEW, GUESTS_POLL_MS, GuestAction, GuestRequest, INVITED_JOINS_ONLY,
    InviteCreate, InviteCreated, SessionGuest, SessionGuests, guest_ended, join_refused,
};
pub use der::{
    certificate_sha256_der, certificate_sha256_pem, pem_decode_certificate, pem_encode_certificate,
};
pub use transport::{HostClientCanceller, set_app_version};
pub use usb::{
    TOKEN_HEADER as USB_TOKEN_HEADER, TUNNEL_PATH as USB_TUNNEL_PATH, UPGRADE as USB_UPGRADE,
    UsbTunnel,
};

/// Re-exported pure HTTP DTOs and parsers from `nightjar_policy::http`.
pub use nightjar_policy::http::{
    App, AppControl, AppRole, Availability, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT,
    DisplayLuminance, Error, Evidence, Family, Feature, HostApiError, HostCapabilities,
    LaunchUnanswered, PERMISSION_CLIPBOARD_READ, PERMISSION_CLIPBOARD_SET, PERMISSION_CONTROLLER,
    PERMISSION_INPUT, PERMISSION_LAUNCH, PERMISSION_LIST, PERMISSION_SERVER_COMMAND,
    PERMISSION_VIEW, PYROWAVE_CODEC_BITS, Reason, SECOND_SCREEN_PARAMETER, ServerInfo, app_control,
    parse_app_list, parse_rubylight_version, xml,
};
pub use rubylight_native_api as native_api;

pub(crate) mod capabilities {
    #[cfg(test)]
    pub use nightjar_policy::http::{Availability, Evidence, Reason, parse_rubylight_version};
    pub use nightjar_policy::http::{
        Family, Feature, HostCapabilities, PYROWAVE_BANDWIDTH_PROBE_BYTES,
    };
}
pub(crate) mod apps {
    pub use nightjar_policy::http::AppRole;
    #[cfg(test)]
    pub use nightjar_policy::http::parse_app_list;
}
pub(crate) mod luminance {
    pub use nightjar_policy::http::DisplayLuminance;
}
pub(crate) mod serverinfo {
    #[cfg(test)]
    pub use nightjar_policy::http::{ServerInfo, parse_version_quad};
    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) const RUBYLIGHT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion><hostname>Gaming PC</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>0123-ABCD</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>5</currentgame><currentgameuuid>uuid-5</currentgameuuid><state>SUNSHINE_SERVER_BUSY</state><LocalIP>192.168.1.20</LocalIP><mac>AA:BB:CC:DD:EE:FF</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>8457985</ServerCodecModeSupport><RustHostVersion>2.2.21</RustHostVersion><RustHostEncoderBitrateKbps>40000</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>50000</RustHostInitialEncoderBitrateKbps><RustHostSessionCount></RustHostSessionCount><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards><Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>0</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>119880</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><ServerCommand>Sleep</ServerCommand><ServerCommand></ServerCommand></root>";
    }
}

use std::sync::{Arc, Mutex};
use std::time::Duration;
use transport::{Cancel, Host, Request, Timeouts};

const SHORT_CONNECT_TIMEOUT: Duration = Duration::from_millis(3000);
const LONG_CONNECT_TIMEOUT: Duration = Duration::from_millis(5000);
const READ_TIMEOUT: Duration = Duration::from_millis(7000);
/// The longest a PC may take to answer a launch or resume: it prepares the session first (its
/// preparation commands, a display, the game), which can take a while, but a PC that never
/// answers must not hold the start for good. Android's `NvHTTP.LAUNCH_READ_TIMEOUT_SECONDS`.
pub const LAUNCH_READ_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_XML_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_APP_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOX_ART_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// The SHA-256 of a stored host certificate's DER: what a QR pairing link's `fp` names.
pub fn certificate_sha256(pem: &str) -> Result<[u8; 32], Error> {
    let mut hash = [0; 32];
    hash.copy_from_slice(&crypto::sha256(&der::pem_decode(pem, "CERTIFICATE")?));
    Ok(hash)
}

/// The `box.png` hosts send from `/appasset` for an app with no cover of its own, by length and SHA-256:
/// Rubylight's and Sunshine-family hosts' since Sunshine 0.21, and Sunshine 0.20 and older. Android's
/// `PlaceholderArt` knows the same two.
const PLACEHOLDER_BOX_ART: [(usize, &str); 2] = [
    (
        680,
        "d9164ebd069b5f735eb8efc557801778498da37f572ef70e3d35604739e6c613",
    ),
    (
        998,
        "f5992b68a72ef6f74c6e4970d0eb081fd7c51e8b5556c6a3f87dc414fda0473b",
    ),
];

/// True when box art is a host's grey placeholder rather than the app's own cover: hosts answer
/// `/appasset` with it instead of a 404, and the app's title stands in for it.
pub fn is_placeholder_box_art(bytes: &[u8]) -> bool {
    PLACEHOLDER_BOX_ART.iter().any(|(len, sha256)| {
        bytes.len() == *len
            && ring::digest::digest(&ring::digest::SHA256, bytes)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                == *sha256
    })
}

/// How a serverinfo answer was reached ([`HostClient::server_info_checked`]), which says what
/// its `PairStatus` is worth: hosts report 0 over plain HTTP whatever they hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoPath {
    /// Over HTTPS from the pinned certificate: `PairStatus` is the host's word.
    Pinned,
    /// The pinned certificate's host refused this client (401) and answered over HTTP: it no
    /// longer knows this client.
    Refused,
    /// Another certificate was presented at this address and the client fell back to HTTP
    /// (`server_info_with_cert_check`); `PairStatus` says nothing about our pairing.
    CertificateMismatch,
    /// Over plain HTTP with nothing pinned: `PairStatus` says nothing of a pairing.
    Plain,
}

/// A launch's failure after `waited`: a timeout that took the whole `limit` is the PC's missing
/// answer; a connect that timed out early stays the usual connection failure.
pub(super) fn launch_failure(error: Error, waited: Duration, limit: Duration) -> Error {
    match error {
        Error::Io(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) && waited >= limit =>
        {
            Error::Io(std::io::Error::other(LaunchUnanswered {
                seconds: limit.as_secs().max(1),
            }))
        }
        other => other,
    }
}

#[derive(Default)]
struct State {
    https_port: u16,
    server_certificate: Option<Vec<u8>>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

/// One host, reached at one address. Methods block; the client may be shared across threads.
pub struct HostClient {
    pub(super) host: Host,
    pub(super) http_port: u16,
    identity: Option<Arc<ClientIdentity>>,
    unique_id: String,
    device_name: String,
    second_screen: bool,
    state: Mutex<State>,
    pub(super) cancel: Arc<Cancel>,
}

#[derive(Clone, Copy)]
pub(super) enum Scheme {
    Http,
    Https,
}

impl HostClient {
    /// `address` is an IP literal (IPv6 may carry a numeric scope) or a host name.
    pub fn new(
        address: &str,
        http_port: u16,
        identity: Arc<ClientIdentity>,
    ) -> Result<Self, Error> {
        let unique_id = identity.unique_id().to_owned();
        Ok(Self {
            host: Host::parse(address)?,
            http_port,
            identity: Some(identity),
            unique_id,
            device_name: "Nightjar".into(),
            second_screen: false,
            state: Mutex::new(State::default()),
            cancel: Arc::new(Cancel::default()),
        })
    }

    /// An unauthenticated client (no client certificate/key) for HTTP-only calls or tests.
    pub fn new_unauthenticated(
        address: &str,
        http_port: u16,
        unique_id: impl Into<String>,
    ) -> Result<Self, Error> {
        Ok(Self {
            host: Host::parse(address)?,
            http_port,
            identity: None,
            unique_id: unique_id.into(),
            device_name: "Nightjar".into(),
            second_screen: false,
            state: Mutex::new(State::default()),
            cancel: Arc::new(Cancel::default()),
        })
    }

    /// The name the host shows for this client while pairing.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }

    /// Every request is the second screen's, with [`SECOND_SCREEN_PARAMETER`]: launch, resume,
    /// bitrate, the PC's bitrate controller, metrics and cancel alike.
    pub fn with_second_screen(mut self) -> Self {
        self.second_screen = true;
        self
    }

    /// Enables or disables the second-screen flag on an existing client instance.
    pub fn set_second_screen(&mut self, second_screen: bool) {
        self.second_screen = second_screen;
    }

    /// The HTTPS port from an earlier serverinfo, to skip asking for it.
    pub fn with_https_port(self, port: u16) -> Self {
        self.set_https_port(port);
        self
    }

    /// Updates the cached HTTPS port in-place (0 forces rediscovery via HTTP `serverinfo`).
    pub fn set_https_port(&self, port: u16) {
        self.state.lock().unwrap().https_port = port;
    }

    /// Currently cached HTTPS port (0 if unknown).
    pub fn https_port_cached(&self) -> u16 {
        self.state.lock().unwrap().https_port
    }

    /// The host certificate stored at pairing (PEM).
    pub fn with_server_certificate_pem(self, pem: &str) -> Result<Self, Error> {
        let der = der::pem_decode(pem, "CERTIFICATE")?;
        self.pin(Some(der));
        Ok(self)
    }

    /// Pins (or clears, with `None`) the host certificate DER bytes in-place.
    pub fn pin_server_certificate_der(&self, der: Option<Vec<u8>>) {
        self.pin(der);
    }

    /// The pinned host certificate DER bytes, if any.
    pub fn server_certificate_der(&self) -> Option<Vec<u8>> {
        self.state.lock().unwrap().server_certificate.clone()
    }

    /// The pinned host certificate as PEM, if any.
    pub fn server_certificate_pem(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .server_certificate
            .as_deref()
            .map(|d| der::pem_encode(d, "CERTIFICATE"))
    }

    pub fn identity(&self) -> Option<&ClientIdentity> {
        self.identity.as_deref()
    }

    /// Aborts requests in flight, such as a pairing waiting for its PIN, and every later one.
    pub fn cancel_pending_requests(&self) {
        self.cancel.cancel();
    }

    /// Whether [`HostClient::cancel_pending_requests`] has been called on this client.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Returns a cloneable cancellation handle that can abort requests from another thread.
    pub fn canceller(&self) -> HostClientCanceller {
        HostClientCanceller(Arc::clone(&self.cancel))
    }

    /// The same host and pairing, untouched by this client's [`HostClient::cancel_pending_requests`]:
    /// for a request that must still go after the stream ends, such as turning a PC's own bitrate
    /// controller off again.
    pub fn detached(&self) -> Self {
        let state = self.state.lock().unwrap();
        Self {
            host: self.host.clone(),
            http_port: self.http_port,
            identity: self.identity.clone(),
            unique_id: self.unique_id.clone(),
            device_name: self.device_name.clone(),
            second_screen: self.second_screen,
            state: Mutex::new(State {
                https_port: state.https_port,
                server_certificate: state.server_certificate.clone(),
                tls: state.tls.clone(),
            }),
            cancel: Arc::new(Cancel::default()),
        }
    }

    fn pin(&self, der: Option<Vec<u8>>) {
        let mut state = self.state.lock().unwrap();
        state.server_certificate = der;
        state.tls = None;
    }

    fn url_target(&self, path: &str, query: Option<&str>) -> String {
        let mut target = format!("{path}?");
        if let Some(query) = query {
            target.push_str(query);
            target.push('&');
        }
        target.push_str(&format!(
            "uniqueid={}&uuid={}",
            transport::encode_query(&self.unique_id),
            transport::random_uuid()
        ));
        if self.second_screen {
            target.push_str(&format!("&{SECOND_SCREEN_PARAMETER}=1"));
        }
        if path == "pair" {
            target.push_str(&format!(
                "&devicename={}",
                transport::encode_query(&self.device_name)
            ));
        }
        target
    }

    fn tls(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        let mut state = self.state.lock().unwrap();
        if let Some(tls) = &state.tls {
            return Ok(tls.clone());
        }
        let server = state
            .server_certificate
            .as_deref()
            .ok_or(Error::NotPaired)?;
        let identity = self
            .identity
            .as_deref()
            .ok_or_else(|| Error::InvalidIdentity("missing client certificate".into()))?;
        let tls = transport::tls_config(
            server,
            identity.certificate_der(),
            identity.private_key_pkcs8(),
        )?;
        state.tls = Some(tls.clone());
        Ok(tls)
    }

    /// The HTTPS port, asking serverinfo over HTTP the first time.
    fn https_port(&self, likely_online: bool) -> Result<u16, Error> {
        let known = self.state.lock().unwrap().https_port;
        if known != 0 {
            return Ok(known);
        }
        let connect = if likely_online {
            LONG_CONNECT_TIMEOUT
        } else {
            SHORT_CONNECT_TIMEOUT
        };
        let raw = self.request_text(
            Scheme::Http,
            "serverinfo",
            None,
            connect,
            Some(READ_TIMEOUT),
        )?;
        let port = ServerInfo::parse(&raw, self.http_port)?.https_port;
        self.state.lock().unwrap().https_port = port;
        Ok(port)
    }

    fn request(
        &self,
        scheme: Scheme,
        path: &str,
        query: Option<&str>,
        connect: Duration,
        read: Option<Duration>,
        body_limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let (port, tls) = match scheme {
            Scheme::Http => (self.http_port, None),
            Scheme::Https => (self.https_port(true)?, Some(self.tls()?)),
        };
        let target = self.url_target(path, query);
        let response = transport::get(
            &Request {
                host: &self.host,
                port,
                target: &target,
                tls,
                timeouts: Timeouts { connect, read },
                body_limit,
            },
            &self.cancel,
        )?;
        match response.status {
            200..=299 => Ok(response.body),
            404 => Err(Error::NotFound(format!("/{path}"))),
            code => Err(Error::Status {
                code: code as i32,
                message: response.reason,
            }),
        }
    }

    pub(super) fn request_text(
        &self,
        scheme: Scheme,
        path: &str,
        query: Option<&str>,
        connect: Duration,
        read: Option<Duration>,
    ) -> Result<String, Error> {
        let limit = if path == "applist" {
            MAX_APP_LIST_BYTES
        } else {
            MAX_XML_RESPONSE_BYTES
        };
        let body = self.request(scheme, path, query, connect, read, limit)?;
        String::from_utf8(body).map_err(|_| Error::Malformed("host reply is not UTF-8".into()))
    }

    /// Serverinfo: over pinned HTTPS when paired, falling back to HTTP when the host does not
    /// know this client (401) or shows another certificate, so the client can pair again.
    pub fn server_info(&self, likely_online: bool) -> Result<ServerInfo, Error> {
        self.server_info_with_cert_check(likely_online)
            .map(|(info, _)| info)
    }

    /// Serverinfo like [`HostClient::server_info`], also reporting [`InfoPath::CertificateMismatch`]
    /// when it fell back to HTTP after another certificate answered at the pinned HTTPS port.
    pub fn server_info_with_cert_check(
        &self,
        likely_online: bool,
    ) -> Result<(ServerInfo, InfoPath), Error> {
        match self.server_info_checked(likely_online) {
            Ok((info, path)) => Ok((info, path)),
            Err(Error::CertificateMismatch) => self
                .server_info_http(likely_online)
                .map(|info| (info, InfoPath::CertificateMismatch)),
            Err(e) => Err(e),
        }
    }

    /// Serverinfo and how it was reached ([`InfoPath`]), which says what its `PairStatus` is
    /// worth. Over pinned HTTPS when paired, over HTTP after the pinned host refused this client
    /// (401) or when nothing is pinned. Another certificate at this address is
    /// [`Error::CertificateMismatch`], with no HTTP fallback: that answer would be another
    /// host's, not this one's.
    pub fn server_info_checked(
        &self,
        likely_online: bool,
    ) -> Result<(ServerInfo, InfoPath), Error> {
        let connect = if likely_online {
            LONG_CONNECT_TIMEOUT
        } else {
            SHORT_CONNECT_TIMEOUT
        };
        let paired = self.state.lock().unwrap().server_certificate.is_some();
        if paired {
            let https = self
                .request_text(
                    Scheme::Https,
                    "serverinfo",
                    None,
                    connect,
                    Some(READ_TIMEOUT),
                )
                .and_then(|raw| ServerInfo::parse(&raw, self.http_port));
            match https {
                Ok(info) => return Ok((info, InfoPath::Pinned)),
                Err(Error::Status { code: 401, .. }) => {
                    return self
                        .server_info_http(likely_online)
                        .map(|info| (info, InfoPath::Refused));
                }
                Err(e) => return Err(e),
            }
        }
        self.server_info_http(likely_online)
            .map(|info| (info, InfoPath::Plain))
    }

    /// Serverinfo over plain HTTP; hosts report `PairStatus` 0 there.
    pub fn server_info_http(&self, likely_online: bool) -> Result<ServerInfo, Error> {
        let connect = if likely_online {
            LONG_CONNECT_TIMEOUT
        } else {
            SHORT_CONNECT_TIMEOUT
        };
        let raw = self.request_text(
            Scheme::Http,
            "serverinfo",
            None,
            connect,
            Some(READ_TIMEOUT),
        )?;
        let info = ServerInfo::parse(&raw, self.http_port)?;
        let mut state = self.state.lock().unwrap();
        if state.https_port == 0 {
            state.https_port = info.https_port;
        }
        Ok(info)
    }

    /// Serverinfo over pinned HTTPS only, without fallback (verifies the paired host).
    pub fn server_info_https(&self) -> Result<ServerInfo, Error> {
        let raw = self.request_text(
            Scheme::Https,
            "serverinfo",
            None,
            SHORT_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        ServerInfo::parse(&raw, self.http_port)
    }

    /// Pairs with the PIN the user types on the host. Blocks until the host answers, up to
    /// minutes; [`HostClient::cancel_pending_requests`] ends the wait. On success the host
    /// certificate is pinned; store it with the identity.
    pub fn pair(&self, server_info: &ServerInfo, pin: &str) -> Result<PairResult, Error> {
        self.pair_with(server_info, pin, None)
    }

    /// One-time PIN pairing (Apollo, Vibepollo, Rubylight): the host already knows the PIN
    /// and passphrase, so no one types anything.
    pub fn pair_with_passphrase(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: &str,
    ) -> Result<PairResult, Error> {
        self.pair_with(server_info, pin, Some(passphrase))
    }

    /// Pairs with a host's QR pairing link (Experimental): one-time PIN pairing, refused with
    /// [`Error::CertificateMismatch`] before anything is sent back when the host's certificate
    /// does not hash to the link's fingerprint. The client must already point at the link's
    /// host and port.
    pub fn pair_with_link(
        &self,
        server_info: &ServerInfo,
        link: &rubylight_protocol::pairing_uri::PairingUri,
    ) -> Result<PairResult, Error> {
        self.pair_checked(
            server_info,
            &link.otp,
            Some(&link.passphrase),
            Some(link.cert_sha256),
        )
    }

    fn pair_with(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: Option<&str>,
    ) -> Result<PairResult, Error> {
        self.pair_checked(server_info, pin, passphrase, None)
    }

    pub fn pair_checked(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: Option<&str>,
        fingerprint: Option<[u8; 32]>,
    ) -> Result<PairResult, Error> {
        let major = server_info.major_version()?;
        let previous = {
            let mut state = self.state.lock().unwrap();
            if state.https_port == 0 {
                state.https_port = server_info.https_port;
            }
            state.server_certificate.clone()
        };
        let identity = self
            .identity
            .as_deref()
            .ok_or_else(|| Error::InvalidIdentity("missing client certificate".into()))?;
        // A failed attempt keeps the previous pin (the Java client left the new one set).
        let mut transport = PairingClient {
            client: self,
            fingerprint,
        };
        let result = pairing::pair(&mut transport, identity, major, pin, passphrase);
        if !matches!(result, Ok(PairResult::Paired { .. })) {
            self.pin(previous);
        }
        result
    }

    /// Unpairs this client; over HTTPS when paired, else HTTP.
    pub fn unpair(&self) -> Result<(), Error> {
        let scheme = if self.state.lock().unwrap().server_certificate.is_some() {
            Scheme::Https
        } else {
            Scheme::Http
        };
        self.request_text(
            scheme,
            "unpair",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        Ok(())
    }

    /// The app list; `capabilities` (from serverinfo) decides which entries are control tiles.
    pub fn app_list(&self, capabilities: &HostCapabilities) -> Result<Vec<App>, Error> {
        let raw = self.app_list_raw()?;
        parse_app_list(&raw, capabilities)
    }

    /// The app list reply as received, for callers that cache it.
    pub fn app_list_raw(&self) -> Result<String, Error> {
        self.request_text(
            Scheme::Https,
            "applist",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )
    }

    fn box_art_query(app_id: i32) -> String {
        format!("appid={app_id}&AssetType=2&AssetIdx=0")
    }

    /// The box art image (PNG or JPEG bytes as the host sends them).
    pub fn box_art(&self, app_id: i32) -> Result<Vec<u8>, Error> {
        self.request(
            Scheme::Https,
            "appasset",
            Some(&Self::box_art_query(app_id)),
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
            MAX_BOX_ART_BYTES,
        )
    }

    /// The box art URL; fetching it needs this client's certificate and the pinned host's.
    pub fn box_art_url(&self, app_id: i32) -> Result<String, Error> {
        let port = self.https_port(true)?;
        Ok(format!(
            "https://{}:{port}/{}",
            self.host.url_host(),
            self.url_target("appasset", Some(&Self::box_art_query(app_id)))
        ))
    }

    /// Quits the running app. Ok(false) when the host declined; a 599 status when the host
    /// still runs a game afterwards (it belongs to another client).
    pub fn cancel(&self) -> Result<bool, Error> {
        let raw = self.request_text(Scheme::Https, "cancel", None, LONG_CONNECT_TIMEOUT, None)?;
        if xml::parse_reply(&raw)?.required_text("cancel")? == "0" {
            return Ok(false);
        }
        if self.server_info(true)?.current_game != 0 {
            return Err(Error::Status {
                code: 599,
                message: String::new(),
            });
        }
        Ok(true)
    }

    fn clipboard_request(&self, text: Option<&str>) -> Result<transport::Response, Error> {
        if text.is_some_and(|text| text.len() > MAX_CLIPBOARD_BYTES) {
            return Err(Error::InvalidArgument(
                "Clipboard text exceeds 1 MiB".into(),
            ));
        }
        let response = transport::send(
            &Request {
                host: &self.host,
                port: self.https_port(true)?,
                target: &self.url_target("actions/clipboard", Some("type=text")),
                tls: Some(self.tls()?),
                timeouts: Timeouts {
                    connect: LONG_CONNECT_TIMEOUT,
                    read: Some(READ_TIMEOUT),
                },
                body_limit: MAX_CLIPBOARD_BYTES,
            },
            &self.cancel,
            text.map(|text| transport::Body::text(text.as_bytes())),
        )?;
        if !(200..=299).contains(&response.status) {
            return Err(Error::Status {
                code: i32::from(response.status),
                message: response.reason,
            });
        }
        Ok(response)
    }

    pub fn clipboard(&self) -> Result<String, Error> {
        let response = self.clipboard_request(None)?;
        if !response.content_type.as_deref().is_some_and(|mime| {
            mime.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("text/plain")
        }) {
            return Err(Error::Malformed(
                "The PC does not support text clipboard transfer".into(),
            ));
        }
        String::from_utf8(response.body)
            .map_err(|_| Error::Malformed("Clipboard text is not UTF-8".into()))
    }

    pub fn send_clipboard(&self, text: &str) -> Result<(), Error> {
        if !self.clipboard_request(Some(text))?.body.is_empty() {
            return Err(Error::Malformed(
                "The PC did not accept the clipboard text".into(),
            ));
        }
        Ok(())
    }

    pub fn set_bitrate(&self, kbps: u32) -> Result<u32, Error> {
        if !(1..=2_000_000).contains(&kbps) {
            return Err(Error::InvalidArgument(
                "Bitrate must be between 1 and 2000000 kbps".into(),
            ));
        }
        let raw = self.request_text(
            Scheme::Https,
            "bitrate",
            Some(&format!("bitrate={kbps}")),
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        xml::parse_reply(&raw)?
            .required_text("bitrate")?
            .parse::<u32>()
            .ok()
            .filter(|applied| (1..=2_000_000).contains(applied))
            .ok_or_else(|| Error::Malformed("The PC did not apply the bitrate".into()))
    }
}

struct PairingClient<'a> {
    client: &'a HostClient,
    /// The SHA-256 the host certificate must have (a QR pairing link's `fp`).
    fingerprint: Option<[u8; 32]>,
}

impl pairing::PairingTransport for PairingClient<'_> {
    fn pair_command(&mut self, query: &str, read_timeout: bool) -> Result<String, Error> {
        self.client.request_text(
            Scheme::Http,
            "pair",
            Some(&format!("updateState=1&{query}")),
            LONG_CONNECT_TIMEOUT,
            read_timeout.then_some(READ_TIMEOUT),
        )
    }

    fn pair_challenge(&mut self) -> Result<String, Error> {
        self.client.request_text(
            Scheme::Https,
            "pair",
            Some("updateState=1&phrase=pairchallenge"),
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )
    }

    fn pin_server_certificate(&mut self, der: &[u8]) -> Result<(), Error> {
        if let Some(expected) = self.fingerprint
            && crypto::sha256(der) != expected
        {
            return Err(Error::CertificateMismatch);
        }
        self.client.pin(Some(der.to_vec()));
        Ok(())
    }

    fn unpair(&mut self) -> Result<(), Error> {
        self.client.unpair()
    }

    fn cancel_pairing(&mut self) -> Result<(), Error> {
        self.client.request_text(
            Scheme::Http,
            "unpair",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    // Nightjar #88: a launch waited for the PC's answer without any limit.
    #[test]
    fn a_launch_the_pc_never_answers_fails_with_its_own_words() {
        assert!(LAUNCH_READ_TIMEOUT > READ_TIMEOUT);
        let timed_out = || Error::Io(std::io::ErrorKind::TimedOut.into());
        let unanswered = launch_failure(timed_out(), LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT);
        assert!(unanswered.is_launch_unanswered());
        assert!(
            unanswered.to_string().contains("within 120 seconds"),
            "{unanswered}"
        );
        assert!(
            matches!(&unanswered, Error::Io(e) if e.kind() == std::io::ErrorKind::Other),
            "not a dropped connection a retry would cure"
        );
        let would_block = Error::Io(std::io::ErrorKind::WouldBlock.into());
        assert!(
            launch_failure(would_block, LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT)
                .is_launch_unanswered()
        );
        // A connect that timed out early stays the usual failure.
        let early = launch_failure(timed_out(), Duration::from_secs(5), LAUNCH_READ_TIMEOUT);
        assert!(!early.is_launch_unanswered());
        assert!(matches!(&early, Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut));
        // Anything else is passed on.
        assert!(matches!(
            launch_failure(Error::Cancelled, LAUNCH_READ_TIMEOUT, LAUNCH_READ_TIMEOUT),
            Error::Cancelled
        ));
        assert!(!Error::Cancelled.is_launch_unanswered());
    }

    #[test]
    fn a_detached_client_still_reaches_the_host_after_a_cancel() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request);
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let identity = Arc::new(identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", port, identity)
            .unwrap()
            .with_https_port(47984);
        client.cancel_pending_requests();
        assert!(matches!(
            client.server_info_http(true),
            Err(Error::Cancelled)
        ));
        let detached = client.detached();
        assert_eq!(detached.state.lock().unwrap().https_port, 47984);
        assert!(!matches!(
            detached.server_info_http(true),
            Err(Error::Cancelled)
        ));
        host.join().unwrap();
    }

    #[test]
    fn only_a_second_screens_client_marks_its_requests() {
        let identity = Arc::new(identity::tests::fixture());
        let first = HostClient::new("127.0.0.1", 47989, identity).unwrap();
        let target = first.url_target("launch", Some("appid=1"));
        assert!(target.starts_with("launch?appid=1&uniqueid="), "{target}");
        assert!(!target.contains(SECOND_SCREEN_PARAMETER), "{target}");
        let second = first.detached().with_second_screen();
        for (path, query) in [
            ("launch", Some("appid=1")),
            ("cancel", None),
            ("api/abr", None),
        ] {
            let target = second.url_target(path, query);
            assert!(target.ends_with("&rlSecondScreen=1"), "{target}");
            assert_eq!(
                target.matches(SECOND_SCREEN_PARAMETER).count(),
                1,
                "{target}"
            );
        }
        assert!(
            second
                .detached()
                .url_target("bitrate", None)
                .ends_with("&rlSecondScreen=1")
        );
        assert!(
            !first
                .url_target("bitrate", None)
                .contains(SECOND_SCREEN_PARAMETER)
        );
    }

    #[test]
    fn the_hosts_placeholder_box_art_is_no_cover() {
        let placeholder = include_bytes!("../../tests/fixtures/box-placeholder.png");
        assert!(is_placeholder_box_art(placeholder));
        let mut other = placeholder.to_vec();
        other[600] ^= 1;
        assert!(
            !is_placeholder_box_art(&other),
            "a cover of the same size is a cover"
        );
        assert!(!is_placeholder_box_art(&[]));
        // In the monorepo the fixture is the host's own placeholder.
        let host = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../rust/assets/package/box.png"
        );
        if let Ok(host) = std::fs::read(host) {
            assert_eq!(host, placeholder.to_vec());
        }
    }
}
