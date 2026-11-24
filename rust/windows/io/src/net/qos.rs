//! Dynamic qWAVE (`qwave.dll`) QoS flow tagging for video and audio UDP streams.
use anyhow::Result;
use std::{
    net::{SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
};
use windows::Win32::Networking::WinSock::{SOCKADDR, SOCKET};

/// qWAVE, loaded at run time as Vibepollo does: Windows Server has
/// qwave.dll only with its Quality Windows Audio Video Experience feature.
struct Qwave {
    handle: usize,
    add: QosAdd,
    remove: QosRemove,
}
type QosAdd = unsafe extern "system" fn(
    windows::Win32::Foundation::HANDLE,
    SOCKET,
    *const SOCKADDR,
    i32,
    u32,
    *mut u32,
) -> windows::core::BOOL;
type QosRemove = unsafe extern "system" fn(
    windows::Win32::Foundation::HANDLE,
    SOCKET,
    u32,
    u32,
) -> windows::core::BOOL;

fn qwave() -> Option<&'static Qwave> {
    use windows::Win32::{Foundation::HANDLE, System::LibraryLoader::*};
    #[repr(C)]
    struct Version {
        major: u16,
        minor: u16,
    }
    type Create = unsafe extern "system" fn(*const Version, *mut HANDLE) -> windows::core::BOOL;
    static QWAVE: std::sync::OnceLock<Option<Qwave>> = std::sync::OnceLock::new();
    // SAFETY: qwave.dll comes from System32 and is never freed, so its pointers, transmuted to
    // their documented signatures, stay valid; `Version` and `handle` outlive the call.
    QWAVE
        .get_or_init(|| unsafe {
            let module = LoadLibraryExW(
                windows::core::w!("qwave.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .ok()?;
            let create: Create =
                std::mem::transmute(GetProcAddress(module, windows::core::s!("QOSCreateHandle"))?);
            let add: QosAdd = std::mem::transmute(GetProcAddress(
                module,
                windows::core::s!("QOSAddSocketToFlow"),
            )?);
            let remove: QosRemove = std::mem::transmute(GetProcAddress(
                module,
                windows::core::s!("QOSRemoveSocketFromFlow"),
            )?);
            let mut handle = HANDLE::default();
            if !create(&Version { major: 1, minor: 0 }, &mut handle).as_bool() {
                tracing::warn!(error = %windows::core::Error::from_thread(), "QoS tagging is unavailable");
                return None;
            }
            Some(Qwave {
                handle: handle.0 as usize,
                add,
                remove,
            })
        })
        .as_ref()
}

/// Load `qwave.dll` and create its handle ahead of the first media burst so
/// [`QosFlow::new`] does not stall the video or audio send path at startup.
pub fn warm_up_qos() -> bool {
    qwave().is_some()
}

/// Tags a socket's datagrams to one client for priority, as Vibepollo does
/// when Moonlight asks (it does on a local network): video as audio-video
/// traffic (DSCP 40, 802.1p 5), audio as voice (DSCP 56, 802.1p 7). Wi-Fi
/// sends those from WMM's video and voice queues, ahead of other traffic,
/// and switches that honour the marks do the same. The tag lasts until the
/// flow is dropped.
pub struct QosFlow(u32);
impl QosFlow {
    /// `None` where there is nothing to tag: no qWAVE, a client on this PC,
    /// or an IPv4 client of a dual-stack socket, which qWAVE cannot tag
    /// without connecting the socket that other sessions share.
    pub fn new(socket: &UdpSocket, peer: SocketAddr, voice: bool) -> Result<Option<Self>> {
        const AUDIO_VIDEO: i32 = 3;
        const VOICE: i32 = 4;
        const NON_ADAPTIVE_FLOW: u32 = 2;
        let mapped = matches!(peer.ip(), std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some());
        if peer.ip().to_canonical().is_loopback() || mapped {
            return Ok(None);
        }
        let Some(qwave) = qwave() else {
            return Ok(None);
        };
        let address = socket2::SockAddr::from(peer);
        let mut flow = 0;
        // SAFETY: `add` stays valid because qwave.dll is never freed, the handle is from
        // QOSCreateHandle, and `address` and `flow` outlive the call.
        let added = unsafe {
            (qwave.add)(
                windows::Win32::Foundation::HANDLE(qwave.handle as *mut _),
                SOCKET(socket.as_raw_socket() as usize),
                address.as_ptr().cast(),
                if voice { VOICE } else { AUDIO_VIDEO },
                NON_ADAPTIVE_FLOW,
                &mut flow,
            )
        };
        if !added.as_bool() {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(Some(Self(flow)))
    }
}
impl Drop for QosFlow {
    fn drop(&mut self) {
        if let Some(qwave) = qwave() {
            // SAFETY: The function and handle come from the never-freed Qwave, and `self.0` is the
            // flow QOSAddSocketToFlow returned; it is removed only here.
            unsafe {
                let _ = (qwave.remove)(
                    windows::Win32::Foundation::HANDLE(qwave.handle as *mut _),
                    SOCKET(0),
                    self.0,
                    0,
                );
            }
        }
    }
}
