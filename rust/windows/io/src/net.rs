//! Avoid stale UDP ICMP errors aborting a subsequent streaming client.

use anyhow::Result;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket},
    sync::{
        Mutex, PoisonError, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW,
    NotifyIpInterfaceChange, NotifyUnicastIpAddressChange,
};
use windows::Win32::Networking::WinSock::*;
/// The PC's DNS host name, which Vibepollo shows to clients by default.
pub fn host_name() -> Option<String> {
    use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    let mut size = 0u32;
    // SAFETY: A null buffer only queries the required length into the local `size`.
    unsafe {
        let _ = GetComputerNameExW(ComputerNameDnsHostname, None, &mut size);
    }
    if size == 0 || size > 1024 {
        return None;
    }
    let mut name = vec![0u16; size as usize];
    // SAFETY: `name` holds `size` UTF-16 units, the length the previous call reported, and outlives
    // the call.
    unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(windows::core::PWSTR(name.as_mut_ptr())),
            &mut size,
        )
        .ok()?;
    }
    name.truncate(size as usize);
    Some(String::from_utf16_lossy(&name)).filter(|name| !name.trim().is_empty())
}
/// The MAC a magic packet wakes this PC with, for a request that reached it
/// at `address` (IPv4-mapped IPv6 included): the physical NIC carrying it, or
/// the one under a virtual or VPN adapter (`wake::primary`). Zeros for
/// loopback or when no physical adapter is connected.
pub fn local_mac(address: std::net::IpAddr) -> Result<String> {
    if address.to_canonical().is_loopback() {
        return Ok(butterpollo_session::wake::format_mac(None));
    }
    Ok(butterpollo_session::wake::reported_mac(
        &crate::wake::adapters()?,
        address,
        crate::wake::armed,
    ))
}
/// The unicast addresses of interfaces that are up, without loopback and
/// link-local ones.
pub fn unicast_addresses() -> Result<Vec<IpAddr>> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::IpHelper::*,
    };
    let mut size = 15000u32;
    for _ in 0..3 {
        anyhow::ensure!(
            size <= 1024 * 1024,
            "network interface table exceeds its limit"
        );
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(first),
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != ERROR_SUCCESS.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        let mut addresses = Vec::new();
        // SAFETY: As in wake::adapters, the linked records live in `storage`, and each sockaddr is
        // converted only within its reported length.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                let mut unicast = if row.OperStatus.0 == 1 {
                    row.FirstUnicastAddress
                } else {
                    std::ptr::null_mut()
                };
                while let Some(ip) = unicast.as_ref() {
                    let address = socket2::SockAddr::try_init(|storage, length| {
                        let size =
                            (ip.Address.iSockaddrLength.max(0) as usize).min(*length as usize);
                        if !ip.Address.lpSockaddr.is_null() {
                            std::ptr::copy_nonoverlapping(
                                ip.Address.lpSockaddr.cast::<u8>(),
                                storage.cast::<u8>(),
                                size,
                            );
                        }
                        *length = size as i32;
                        Ok(())
                    })
                    .ok()
                    .and_then(|(_, address)| address.as_socket());
                    if let Some(address) = address.map(|a| a.ip().to_canonical())
                        && !address.is_loopback()
                        && !address.is_unspecified()
                        && !matches!(address, IpAddr::V4(v4) if v4.is_link_local())
                        && !matches!(address, IpAddr::V6(v6) if v6.is_unicast_link_local())
                    {
                        addresses.push(address);
                    }
                    unicast = ip.Next;
                }
                adapter = row.Next;
            }
        }
        addresses.sort();
        addresses.dedup();
        return Ok(addresses);
    }
    anyhow::bail!("network interfaces changed during enumeration")
}
/// The next hops of every default route (0.0.0.0/0 and ::/0): the routers this PC sends the
/// internet through. One read of the routing table, no network traffic.
pub fn default_gateways() -> Result<Vec<IpAddr>> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    let mut table = std::ptr::null_mut();
    // SAFETY: `table` is valid on success, read only within its NumEntries rows and freed once.
    unsafe {
        let status = GetIpForwardTable2(AF_UNSPEC, &mut table);
        if status.0 != 0 {
            return Err(std::io::Error::from_raw_os_error(status.0 as i32).into());
        }
        let rows = std::slice::from_raw_parts(
            (&raw const (*table).Table).cast::<MIB_IPFORWARD_ROW2>(),
            (*table).NumEntries as usize,
        );
        let mut gateways = Vec::new();
        for row in rows {
            if row.DestinationPrefix.PrefixLength != 0 {
                continue;
            }
            let hop = row.NextHop;
            // The family field is shared by every view; a view is read only when it names it.
            let address = match hop.si_family {
                AF_INET => Some(IpAddr::from(hop.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())),
                AF_INET6 => Some(IpAddr::from(hop.Ipv6.sin6_addr.u.Byte).to_canonical()),
                _ => None,
            };
            if let Some(address) = address.filter(|address| !address.is_unspecified()) {
                gateways.push(address);
            }
        }
        FreeMibTable(table.cast());
        gateways.sort();
        gateways.dedup();
        Ok(gateways)
    }
}
/// The process that owns the IPv4 TCP connection from `local` to `remote`; None when there is
/// no such connection. Kernel drivers' connections belong to System, PID 4.
pub fn tcp_owner(
    local: std::net::SocketAddrV4,
    remote: std::net::SocketAddrV4,
) -> Result<Option<u32>> {
    use windows::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR},
        NetworkManagement::IpHelper::*,
    };
    let address = |raw: u32| std::net::Ipv4Addr::from(raw.to_ne_bytes());
    // The low 16 bits hold the port in network order.
    let port = |raw: u32| {
        let bytes = raw.to_ne_bytes();
        u16::from_be_bytes([bytes[0], bytes[1]])
    };
    let mut size = 0u32;
    let mut storage = Vec::<u64>::new();
    for _ in 0..4 {
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call;
        // an empty one only queries the size.
        let status = unsafe {
            GetExtendedTcpTable(
                (!storage.is_empty()).then(|| storage.as_mut_ptr().cast()),
                &mut size,
                false,
                AF_INET.0 as u32,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER.0 {
            anyhow::ensure!(size <= 64 << 20, "the TCP table exceeds its limit");
            storage = vec![0u64; (size as usize).div_ceil(8)];
            continue;
        }
        if status != NO_ERROR.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        if storage.is_empty() {
            return Ok(None);
        }
        // SAFETY: The call filled `storage` with a MIB_TCPTABLE_OWNER_PID whose `dwNumEntries`
        // rows follow its count inside the `size` bytes it reported.
        let rows = unsafe {
            let table = &*storage.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
        };
        return Ok(rows
            .iter()
            .find(|row| {
                address(row.dwLocalAddr) == *local.ip()
                    && port(row.dwLocalPort) == local.port()
                    && address(row.dwRemoteAddr) == *remote.ip()
                    && port(row.dwRemotePort) == remote.port()
            })
            .map(|row| row.dwOwningPid));
    }
    anyhow::bail!("TCP connections changed during enumeration")
}
/// Active Ethernet/Wi-Fi IPv4 addresses for Moonlight's manual Add PC flow.
pub fn lan_addresses() -> Result<Vec<String>> {
    Ok(lan_interfaces()?
        .into_iter()
        .map(|interface| interface.address.to_string())
        .collect())
}
/// An active Ethernet or Wi-Fi interface's IPv4 address, as [`lan_interfaces`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanInterface {
    pub address: std::net::Ipv4Addr,
    /// A physical adapter: NDIS calls the interface hardware.
    pub hardware: bool,
    /// Hyper-V's virtual Ethernet adapter, by [`hyper_v_adapter`].
    pub hyper_v: bool,
    /// The IPv4 interface metric: of two routes alike, Windows takes the lower.
    pub metric: u32,
    /// The interface's IPv4 default gateways.
    pub gateways: Vec<std::net::Ipv4Addr>,
}
impl LanInterface {
    /// Whether this PC reaches its router through the interface: a physical adapter, or
    /// Hyper-V's adapter on an external switch, which carries the address of the physical card
    /// it shares and its default gateway. Hyper-V's Default Switch and WSL's adapter have no
    /// default gateway (this PC is theirs), and VPN adapters that call themselves Ethernet (TAP,
    /// ZeroTier) are neither.
    pub fn uplink(&self) -> bool {
        self.hardware || (self.hyper_v && !self.gateways.is_empty())
    }
}
/// Whether an adapter's description, which its driver gives and the user cannot rename (unlike
/// its name, "vEthernet (...)"), is Hyper-V's virtual Ethernet adapter: "Hyper-V Virtual
/// Ethernet Adapter", with " #2" and on for more of them.
pub fn hyper_v_adapter(description: &str) -> bool {
    description.starts_with("Hyper-V Virtual Ethernet Adapter")
}
/// Active Ethernet/Wi-Fi IPv4 addresses with what tells their interfaces apart: Hyper-V, WSL and
/// some VPN adapters (TAP, ZeroTier) also identify as Ethernet. Physical first, then private
/// addresses first.
pub fn lan_interfaces() -> Result<Vec<LanInterface>> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::IpHelper::*,
    };
    let mut size = 15000u32;
    for _ in 0..3 {
        anyhow::ensure!(
            size <= 1024 * 1024,
            "network interface table exceeds its limit"
        );
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_SKIP_ANYCAST
                    | GAA_FLAG_SKIP_MULTICAST
                    | GAA_FLAG_SKIP_DNS_SERVER
                    | GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(first),
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != ERROR_SUCCESS.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        let mut addresses: Vec<LanInterface> = Vec::new();
        // SAFETY: As in wake::adapters, the linked records live in `storage`, and each sockaddr is read
        // as SOCKADDR_IN only when non-null and long enough; the gateway list is read the same way,
        // the description is a NUL-terminated string in the same buffer, and `interface` is a local.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                if row.OperStatus.0 == 1 && matches!(row.IfType, 6 | 71) {
                    let mut interface = MIB_IF_ROW2 {
                        InterfaceLuid: row.Luid,
                        ..Default::default()
                    };
                    let hardware = GetIfEntry2(&mut interface).0 == 0
                        && interface.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
                    let hyper_v = !row.Description.is_null()
                        && row
                            .Description
                            .to_string()
                            .is_ok_and(|description| hyper_v_adapter(&description));
                    let mut gateways = Vec::new();
                    let mut gateway = row.FirstGatewayAddress;
                    while let Some(hop) = gateway.as_ref() {
                        if !hop.Address.lpSockaddr.is_null()
                            && hop.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                        {
                            let socket = &*hop.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            let address =
                                std::net::Ipv4Addr::from(socket.sin_addr.S_un.S_addr.to_ne_bytes());
                            if socket.sin_family == AF_INET && !address.is_unspecified() {
                                gateways.push(address);
                            }
                        }
                        gateway = hop.Next;
                    }
                    let mut unicast = row.FirstUnicastAddress;
                    while let Some(ip) = unicast.as_ref() {
                        if !ip.Address.lpSockaddr.is_null()
                            && ip.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                        {
                            let socket = &*ip.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            if socket.sin_family == AF_INET {
                                let address = std::net::Ipv4Addr::from(
                                    socket.sin_addr.S_un.S_addr.to_ne_bytes(),
                                );
                                if !address.is_loopback()
                                    && !address.is_link_local()
                                    && !address.is_unspecified()
                                {
                                    addresses.push(LanInterface {
                                        address,
                                        hardware,
                                        hyper_v,
                                        metric: row.Ipv4Metric,
                                        gateways: gateways.clone(),
                                    });
                                }
                            }
                        }
                        unicast = ip.Next;
                    }
                }
                adapter = row.Next;
            }
        }
        // Hyper-V/WSL adapters also identify as Ethernet. Prefer a physical
        // interface so first-run instructions point at a reachable LAN address.
        addresses.sort_by_key(|interface| {
            (
                !interface.hardware,
                !interface.address.is_private(),
                interface.address,
            )
        });
        addresses.dedup_by_key(|interface| interface.address);
        return Ok(addresses);
    }
    anyhow::bail!("network interfaces changed during enumeration")
}
pub fn configure_udp(socket: &UdpSocket) -> Result<()> {
    let disabled = 0u32;
    let mut returned = 0;
    // SAFETY: The borrowed socket stays open, `disabled` is the 4-byte input buffer, and the call
    // is synchronous with no output buffer.
    if unsafe {
        WSAIoctl(
            SOCKET(socket.as_raw_socket() as usize),
            SIO_UDP_CONNRESET,
            Some((&disabled as *const u32).cast()),
            4,
            None,
            0,
            &mut returned,
            None,
            None,
        )
    } == SOCKET_ERROR
    {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0).into());
    }
    // Vibepollo's video socket buffer: 4 MiB absorbs startup keyframe and
    // initial audio bursts without the non-blocking socket refusing them.
    socket2::SockRef::from(socket).set_send_buffer_size(SEND_BUFFER_DEFAULT)?;
    Ok(())
}

/// Default UDP send buffer applied by [`configure_udp`] (matches WebRTC media).
pub const SEND_BUFFER_DEFAULT: usize = 4 << 20;
/// Set whether this socket refuses to fragment its later datagrams:
/// `IP_DONTFRAGMENT` and `IPV6_DONTFRAG`, so an oversized probe packet fails
/// with WSAEMSGSIZE at the sender instead of being fragmented and lost whole
/// on a hop that cannot reassemble it. Both options are tried because a
/// dual-stack socket carries both families; the one the socket does not carry
/// refuses its option harmlessly. The PyroWave probe sets it on its own socket
/// and clears it again; the shared video socket never sets it, since the flag
/// would govern every stream using that process-wide socket.
pub fn set_dont_fragment(socket: &UdpSocket, enabled: bool) -> Result<()> {
    let raw = SOCKET(socket.as_raw_socket() as usize);
    let value = u32::from(enabled).to_ne_bytes();
    let mut applied = false;
    let mut last = None;
    for (level, option) in [(IPPROTO_IP, IP_DONTFRAGMENT), (IPPROTO_IPV6, IPV6_DONTFRAG)] {
        // SAFETY: the borrowed socket stays open, and the option takes this 4-byte BOOL.
        if unsafe { setsockopt(raw, level.0, option, Some(&value)) } == SOCKET_ERROR {
            // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
            let error = unsafe { WSAGetLastError() };
            // A family the socket does not carry refuses only its own option.
            if !matches!(error, WSAENOPROTOOPT | WSAEINVAL | WSAEPROTONOSUPPORT) {
                return Err(std::io::Error::from_raw_os_error(error.0).into());
            }
            last = Some(error);
        } else {
            applied = true;
        }
    }
    if applied {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(last.unwrap_or(WSAEINVAL).0).into())
    }
}
/// Send errors after which the socket still works and only these datagrams
/// are lost: a full send buffer, no buffer space, or a route that is down for
/// the moment. Vibepollo drops the packets and keeps streaming; so do we.
fn transient(error: WSA_ERROR) -> bool {
    [
        WSAEWOULDBLOCK,
        WSAENOBUFS,
        WSAENETDOWN,
        WSAENETUNREACH,
        WSAENETRESET,
        WSAEHOSTUNREACH,
        WSAEHOSTDOWN,
        WSAECONNRESET,
        WSAEADDRNOTAVAIL,
    ]
    .contains(&error)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentedFailure {
    DisableOffload,
    FallbackDatagrams,
    DropBatch,
    Fatal,
}
fn classify_segmented_error(error: WSA_ERROR) -> SegmentedFailure {
    if [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE].contains(&error) {
        SegmentedFailure::DisableOffload
    } else if error == WSAENOBUFS {
        SegmentedFailure::FallbackDatagrams
    } else if transient(error) {
        SegmentedFailure::DropBatch
    } else {
        SegmentedFailure::Fatal
    }
}
/// A send or receive error that loses only this datagram; the socket keeps
/// working. A receive also reports a datagram too large for its buffer this
/// way (WSAEMSGSIZE) after discarding it.
pub fn datagram_lost(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error().is_some_and(|code| {
            let code = WSA_ERROR(code);
            transient(code) || code == WSAEMSGSIZE
        })
}
/// How long one send, or all of one batch's sends together, wait for room in
/// a full socket buffer before dropping. Per datagram, 64 refused datagrams
/// would hold the sender for 256 ms. Within a video frame
/// ([`Batch::begin_frame`]) all its batches share one deadline: the later of
/// the frame's own (about a frame period from its start, at most 20 ms) and
/// this long after the frame's first refusal.
pub const WRITABLE_WAIT: std::time::Duration = std::time::Duration::from_millis(4);
const BUFFER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1);
/// Default shared transient `WSAENOBUFS` retry budget for one audio send pass.
pub const AUDIO_ENOBUFS_WAIT: std::time::Duration = std::time::Duration::from_millis(12);

/// Shared retry deadline across one datagram send pass (such as all audio
/// packets captured in one pass) so a congested socket does not multiply waits
/// per packet beyond one audio interval.
#[derive(Clone, Copy, Debug)]
pub struct DatagramPass {
    until: std::time::Instant,
    enobufs_until: std::time::Instant,
    deadline: Option<std::time::Instant>,
    enobufs_deadline: Option<std::time::Instant>,
}

impl DatagramPass {
    /// Start a pass capped at [`WRITABLE_WAIT`] for `WSAEWOULDBLOCK` and
    /// `enobufs_wait` for `WSAENOBUFS`.
    pub fn new(enobufs_wait: std::time::Duration) -> Self {
        let now = std::time::Instant::now();
        Self {
            until: now + WRITABLE_WAIT,
            enobufs_until: now + enobufs_wait,
            deadline: None,
            enobufs_deadline: None,
        }
    }
}

/// The writable wait left to one send call, started at its first full buffer.
#[derive(Default)]
struct WaitBudget {
    deadline: Option<std::time::Instant>,
    /// The frame's own deadline ([`Batch::begin_frame`]), if one is in flight.
    frame_until: Option<std::time::Instant>,
    /// Optional separate retry window for transient `WSAENOBUFS` pressure so
    /// startup bursts and high-refresh frames recover without slowing down
    /// `WSAEWOULDBLOCK` waits or adding latency to successful sends.
    enobufs_deadline: Option<std::time::Instant>,
    enobufs_until: Option<std::time::Instant>,
    /// Full-buffer refusals seen by this send call.
    refusals: u32,
    backoff_timer: Option<crate::os_helpers::Timer>,
}
impl WaitBudget {
    /// A budget shared by every batch of the frame in flight: `deadline` is
    /// the frame's, once a batch of it met a full socket, so a socket that
    /// stays full is not waited on again for each batch and the rest of that
    /// frame is abandoned instead; `until` is the frame's own deadline.
    #[cfg(test)]
    fn for_frame(until: Option<std::time::Instant>, deadline: Option<std::time::Instant>) -> Self {
        Self::for_frame_with_enobufs(until, deadline, None, None)
    }
    fn for_frame_with_enobufs(
        until: Option<std::time::Instant>,
        deadline: Option<std::time::Instant>,
        enobufs_until: Option<std::time::Instant>,
        enobufs_deadline: Option<std::time::Instant>,
    ) -> Self {
        Self {
            deadline,
            frame_until: until,
            enobufs_deadline,
            enobufs_until,
            ..Default::default()
        }
    }
    /// The deadline, set at the first full buffer: [`WRITABLE_WAIT`] from
    /// then, or the frame's own deadline if that is later. A frame that took
    /// most of its period to pace out still gets the usual wait.
    fn deadline_at(&mut self, now: std::time::Instant) -> std::time::Instant {
        let wait = now + WRITABLE_WAIT;
        *self
            .deadline
            .get_or_insert(self.frame_until.map_or(wait, |until| until.max(wait)))
    }
    fn deadline_for(&mut self, now: std::time::Instant, enobufs: bool) -> std::time::Instant {
        if enobufs && (self.enobufs_until.is_some() || self.enobufs_deadline.is_some()) {
            let wait = now + WRITABLE_WAIT;
            *self
                .enobufs_deadline
                .get_or_insert(self.enobufs_until.map_or(wait, |until| until.max(wait)))
        } else {
            self.deadline_at(now)
        }
    }
    /// Count a full-buffer refusal; true from the second on, when the retry
    /// should pause first. A socket can poll writable and still refuse a
    /// large segmented batch, and polling alone would spin on it.
    fn refused_again(&mut self) -> bool {
        self.refusals += 1;
        self.refusals > 1
    }
    /// Milliseconds to wait for a full buffer now; 0 once the budget is spent.
    fn take(&mut self, now: std::time::Instant) -> i32 {
        let left = self.deadline_at(now).saturating_duration_since(now);
        left.as_micros().div_ceil(1000) as i32
    }
    /// Reserve a short pause only when it leaves time for the retry. All
    /// datagrams in a batch share this deadline, including time already spent
    /// polling a full socket or sending earlier datagrams.
    fn backoff_for(
        &mut self,
        now: std::time::Instant,
        enobufs: bool,
    ) -> Option<std::time::Instant> {
        let retry = now + BUFFER_RETRY_DELAY;
        (retry < self.deadline_for(now, enobufs)).then_some(retry)
    }
    #[cfg(test)]
    fn backoff_at(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        self.backoff_for(now, false)
    }
}
/// Wait until `socket` has a datagram to read, or `timeout_ms` passes;
/// returns whether one is there. Unlike a fixed sleep, input waiting on the
/// socket is handled the moment it arrives.
pub fn wait_readable(
    socket: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket], timeout_ms)
}
/// Include host feedback in the same wait as incoming control traffic.
pub fn wait_readable_or_wake(
    socket: std::os::windows::io::RawSocket,
    wake: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket, wake], timeout_ms)
}
fn wait_readable_sockets<const N: usize>(
    sockets: [std::os::windows::io::RawSocket; N],
    timeout_ms: i32,
) -> std::io::Result<bool> {
    let mut poll = sockets.map(|socket| WSAPOLLFD {
        fd: SOCKET(socket as usize),
        events: POLLRDNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    });
    // SAFETY: `poll` holds N entries and outlives the call; a stale socket only makes
    // WSAPoll fail.
    match unsafe { WSAPoll(poll.as_mut_ptr(), N as u32, timeout_ms) } {
        ready if ready >= 0 => Ok(ready > 0),
        _ => Err(std::io::Error::from_raw_os_error(
            // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
            unsafe { WSAGetLastError() }.0,
        )),
    }
}
#[derive(Clone, Copy, Default)]
pub struct WritableWaits {
    pub count: u64,
    pub elapsed: std::time::Duration,
    /// Last transient socket refusal, including one followed by a successful retry.
    pub last_error: i32,
}
/// ENOBUFS can come from transport/driver allocation pressure while the socket
/// still polls writable. Pause briefly before a retry rather than hot-polling
/// that readiness bit or dropping an entire segmented batch immediately. A
/// repeated WSAEWOULDBLOCK pauses the same way before it polls again.
fn buffer_available(
    socket: &UdpSocket,
    error: WSA_ERROR,
    budget: &mut WaitBudget,
    mut trace: Option<&mut WritableWaits>,
) -> bool {
    if let Some(trace) = trace.as_mut() {
        trace.last_error = error.0;
    }
    let again = budget.refused_again();
    if error == WSAEWOULDBLOCK {
        if again && !pause(budget, false, trace.as_deref_mut()) {
            return false;
        }
        return writable(socket, budget, trace);
    }
    debug_assert_eq!(error, WSAENOBUFS);
    pause(budget, true, trace)
}

/// Wait [`BUFFER_RETRY_DELAY`] if the budget leaves time for a retry after
/// it; false when it does not.
fn pause(budget: &mut WaitBudget, enobufs: bool, trace: Option<&mut WritableWaits>) -> bool {
    let started = std::time::Instant::now();
    let Some(retry_at) = budget.backoff_for(started, enobufs) else {
        return false;
    };
    if budget.backoff_timer.is_none() {
        budget.backoff_timer = crate::os_helpers::Timer::new().ok();
    }
    let Some(timer) = budget.backoff_timer.as_ref() else {
        return false;
    };
    timer.until(retry_at);
    let finished = std::time::Instant::now();
    if let Some(trace) = trace {
        trace.count += 1;
        trace.elapsed += finished.saturating_duration_since(started);
    }
    // A delayed timer wake must not revive an expired batch budget.
    finished < budget.deadline_for(started, enobufs)
}

/// A successful UDP send must never be repeated. Retry buffer pressure for as
/// long as the caller's budget allows, so a socket that is full for a moment
/// (a busy CPU, a late transmit completion) carries the batch instead of
/// losing it; `wait` paces the retries ([`buffer_available`] pauses 1 ms
/// before each after the first). All other errors retain their existing
/// drop/fatal handling.
fn send_with_retry<T>(
    mut send: impl FnMut() -> std::result::Result<T, WSA_ERROR>,
    mut wait: impl FnMut(WSA_ERROR) -> bool,
) -> std::result::Result<T, WSA_ERROR> {
    loop {
        match send() {
            Err(error) if [WSAEWOULDBLOCK, WSAENOBUFS].contains(&error) && wait(error) => continue,
            result => return result,
        }
    }
}
fn writable(
    socket: &UdpSocket,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> bool {
    let timeout_ms = budget.take(std::time::Instant::now());
    if timeout_ms == 0 {
        return false;
    }
    let started = trace.as_ref().map(|_| std::time::Instant::now());
    let mut poll = [WSAPOLLFD {
        fd: SOCKET(socket.as_raw_socket() as usize),
        events: POLLWRNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    }];
    // SAFETY: `poll` is a one-element array that outlives the call, for a socket the borrow keeps
    // open.
    let ready = (unsafe { WSAPoll(poll.as_mut_ptr(), 1, timeout_ms) }) > 0
        && poll[0].revents.0 & POLLWRNORM.0 != 0;
    if let (Some(trace), Some(started)) = (trace, started) {
        trace.count += 1;
        trace.elapsed += started.elapsed();
    }
    ready
}
/// One datagram from `source`, retried once when the socket buffer is full
/// and frees up within `budget`. The inner error is Winsock's.
fn send_one(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> Result<std::result::Result<(), WSA_ERROR>> {
    let send = || send_from(socket, packet, peer, source);
    let mut trace = trace;
    let result = send_with_retry(send, |error| {
        buffer_available(socket, error, budget, trace.as_deref_mut())
    });
    Ok(match result {
        Ok(size) => {
            anyhow::ensure!(
                size == packet.len(),
                "Winsock returned an incomplete UDP datagram"
            );
            Ok(())
        }
        Err(error) => Err(error),
    })
}
fn send_datagram_budget(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    budget: &mut WaitBudget,
) -> Result<bool> {
    let (result, _) = socket.send_pinned(peer, socket.source(peer), |source| {
        send_one(socket, packet, peer, source, budget, None)
    })?;
    match result {
        Ok(()) => Ok(true),
        Err(error) if transient(error) => Ok(false),
        Err(error) => Err(std::io::Error::from_raw_os_error(error.0).into()),
    }
}
/// Shared wait budget across a group of datagrams sent via [`send_datagram_with_wait`].
#[derive(Default)]
pub struct DatagramWait(WaitBudget);
/// One datagram, from the address `peer` reached, sharing `wait` so a stalled
/// socket blocks at most [`WRITABLE_WAIT`] across the whole group; `Ok(false)`
/// when a transient error dropped it.
pub fn send_datagram_with_wait(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    wait: &mut DatagramWait,
) -> Result<bool> {
    send_datagram_budget(socket, packet, peer, &mut wait.0)
}
/// One datagram, from the address `peer` reached; `Ok(false)` when a
/// transient error dropped it.
pub fn send_datagram(socket: &ReplySocket, packet: &[u8], peer: SocketAddr) -> Result<bool> {
    send_datagram_with_wait(socket, packet, peer, &mut DatagramWait::default())
}
/// One datagram using a [`DatagramPass`] shared across a capture pass so
/// `WSAENOBUFS` can recover while persistent refusal fails subsequent packets
/// immediately without multiplying retry sleeps.
pub fn send_datagram_pass(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    pass: &mut DatagramPass,
) -> Result<bool> {
    let mut budget = WaitBudget::for_frame_with_enobufs(
        Some(pass.until),
        pass.deadline,
        Some(pass.enobufs_until),
        pass.enobufs_deadline,
    );
    let ok = send_datagram_budget(socket, packet, peer, &mut budget)?;
    pass.deadline = budget.deadline.filter(|_| !ok);
    pass.enobufs_deadline = budget.enobufs_deadline.filter(|_| !ok);
    Ok(ok)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// Physical Ethernet speed; zero means unknown.
    pub bps: u64,
    pub wireless: bool,
    /// The NDIS locally unique identifier (LUID) of the interface the route
    /// leaves by; zero means unknown. A host that changes network adapter
    /// has a different path to the same client. It names the adapter, not the
    /// network it is joined to.
    pub interface: u64,
}
fn interface_link(row: &windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2) -> Link {
    use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp};
    if row.OperStatus != IfOperStatusUp {
        return Link::default();
    }
    Link {
        // SAFETY: `Value` and the bitfield view of the LUID union are the same eight bytes.
        interface: unsafe { row.InterfaceLuid.Value },
        bps: if row.Type == IF_TYPE_ETHERNET_CSMACD
            && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
        {
            row.TransmitLinkSpeed
        } else {
            0
        },
        wireless: matches!(
            row.Type,
            IF_TYPE_IEEE80211 | IF_TYPE_IEEE80216_WMAN | IF_TYPE_WWANPP | IF_TYPE_WWANPP2
        ),
    }
}
fn underlying_link(
    mut index: u32,
    stack: &[windows::Win32::NetworkManagement::IpHelper::MIB_IFSTACK_ROW],
    mut entry: impl FnMut(u32) -> Option<windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2>,
) -> Link {
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(index) {
        let mut lower = stack.iter().filter(|row| {
            row.HigherLayerInterfaceIndex == index && row.LowerLayerInterfaceIndex != 0
        });
        let Some(row) = lower.next() else { break };
        // A team or ambiguous binding cannot establish the route's medium.
        if lower.next().is_some() {
            break;
        }
        index = row.LowerLayerInterfaceIndex;
        let Some(row) = entry(index) else { break };
        if row.OperStatus != windows::Win32::NetworkManagement::Ndis::IfOperStatusUp {
            break;
        }
        if row.InterfaceAndOperStatusFlags._bitfield & 1 != 0 {
            return interface_link(&row);
        }
    }
    Link::default()
}
/// Classify the route, using a virtual Ethernet adapter's physical binding
/// when Windows exposes an unambiguous interface stack.
pub fn routed_link(peer: SocketAddr) -> Link {
    use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp};
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    if peer.ip().is_loopback() {
        return Link::default();
    }
    let address = socket2::SockAddr::from(peer);
    let mut index = 0;
    // SAFETY: `address` and `row` outlive the calls that read and fill them; `table` is valid on
    // success and freed only after `stack`, its NumEntries rows, is last used.
    unsafe {
        if GetBestInterfaceEx(address.as_ptr().cast(), &mut index) != 0 {
            return Link::default();
        }
        let mut row = MIB_IF_ROW2 {
            InterfaceIndex: index,
            ..Default::default()
        };
        // Without statistics: the counters cost the driver a query, and
        // only the interface's type, state and speed are read.
        if GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row).0 != 0
            || row.OperStatus != IfOperStatusUp
        {
            return Link::default();
        }
        // The interface the route leaves by, whatever the medium under it.
        let interface = row.InterfaceLuid.Value;
        if row.Type != IF_TYPE_ETHERNET_CSMACD || row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
        {
            return interface_link(&row);
        }
        let mut table = std::ptr::null_mut();
        if GetIfStackTable(&mut table).0 != 0 {
            return Link::default();
        }
        let stack = std::slice::from_raw_parts(
            (&raw const (*table).Table).cast(),
            (*table).NumEntries as usize,
        );
        let link = underlying_link(index, stack, |index| {
            let mut row = MIB_IF_ROW2 {
                InterfaceIndex: index,
                ..Default::default()
            };
            (GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row).0 == 0).then_some(row)
        });
        FreeMibTable(table.cast());
        Link { interface, ..link }
    }
}
/// Physical Ethernet speed on the route to this client; zero means unknown.
pub fn routed_link_bps(peer: SocketAddr) -> u64 {
    routed_link(peer).bps
}
/// Whether Windows reaches `peer` without a gateway, so it is on this host's
/// link. A route with a next hop crosses a router that may carry a smaller MTU,
/// which the host cannot see, so jumbo packets are offered only on-link.
pub fn routed_same_link(peer: SocketAddr) -> bool {
    use windows::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows::Win32::Networking::WinSock::SOCKADDR_INET;
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    let address = socket2::SockAddr::from(peer);
    let mut route = MIB_IPFORWARD_ROW2::default();
    let mut source = SOCKADDR_INET::default();
    // SAFETY: `address` outlives the call; `route` and `source` are live locals
    // the call fills before they are read on success.
    unsafe {
        GetBestRoute2(
            None,
            0,
            None,
            address.as_ptr().cast(),
            0,
            &mut route,
            &mut source,
        )
        .ok()
        .is_ok_and(|()| next_hop_unspecified(&route.NextHop))
    }
}

/// Whether a route's next hop is unspecified: the destination is on-link.
fn next_hop_unspecified(next: &windows::Win32::Networking::WinSock::SOCKADDR_INET) -> bool {
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    // SAFETY: `si_family` is the union's tag; matching it is the contract for
    // reading the selected member, which Windows initialised.
    unsafe {
        if next.si_family == AF_INET {
            next.Ipv4.sin_addr.S_un.S_addr == 0
        } else if next.si_family == AF_INET6 {
            next.Ipv6.sin6_addr.u.Byte.iter().all(|byte| *byte == 0)
        } else {
            // An unknown family is not proven on-link; offer nothing.
            false
        }
    }
}

/// The payload MTU of the interface Windows routes `peer` through, IPv4 or IPv6:
/// the size a datagram may have before the network fragments it. None when the
/// lookup fails or there is no route.
pub fn routed_mtu(peer: SocketAddr) -> Option<u32> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetBestInterfaceEx, GetIpInterfaceEntry, MIB_IPINTERFACE_ROW,
    };
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    let address = socket2::SockAddr::from(peer);
    let mut index = 0;
    // SAFETY: `address` outlives the call that only reads it, and `row` is
    // filled by the call before `NlMtu` is read on success.
    unsafe {
        if GetBestInterfaceEx(address.as_ptr().cast(), &mut index) != 0 {
            return None;
        }
        let mut row = MIB_IPINTERFACE_ROW {
            Family: if peer.is_ipv4() { AF_INET } else { AF_INET6 },
            InterfaceIndex: index,
            ..Default::default()
        };
        if GetIpInterfaceEntry(&mut row).0 != 0 {
            return None;
        }
        Some(row.NlMtu)
    }
}
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
/// Per-message segmentation leaves the shared socket's options untouched.
pub struct Batch {
    offload: bool,
    pub system_calls: u64,
    /// Datagrams lost to transient send errors.
    pub dropped: u64,
    /// Packets handed to Winsock in the last [`Batch::send`] call.
    last_sent: usize,
    /// Datagrams the path refused because they are larger than its MTU with
    /// `IP_DONTFRAGMENT`/`IPV6_DONTFRAG` set; also counted in `dropped`.
    pub oversized: u64,
    /// The sender's socket refuses fragmentation ([`set_dont_fragment`]), as
    /// the PyroWave probe's does for a jumbo path, so an oversized datagram is
    /// a lost packet to report, not a send error that ends the stream.
    pub dont_fragment: bool,
    /// Enabled by the sender only for pacing traces.
    pub waits: Option<WritableWaits>,
    /// The frame in flight (see [`Batch::begin_frame`]): its own deadline,
    /// and the deadline its batches share once one met a full socket. `None`
    /// outside a frame, where one batch waits [`WRITABLE_WAIT`] on its own.
    frame: Option<FrameWait>,
    unreported: u64,
    reported: Option<std::time::Instant>,
    /// Scratch WSABUFs filled for each segmented send, so their 1 KiB is not
    /// zeroed anew every batch.
    buffers: [WSABUF; 64],
    /// The control data and address of the last segmented send, rebuilt only
    /// when the peer, the first packet's size or the source changes.
    segmented: Option<Segmented>,
}
impl Default for Batch {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
            dropped: 0,
            last_sent: 0,
            oversized: 0,
            dont_fragment: false,
            waits: None,
            frame: None,
            unreported: 0,
            reported: None,
            buffers: [WSABUF {
                len: 0,
                buf: windows::core::PSTR::null(),
            }; 64],
            segmented: None,
        }
    }
}
#[derive(Clone, Copy)]
struct FrameWait {
    until: std::time::Instant,
    deadline: Option<std::time::Instant>,
    enobufs_until: Option<std::time::Instant>,
    enobufs_deadline: Option<std::time::Instant>,
}
fn segmented_retries_in_place(error: WSA_ERROR) -> bool {
    error != WSAENOBUFS
}
impl Batch {
    /// Start one video frame: its batches share one wait for room in a full
    /// socket, up to `wait` from now or [`WRITABLE_WAIT`] from the frame's
    /// first refusal, whichever is later (see [`Batch::frame_wait_spent`]).
    pub fn begin_frame(&mut self, wait: std::time::Duration) {
        let now = std::time::Instant::now();
        self.frame = Some(FrameWait {
            until: now + wait,
            deadline: None,
            enobufs_until: None,
            enobufs_deadline: None,
        });
    }
    /// Start one video frame with a separate shared retry ceiling for
    /// transient `WSAENOBUFS` driver pressure, keeping `WSAEWOULDBLOCK` at
    /// `wait` and adding zero delay when the socket accepts datagrams.
    pub fn begin_frame_with_enobufs(
        &mut self,
        wait: std::time::Duration,
        enobufs_wait: std::time::Duration,
    ) {
        let now = std::time::Instant::now();
        self.frame = Some(FrameWait {
            until: now + wait,
            deadline: None,
            enobufs_until: Some(now + enobufs_wait),
            enobufs_deadline: None,
        });
    }
    /// The frame is over: a later batch outside a frame gets its own wait.
    pub fn end_frame(&mut self) {
        self.frame = None;
    }
    /// The frame in flight met a full socket and its shared wait is over:
    /// its sender gives up the rest of it.
    pub fn frame_wait_spent(&self) -> bool {
        let now = std::time::Instant::now();
        self.frame.is_some_and(|frame| {
            [frame.deadline, frame.enobufs_deadline]
                .into_iter()
                .flatten()
                .any(|deadline| now >= deadline)
        })
    }
    /// How many of the leading `packets` one segmented send carries: equal
    /// sizes, then at most one shorter, at most 64 and 65507 bytes in all.
    /// The shorter tail rides along, as it would otherwise take a paced
    /// iteration and a syscall of its own. An empty datagram is never in a
    /// batch: segmentation would not send it.
    pub fn count(packets: &[Vec<u8>], limit: usize) -> usize {
        let Some(first) = packets.first() else {
            return 0;
        };
        let size = first.len();
        if size == 0 {
            return 1;
        }
        let limit = limit.min(65507);
        let max = (limit / size).clamp(1, 64);
        let (mut count, mut total) = (0, 0);
        for packet in packets.iter().take(max) {
            if packet.len() != size {
                break;
            }
            count += 1;
            total += size;
        }
        if let Some(tail) = packets.get(count) {
            let len = tail.len();
            if count < max && len > 0 && len < size && total + len <= 65507 {
                count += 1;
            }
        }
        count.max(1)
    }
    /// Count dropped datagrams and report them at most every 5 seconds.
    fn dropped(&mut self, count: usize, error: WSA_ERROR) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error.0;
        }
        self.dropped += count as u64;
        self.unreported += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            tracing::warn!(
                code = error.0,
                dropped = self.unreported,
                "UDP send failed: the socket stayed full, so packets were dropped and the client may stutter. The PC could not send fast enough (its CPU or network adapter fell behind); close other work on the PC or lower the bitrate"
            );
            self.unreported = 0;
            self.reported = Some(std::time::Instant::now());
        }
    }
    /// Count datagrams as refused by a full socket without sending them,
    /// for `BUTTERPOLLO_TEST_SEND_OUTAGE`.
    pub fn refuse(&mut self, count: usize) {
        self.last_sent = 0;
        self.dropped(count, WSAEWOULDBLOCK);
    }
    /// Number of packets sent in the last [`Batch::send`] call.
    pub fn sent_packets(&self) -> usize {
        self.last_sent
    }
    /// Count datagrams the path refused for their size with fragmentation
    /// refused: lost like a full buffer's, and reported once every 5 seconds as
    /// the jumbo path's fault rather than a transient socket error.
    fn too_large(&mut self, count: usize, error: WSA_ERROR) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error.0;
        }
        self.dropped += count as u64;
        self.oversized += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            tracing::warn!(
                code = error.0,
                dropped = count,
                "video datagrams exceed the path MTU and are dropped: the offered packet size plus 64 bytes of RTP, UDP and IPv6 headers must fit every hop's MTU"
            );
            self.reported = Some(std::time::Instant::now());
        }
    }
    /// Bytes handed to Winsock. Datagrams that hit a transient error are
    /// dropped, as Vibepollo does, rather than ending the stream. The batch
    /// shares one writable wait: under congestion it drops rather than
    /// holding the frame for a wait per datagram. They leave from the
    /// address `peer` reached.
    pub fn send(
        &mut self,
        socket: &ReplySocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
    ) -> Result<usize> {
        self.last_sent = 0;
        if packets.is_empty() {
            return Ok(0);
        }
        let mut source = socket.source(peer);
        let mut budget = WaitBudget::for_frame_with_enobufs(
            self.frame.map(|frame| frame.until),
            self.frame.and_then(|frame| frame.deadline),
            self.frame.and_then(|frame| frame.enobufs_until),
            self.frame.and_then(|frame| frame.enobufs_deadline),
        );
        let dropped = self.dropped;
        let result = self.send_within(socket, packets, peer, &mut source, &mut budget);
        if let Some(frame) = self.frame.as_mut() {
            // The shared deadline lasts only while the socket stays full: a
            // batch that dropped nothing ends that spell, and a later refusal
            // starts a new one with at least WRITABLE_WAIT.
            frame.deadline = budget.deadline.filter(|_| self.dropped > dropped);
            frame.enobufs_deadline = budget.enobufs_deadline.filter(|_| self.dropped > dropped);
        }
        result
    }
    fn send_within(
        &mut self,
        socket: &ReplySocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: &mut Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<usize> {
        if self.offload
            && packets.len() > 1
            && packets.len() <= 64
            && !packets[0].is_empty()
            && packets[..packets.len() - 1]
                .iter()
                .all(|p| p.len() == packets[0].len())
            && (1..=packets[0].len()).contains(&packets.last().unwrap().len())
        {
            let result;
            (result, *source) = socket.send_pinned(peer, *source, |source| {
                self.send_segmented(socket, packets, peer, source, budget)
            })?;
            match result {
                Ok(sent) => {
                    self.last_sent = packets.len();
                    return Ok(sent);
                }
                Err(error) => match classify_segmented_error(error) {
                    // WSAEMSGSIZE here can be the offload's own limit, not the
                    // path's MTU: turn segmentation off and send the datagrams one
                    // by one, where an oversized one is counted as such. Dropping
                    // the batch would leave every later batch dropped as well.
                    SegmentedFailure::DisableOffload => {
                        self.offload = false;
                        tracing::warn!(
                            code = error.0,
                            "UDP segmentation unavailable or this batch could not be segmented; using individual datagrams with more CPU overhead. Update the network driver or lower bitrate if sending stalls"
                        );
                    }
                    // Transient ENOBUFS on a multi-packet GSO buffer can still
                    // leave room for individual datagrams; keep offload enabled
                    // for later batches once driver pressure clears.
                    SegmentedFailure::FallbackDatagrams => {}
                    SegmentedFailure::DropBatch => {
                        self.dropped(packets.len(), error);
                        return Ok(0);
                    }
                    SegmentedFailure::Fatal => {
                        return Err(std::io::Error::from_raw_os_error(error.0).into());
                    }
                },
            }
        }
        let mut sent = 0;
        for (index, packet) in packets.iter().enumerate() {
            let result;
            (result, *source) = socket.send_pinned(peer, *source, |source| {
                self.system_calls += 1;
                send_one(socket, packet, peer, source, budget, self.waits.as_mut())
            })?;
            match result {
                Ok(()) => {
                    sent += packet.len();
                    self.last_sent += 1;
                }
                // A socket that refuses fragmentation reports a datagram larger than the
                // path's MTU this way, as the PyroWave probe does on a jumbo path. Drop
                // it like a path-MTU refusal; ending the probe's or a stream's send is
                // worse than losing that datagram.
                Err(error) if error == WSAEMSGSIZE => {
                    self.too_large(1, error);
                }
                Err(error) if transient(error) => {
                    self.dropped(packets.len() - index, error);
                    break;
                }
                Err(error) => return Err(std::io::Error::from_raw_os_error(error.0).into()),
            }
        }
        Ok(sent)
    }
    /// One WSASendMsg with UDP segmentation from `source`, retried once when
    /// the socket buffer is full and frees up within `budget`. The inner error
    /// is Winsock's.
    fn send_segmented(
        &mut self,
        socket: &UdpSocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<std::result::Result<usize, WSA_ERROR>> {
        let size = u32::try_from(packets[0].len())?;
        // Rebuild the address and control data only when they changed, then
        // fill the reused scratch buffers and count their bytes in one pass.
        if self
            .segmented
            .as_ref()
            .is_none_or(|s| s.peer != peer || s.size != size || s.source != source)
        {
            let mut control = Ancillary::default();
            control.push(IPPROTO_UDP, UDP_SEND_MSG_SIZE, size);
            if let Some(source) = source {
                control.source(source);
            }
            self.segmented = Some(Segmented {
                peer,
                size,
                source,
                control,
                address: socket2::SockAddr::from(peer),
            });
        }
        let mut expected = 0usize;
        for (buffer, packet) in self.buffers.iter_mut().zip(packets) {
            buffer.len = packet.len() as u32;
            buffer.buf = windows::core::PSTR(packet.as_ptr().cast_mut());
            expected += packet.len();
        }
        let buffers = &self.buffers[..packets.len()];
        let segmented = self.segmented.as_ref().unwrap();
        let system_calls = &mut self.system_calls;
        let result = send_with_retry(
            || {
                *system_calls += 1;
                send_message(socket, buffers, &segmented.address, &segmented.control)
            },
            // Transient ENOBUFS on a multi-packet GSO buffer falls back immediately
            // to individual datagrams (which get the full enobufs_deadline) without
            // spending the recovery budget inside send_segmented or disabling offload.
            |error| {
                segmented_retries_in_place(error)
                    && buffer_available(socket, error, budget, self.waits.as_mut())
            },
        );
        Ok(match result {
            Ok(sent) => {
                anyhow::ensure!(sent == expected, "Winsock returned an incomplete UDP batch");
                Ok(sent)
            }
            Err(error) => Err(error),
        })
    }
}
/// WSABUFs over the first 64 of `packets`, the rest empty. They borrow the
/// packets' bytes without the borrow checker knowing: use them while
/// `packets` lives.
fn wsa_buffers<T: AsRef<[u8]>>(packets: &[T]) -> [WSABUF; 64] {
    let mut buffers = [WSABUF {
        len: 0,
        buf: windows::core::PSTR::null(),
    }; 64];
    for (buffer, packet) in buffers.iter_mut().zip(packets) {
        let packet = packet.as_ref();
        buffer.len = packet.len() as u32;
        buffer.buf = windows::core::PSTR(packet.as_ptr().cast_mut());
    }
    buffers
}
/// Datagrams of any size from a plain socket to one peer, in order and in as
/// few system calls as UDP segmentation allows: each run of equal-sized
/// datagrams, the last of a run maybe shorter, goes in one WSASendMsg. For
/// WebRTC, whose RTP packets are equal but for each frame's last and whose
/// other traffic is any size. Without segmentation they go one by one.
pub struct Segments {
    offload: bool,
    pub system_calls: u64,
}
impl Default for Segments {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
        }
    }
}
/// What became of one [`Segments::send`].
#[derive(Debug, Default)]
pub struct Sent {
    pub datagrams: usize,
    pub bytes: usize,
    /// Datagrams lost to send errors, and the last such error.
    pub failed: usize,
    pub error: Option<std::io::Error>,
}
impl Segments {
    /// How many of the leading `packets` one segmented send carries: equal
    /// sizes, then at most one shorter, at most 64 and 65507 bytes in all.
    /// An empty datagram is never in a run: segmentation would not send it.
    pub fn run<T: AsRef<[u8]>>(packets: &[T]) -> usize {
        let Some(size) = packets.first().map(|p| p.as_ref().len()) else {
            return 0;
        };
        if size == 0 {
            return 1;
        }
        let (mut count, mut total) = (0, 0);
        for len in packets.iter().take(64).map(|p| p.as_ref().len()) {
            if len > size || len == 0 || total + len > 65507 {
                break;
            }
            count += 1;
            total += len;
            if len < size {
                break;
            }
        }
        count.max(1)
    }
    /// Send `packets` to `peer`. An error loses only the datagrams it hit:
    /// the caller counts them and goes on.
    pub fn send<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> Sent {
        let mut sent = Sent::default();
        let mut rest = packets;
        while !rest.is_empty() {
            let (run, tail) = rest.split_at(if self.offload { Self::run(rest) } else { 1 });
            rest = tail;
            if run.len() > 1 {
                let expected: usize = run.iter().map(|p| p.as_ref().len()).sum();
                match self.send_run(socket, run, peer) {
                    Ok(bytes) if bytes == expected => {
                        sent.datagrams += run.len();
                        sent.bytes += bytes;
                        continue;
                    }
                    // A datagram socket sends all or nothing; a short count
                    // would be a Winsock bug. Loud, counted as lost, and not
                    // sent again: some of it may have gone.
                    Ok(bytes) => {
                        tracing::warn!(bytes, expected, "Winsock sent part of a UDP batch");
                        sent.failed += run.len();
                        sent.error =
                            Some(std::io::Error::other("Winsock sent part of a UDP batch"));
                        continue;
                    }
                    Err(error)
                        if [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE]
                            .contains(&error) =>
                    {
                        self.offload = false;
                        tracing::warn!(
                            code = error.0,
                            "UDP segmentation unavailable; WebRTC sends individual datagrams with more CPU overhead. Update the network driver if sending stalls"
                        );
                    }
                    // Nothing of the run went. One by one, as before
                    // segmentation, a full buffer or a route that comes back
                    // loses only the datagrams it hits.
                    Err(_) => {}
                }
            }
            for packet in run {
                self.system_calls += 1;
                match socket.send_to(packet.as_ref(), peer) {
                    Ok(_) => {
                        sent.datagrams += 1;
                        sent.bytes += packet.as_ref().len();
                    }
                    Err(error) => {
                        sent.failed += 1;
                        sent.error = Some(error);
                    }
                }
            }
        }
        sent
    }
    /// One WSASendMsg of a [`Segments::run`], segmented at its first
    /// datagram's size. The error is Winsock's.
    fn send_run<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> std::result::Result<usize, WSA_ERROR> {
        let mut control = Ancillary::default();
        // A run is at most 65507 bytes, so its first datagram's size fits.
        control.push(
            IPPROTO_UDP,
            UDP_SEND_MSG_SIZE,
            packets[0].as_ref().len() as u32,
        );
        let buffers = wsa_buffers(packets);
        self.system_calls += 1;
        send_message(
            socket,
            &buffers[..packets.len()],
            &socket2::SockAddr::from(peer),
            &control,
        )
    }
}
/// WSASendMsg control data: whole messages, each padded to 8 bytes as
/// WSA_CMSG_SPACE pads them.
#[repr(C, align(8))]
struct Ancillary {
    bytes: [u8; 64],
    len: usize,
}
impl Default for Ancillary {
    fn default() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
        }
    }
}
impl Ancillary {
    fn push<T: Copy>(&mut self, level: IPPROTO, kind: i32, data: T) {
        let header = std::mem::size_of::<CMSGHDR>();
        let size = header + std::mem::size_of::<T>();
        let end = self.len + size.next_multiple_of(8);
        assert!(end <= self.bytes.len(), "control data too large");
        // SAFETY: the header and `data` end within `bytes` (asserted above), and unaligned writes
        // need no alignment.
        unsafe {
            let at = self.bytes.as_mut_ptr().add(self.len);
            at.cast::<CMSGHDR>().write_unaligned(CMSGHDR {
                cmsg_len: size,
                cmsg_level: level.0,
                cmsg_type: kind,
            });
            at.add(header).cast::<T>().write_unaligned(data);
        }
        self.len = end;
    }
    /// Send from `source`, as Moonlight's ENet does: IP_PKTINFO for IPv4, also
    /// on a dual-stack socket, IPV6_PKTINFO for IPv6.
    fn source(&mut self, source: IpAddr) {
        match source.to_canonical() {
            IpAddr::V4(source) => self.push(
                IPPROTO_IP,
                IP_PKTINFO,
                IN_PKTINFO {
                    ipi_addr: IN_ADDR {
                        S_un: IN_ADDR_0 {
                            S_addr: u32::from_ne_bytes(source.octets()),
                        },
                    },
                    ipi_ifindex: 0,
                },
            ),
            IpAddr::V6(source) => self.push(
                IPPROTO_IPV6,
                IPV6_PKTINFO,
                IN6_PKTINFO {
                    ipi6_addr: IN6_ADDR {
                        u: IN6_ADDR_0 {
                            Byte: source.octets(),
                        },
                    },
                    ipi6_ifindex: 0,
                },
            ),
        }
    }
}
/// The control data and address of one segmented send, kept so a frame's
/// batches rebuild them only when the peer, the first packet's size or the
/// source changes.
struct Segmented {
    peer: SocketAddr,
    size: u32,
    source: Option<IpAddr>,
    control: Ancillary,
    address: socket2::SockAddr,
}
/// One WSASendMsg of `buffers` to `peer` with `control`. The error is Winsock's.
fn send_message(
    socket: &UdpSocket,
    buffers: &[WSABUF],
    peer: &socket2::SockAddr,
    control: &Ancillary,
) -> std::result::Result<usize, WSA_ERROR> {
    let message = WSAMSG {
        name: peer.as_ptr().cast_mut().cast(),
        namelen: peer.len(),
        // WSASendMsg only reads the buffers.
        lpBuffers: buffers.as_ptr().cast_mut(),
        dwBufferCount: buffers.len() as u32,
        Control: WSABUF {
            len: control.len as u32,
            buf: windows::core::PSTR(control.bytes.as_ptr().cast_mut()),
        },
        dwFlags: 0,
    };
    let mut sent = 0;
    // SAFETY: `message` and all it points to outlive this synchronous call, and dwBufferCount and
    // the control length stay within `buffers` and `control.bytes`.
    let result = unsafe {
        WSASendMsg(
            SOCKET(socket.as_raw_socket() as usize),
            &message,
            0,
            Some(&mut sent),
            None,
            None,
        )
    };
    if result == 0 {
        Ok(sent as usize)
    } else {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        Err(unsafe { WSAGetLastError() })
    }
}
/// One datagram to `peer`, from `source` when it is known. The error is Winsock's.
fn send_from(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
) -> std::result::Result<usize, WSA_ERROR> {
    let Some(source) = source else {
        return socket
            .send_to(packet, peer)
            .map_err(|e| WSA_ERROR(e.raw_os_error().unwrap_or(WSAEINVAL.0)));
    };
    let mut control = Ancillary::default();
    control.source(source);
    let buffer = WSABUF {
        len: packet.len() as u32,
        buf: windows::core::PSTR(packet.as_ptr().cast_mut()),
    };
    send_message(socket, &[buffer], &socket2::SockAddr::from(peer), &control)
}
/// Spoofed senders cannot grow a ReplySocket's table without bound; a client
/// still streaming is learned again from its next datagram.
const REPLY_SOURCES: usize = 256;
/// A UDP socket that answers each client from the local address the client's
/// datagrams arrived on, as moonlight-common-c's ENet does. Bound to every
/// address, Windows otherwise picks the source by route: on a PC where a VPN
/// such as Tailscale also routes the LAN, replies to a LAN client could leave
/// from the VPN's address, and the client drops them (issue #54).
///
/// Every send through it (`send_to`, `Batch::send`, `send_datagram`) uses
/// what `answer_from` learned for that peer. A peer Windows already routes
/// from the address it reached, as on a plain LAN, is answered as before,
/// with plain sendto and without taking the table's lock while no peer is
/// pinned.
pub struct ReplySocket {
    socket: UdpSocket,
    receive_message: unsafe extern "system" fn(
        SOCKET,
        *mut WSAMSG,
        *mut u32,
        *mut windows::Win32::System::IO::OVERLAPPED,
        LPWSAOVERLAPPED_COMPLETION_ROUTINE,
    ) -> i32,
    /// The address Windows sends to a peer from; `routed_source` but in tests.
    route: fn(SOCKET, SocketAddr) -> std::io::Result<IpAddr>,
    /// The time; `Instant::now` but in tests.
    now: fn() -> std::time::Instant,
    sources: RwLock<Sources>,
    /// Whether any peer in `sources` is pinned, so sends skip the lock
    /// while none is.
    any_pinned: AtomicBool,
    /// Whether a datagram without its local address was logged.
    missing_reported: AtomicBool,
    /// Whether a failed route lookup was logged.
    route_reported: AtomicBool,
}
/// How often a peer's route is checked again while its datagrams arrive, so
/// a VPN coming up or going down mid-stream is noticed.
const ROUTE_CHECK: std::time::Duration = std::time::Duration::from_secs(1);
/// How soon after the last check a failed send has the route checked again.
const RECHECK: std::time::Duration = std::time::Duration::from_millis(100);
/// How long an address Windows refused to send to a peer from is not pinned
/// again; a refusal that lasts costs a failed send and its retry this often.
const REFUSAL: std::time::Duration = std::time::Duration::from_secs(5);
/// What a ReplySocket learned about one peer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reached {
    /// The local address the peer's datagrams arrive on.
    local: IpAddr,
    /// Whether Windows routes replies from another address, so they must be
    /// sent from `local`.
    pinned: bool,
    /// When the route was last checked.
    checked: std::time::Instant,
    /// Whether a send to the peer failed as unreachable since.
    recheck: bool,
}
/// What a ReplySocket learned per peer, each table holding at most
/// REPLY_SOURCES entries and evicting the oldest.
#[derive(Default)]
struct Sources {
    /// What each peer last reached, and when it was learned.
    pins: HashMap<SocketAddr, (Reached, u64)>,
    /// The local addresses Windows refused to send to a peer from, and when;
    /// not pinned again for REFUSAL, as a VPN that went down may come back.
    refused: HashMap<(SocketAddr, IpAddr), (std::time::Instant, u64)>,
    /// Orders both tables' entries.
    clock: u64,
}
impl Sources {
    /// Inserts into `table`, evicting its oldest entry when full; whether
    /// `key` is new.
    fn remember<K: Copy + Eq + std::hash::Hash, V>(
        table: &mut HashMap<K, (V, u64)>,
        clock: &mut u64,
        key: K,
        value: V,
    ) -> bool {
        if table.len() >= REPLY_SOURCES
            && !table.contains_key(&key)
            && let Some(oldest) = table.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k)
        {
            table.remove(&oldest);
        }
        *clock += 1;
        table.insert(key, (value, *clock)).is_none()
    }
    /// Whether Windows refused to send to `peer` from `local` less than
    /// REFUSAL before `now`.
    fn refusing(&self, peer: SocketAddr, local: IpAddr, now: std::time::Instant) -> bool {
        self.refused
            .get(&(peer, local))
            .is_some_and(|(at, _)| now.duration_since(*at) < REFUSAL)
    }
    /// Whether a datagram from `peer` to `local` at `now` changes nothing:
    /// the same address with a fresh route, or one refused for it.
    fn settled(&self, peer: SocketAddr, local: Option<IpAddr>, now: std::time::Instant) -> bool {
        let reached = self.pins.get(&peer).map(|(reached, _)| reached);
        match local {
            Some(local) if self.refusing(peer, local, now) => reached.is_none(),
            Some(local) => reached.is_some_and(|reached| {
                let age = now.duration_since(reached.checked);
                reached.local == local && age < ROUTE_CHECK && !(reached.recheck && age >= RECHECK)
            }),
            None => reached.is_none(),
        }
    }
}
/// The errors with which Windows refuses to send from a pinned address: one
/// it no longer has (WSAEADDRNOTAVAIL, or WSAEINVAL for one it never had),
/// or one whose interface has no route to the peer (WSAENETUNREACH,
/// WSAEHOSTUNREACH), as on a Hyper-V switch or a Tailscale subnet router.
/// WSAEINVAL is also how a NIC without UDP segmentation fails.
const REFUSED: [WSA_ERROR; 4] = [WSAEINVAL, WSAEADDRNOTAVAIL, WSAENETUNREACH, WSAEHOSTUNREACH];
impl ReplySocket {
    /// Ask Windows for each datagram's local address on `socket`.
    pub fn new(socket: UdpSocket) -> std::io::Result<Self> {
        let raw = SOCKET(socket.as_raw_socket() as usize);
        let ipv6 = socket.local_addr()?.is_ipv6();
        let mut options = Vec::with_capacity(2);
        // An IPv6-only socket receives no IPv4 datagrams.
        if !ipv6 || !socket2::SockRef::from(&socket).only_v6()? {
            options.push((IPPROTO_IP, IP_PKTINFO));
        }
        if ipv6 {
            options.push((IPPROTO_IPV6, IPV6_PKTINFO));
        }
        for (level, option) in options {
            // SAFETY: the borrowed socket stays open, and the option takes this 4-byte BOOL.
            if unsafe { setsockopt(raw, level.0, option, Some(&1u32.to_ne_bytes())) }
                == SOCKET_ERROR
            {
                return Err(last_error());
            }
        }
        let mut receive_message: LPFN_WSARECVMSG = None;
        let mut returned = 0;
        // SAFETY: the input is the GUID and the output the function pointer's own storage, both
        // outliving this synchronous call on the open socket.
        if unsafe {
            WSAIoctl(
                raw,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                Some((&WSAID_WSARECVMSG as *const windows::core::GUID).cast()),
                std::mem::size_of::<windows::core::GUID>() as u32,
                Some((&mut receive_message as *mut LPFN_WSARECVMSG).cast()),
                std::mem::size_of::<LPFN_WSARECVMSG>() as u32,
                &mut returned,
                None,
                None,
            )
        } == SOCKET_ERROR
        {
            return Err(last_error());
        }
        Ok(Self {
            socket,
            receive_message: receive_message
                .ok_or_else(|| std::io::Error::other("Winsock returned no WSARecvMsg"))?,
            sources: RwLock::default(),
            route: routed_source,
            now: std::time::Instant::now,
            any_pinned: AtomicBool::new(false),
            missing_reported: AtomicBool::new(false),
            route_reported: AtomicBool::new(false),
        })
    }
    pub fn bind(address: impl ToSocketAddrs) -> std::io::Result<Self> {
        Self::new(UdpSocket::bind(address)?)
    }
    /// One datagram: its size, its sender, and the local address it arrived
    /// on. A datagram larger than `buffer` is lost with WSAEMSGSIZE, as
    /// recv_from loses it.
    pub fn receive(
        &self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        #[repr(C, align(8))]
        struct Control([u8; 128]);
        let mut control = Control([0; 128]);
        let mut data = WSABUF {
            len: buffer.len().try_into().unwrap_or(u32::MAX),
            buf: windows::core::PSTR(buffer.as_mut_ptr()),
        };
        // SAFETY: WSARecvMsg writes at most `length` bytes of sender address into the storage
        // try_init provides and the length it wrote; `data`, `control` and `message` outlive this
        // synchronous call on the open socket.
        let ((size, flags, control_len), peer) = unsafe {
            socket2::SockAddr::try_init(|storage, length| {
                let mut message = WSAMSG {
                    name: storage.cast(),
                    namelen: *length,
                    lpBuffers: &mut data,
                    dwBufferCount: 1,
                    Control: WSABUF {
                        len: control.0.len() as u32,
                        buf: windows::core::PSTR(control.0.as_mut_ptr()),
                    },
                    dwFlags: 0,
                };
                let mut size = 0;
                if (self.receive_message)(
                    SOCKET(self.socket.as_raw_socket() as usize),
                    &mut message,
                    &mut size,
                    std::ptr::null_mut(),
                    None,
                ) == SOCKET_ERROR
                {
                    return Err(last_error());
                }
                *length = message.namelen;
                Ok((size as usize, message.dwFlags, message.Control.len as usize))
            })?
        };
        if flags & MSG_PARTIAL != 0 {
            return Err(std::io::Error::from_raw_os_error(WSAEMSGSIZE.0));
        }
        let peer = peer
            .as_socket()
            .ok_or_else(|| std::io::Error::other("a datagram from a non-IP sender"))?;
        let local = local_address(&control.0[..control_len.min(control.0.len())]);
        if local.is_none() && !self.missing_reported.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                %peer,
                "Windows did not report which address a datagram arrived on; replies to it leave from the address Windows routes by"
            );
        }
        Ok((size, peer, local))
    }
    /// Answer `peer` from `local`, the address its datagram arrived on,
    /// unless Windows refused that within REFUSAL. Asks which address Windows routes
    /// to `peer` from when `local` is new, and again at most every
    /// ROUTE_CHECK, here on the receiving thread rather than on a send.
    pub fn answer_from(&self, peer: SocketAddr, local: Option<IpAddr>) {
        let now = (self.now)();
        if self.sources.read().unwrap().settled(peer, local, now) {
            return;
        }
        let local = local.filter(|local| !self.sources.read().unwrap().refusing(peer, *local, now));
        let Some(local) = local else {
            let mut sources = self.sources.write().unwrap();
            sources.pins.remove(&peer);
            self.note_pins(&sources);
            return;
        };
        let previous = self
            .sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.local == local)
            .map(|(reached, _)| reached.pinned);
        let pinned = self.routes_elsewhere(peer, local, previous);
        let mut sources = self.sources.write().unwrap();
        let Sources {
            pins,
            refused,
            clock,
        } = &mut *sources;
        refused.remove(&(peer, local));
        Sources::remember(
            pins,
            clock,
            peer,
            Reached {
                local,
                pinned,
                checked: now,
                recheck: false,
            },
        );
        self.note_pins(&sources);
    }
    /// Forget what `peer` reached, once its connection or session ended.
    pub fn forget(&self, peer: SocketAddr) {
        let mut sources = self.sources.write().unwrap();
        sources.pins.remove(&peer);
        sources.refused.retain(|(refused, _), _| *refused != peer);
        self.note_pins(&sources);
    }
    /// Records whether any peer is pinned, after `sources` changed under
    /// its write lock.
    fn note_pins(&self, sources: &Sources) {
        let any = sources.pins.values().any(|(reached, _)| reached.pinned);
        self.any_pinned.store(any, Ordering::Release);
    }
    /// The address replies to `peer` must leave from; None leaves it to
    /// Windows, which routes them from the address the peer reached or
    /// knows nothing better.
    pub fn source(&self, peer: SocketAddr) -> Option<IpAddr> {
        if !self.any_pinned.load(Ordering::Acquire) {
            return None;
        }
        self.sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.pinned)
            .map(|(reached, _)| reached.local)
    }
    /// The senders at `ip` and the address each reached, for logs.
    pub fn sources_from(&self, ip: IpAddr) -> Vec<(SocketAddr, IpAddr)> {
        let ip = ip.to_canonical();
        let mut sources: Vec<_> = self
            .sources
            .read()
            .unwrap()
            .pins
            .iter()
            .filter(|(peer, _)| peer.ip().to_canonical() == ip)
            .map(|(peer, (reached, _))| (*peer, reached.local))
            .collect();
        sources.sort();
        sources
    }
    /// One datagram to `peer`, from the address it reached.
    pub fn send_to(&self, buffer: &[u8], peer: SocketAddr) -> std::io::Result<usize> {
        let Ok((result, _)) = self.send_pinned(peer, self.source(peer), |source| {
            Ok::<_, std::convert::Infallible>(send_from(&self.socket, buffer, peer, source))
        });
        result.map_err(|error| std::io::Error::from_raw_os_error(error.0))
    }
    /// Sends with `send` from `source`, the address `peer` reached, and once
    /// more from the address Windows routes by when Windows refuses it. Only
    /// that second send succeeding shows the pin was at fault: a network
    /// that is down, or a NIC without UDP segmentation, keeps it, and the
    /// second send's error is the one the caller handles. Any other refusal
    /// has the route checked again soon (`reroute`). Returns the result and
    /// the source later sends to `peer` use.
    fn send_pinned<T, E>(
        &self,
        peer: SocketAddr,
        source: Option<IpAddr>,
        mut send: impl FnMut(Option<IpAddr>) -> Result<Result<T, WSA_ERROR>, E>,
    ) -> Result<(Result<T, WSA_ERROR>, Option<IpAddr>), E> {
        let result = send(source)?;
        let Err(error) = &result else {
            return Ok((result, source));
        };
        let error = *error;
        if !REFUSED.contains(&error) {
            return Ok((result, source));
        }
        let Some(pinned) = source else {
            self.reroute(peer);
            return Ok((result, source));
        };
        match send(None)? {
            Ok(sent) => {
                self.refuse(peer, pinned, error);
                Ok((Ok(sent), None))
            }
            Err(retry) => {
                self.reroute(peer);
                Ok((Err(retry), source))
            }
        }
    }
    /// Whether Windows routes replies to `peer` from an address other than
    /// `local`. When Windows cannot say, `previous` stands, and a new peer is
    /// answered from `local`, the address it expects.
    fn routes_elsewhere(&self, peer: SocketAddr, local: IpAddr, previous: Option<bool>) -> bool {
        match (self.route)(SOCKET(self.socket.as_raw_socket() as usize), peer) {
            Ok(routed) => routed != local,
            Err(error) => {
                if !self.route_reported.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        %peer,
                        %local,
                        %error,
                        "Windows did not say which address it routes this client by; replying from the address the client reached"
                    );
                }
                previous.unwrap_or(true)
            }
        }
    }
    /// Asks the receiving thread to check `peer`'s route again with its next
    /// datagram, at most every RECHECK, after a send to it failed, as when a
    /// VPN came up or went down. The sending thread asks Windows nothing.
    fn reroute(&self, peer: SocketAddr) {
        let flagged = |sources: &Sources| {
            sources
                .pins
                .get(&peer)
                .is_none_or(|(reached, _)| reached.recheck)
        };
        if flagged(&self.sources.read().unwrap()) {
            return;
        }
        if let Some((reached, _)) = self.sources.write().unwrap().pins.get_mut(&peer) {
            reached.recheck = true;
        }
    }
    /// Stop answering `peer` from `source`, which Windows refused, as after a
    /// VPN disconnects or when its interface has no route back; for REFUSAL,
    /// replies leave from the address Windows routes by.
    fn refuse(&self, peer: SocketAddr, source: IpAddr, error: WSA_ERROR) {
        let now = (self.now)();
        let new = {
            let mut sources = self.sources.write().unwrap();
            let new = !sources.refusing(peer, source, now);
            let Sources {
                pins,
                refused,
                clock,
            } = &mut *sources;
            if pins
                .get(&peer)
                .is_some_and(|(reached, _)| reached.local == source)
            {
                pins.remove(&peer);
            }
            Sources::remember(refused, clock, (peer, source), now);
            self.note_pins(&sources);
            new
        };
        if new {
            tracing::warn!(
                %peer,
                %source,
                code = error.0,
                "Windows refused to send from the address the client reached; replying to this client from the address Windows routes by"
            );
        }
    }
}
/// The local address Windows sends to `peer` from, by route.
fn routed_source(socket: SOCKET, peer: SocketAddr) -> std::io::Result<IpAddr> {
    let peer = socket2::SockAddr::from(peer);
    // SAFETY: Winsock writes at most the storage's length into it, and the
    // input is the peer's own sockaddr, both outliving this synchronous call
    // on the open socket.
    let ((), routed) = unsafe {
        socket2::SockAddr::try_init(|storage, length| {
            let mut returned = 0;
            if WSAIoctl(
                socket,
                SIO_ROUTING_INTERFACE_QUERY,
                Some(peer.as_ptr().cast()),
                peer.len() as u32,
                Some(storage.cast()),
                *length as u32,
                &mut returned,
                None,
                None,
            ) == SOCKET_ERROR
            {
                return Err(last_error());
            }
            *length = returned as _;
            Ok(())
        })
    }?;
    routed
        .as_socket()
        .map(|routed| routed.ip().to_canonical())
        .ok_or_else(|| std::io::Error::other("Windows returned no IP address for the route"))
}
impl std::ops::Deref for ReplySocket {
    type Target = UdpSocket;
    fn deref(&self) -> &UdpSocket {
        &self.socket
    }
}
impl AsRawSocket for ReplySocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket.as_raw_socket()
    }
}
impl AsSocket for ReplySocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.socket.as_socket()
    }
}
/// The local address in WSARecvMsg's control data: IP_PKTINFO or IPV6_PKTINFO.
fn local_address(control: &[u8]) -> Option<IpAddr> {
    let header = std::mem::size_of::<CMSGHDR>();
    let mut offset = 0;
    while offset + header <= control.len() {
        // SAFETY: the header lies within `control`, and an unaligned read needs no alignment.
        let message = unsafe {
            control
                .as_ptr()
                .add(offset)
                .cast::<CMSGHDR>()
                .read_unaligned()
        };
        if message.cmsg_len < header || offset + message.cmsg_len > control.len() {
            return None;
        }
        let data = &control[offset + header..offset + message.cmsg_len];
        if message.cmsg_level == IPPROTO_IP.0
            && message.cmsg_type == IP_PKTINFO
            && data.len() >= std::mem::size_of::<IN_PKTINFO>()
        {
            // SAFETY: `data` holds an IN_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN_PKTINFO>()
                    .read_unaligned()
                    .ipi_addr
                    .S_un
                    .S_addr
            };
            return Some(std::net::Ipv4Addr::from(address.to_ne_bytes()).into());
        }
        if message.cmsg_level == IPPROTO_IPV6.0
            && message.cmsg_type == IPV6_PKTINFO
            && data.len() >= std::mem::size_of::<IN6_PKTINFO>()
        {
            // SAFETY: `data` holds an IN6_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN6_PKTINFO>()
                    .read_unaligned()
                    .ipi6_addr
                    .u
                    .Byte
            };
            return Some(IpAddr::from(address).to_canonical());
        }
        offset += message.cmsg_len.next_multiple_of(8);
    }
    None
}
fn last_error() -> std::io::Error {
    // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
    std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0)
}
/// Windows telling this PC's IP interfaces or addresses changed: an interface came or went, a
/// parameter of one (its state among them) moved, or an address was added, removed or changed
/// (the interface notification does not report addresses). A watcher re-reads the interfaces on
/// a change instead of on a short timer, so the reading runs only when there was one.
pub struct InterfaceChanges {
    /// The registrations `CancelMibChangeNotify2` takes: interfaces, then unicast addresses.
    handles: Vec<windows::Win32::Foundation::HANDLE>,
    /// The context the OS callback signals, kept alive until after the cancel: a callback that
    /// races it finds its context valid.
    context: std::sync::Arc<Signal>,
    worker: Option<std::thread::JoinHandle<()>>,
}
/// What the callback and the worker tell each other. The callback only notes the change and
/// wakes the worker: it runs on a Windows thread, where no other work happens.
#[derive(Default)]
struct Signal {
    state: Mutex<SignalState>,
    wake: std::sync::Condvar,
}
#[derive(Default)]
struct SignalState {
    changed: bool,
    stopped: bool,
}
/// How long a change holds the window open for the ones that follow it: whatever the watcher
/// did about the first has happened by the end of it, and later changes within it are covered.
const QUIET: std::time::Duration = std::time::Duration::from_secs(2);
impl InterfaceChanges {
    /// Call `changed` when Windows reports an interface change, and `quiet` once none followed
    /// within a short window. `None` when Windows refuses the notification: the caller then
    /// falls back to its own timer.
    pub fn register(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(false, changed, quiet)
    }
    /// The registration for the test below, which also reports every interface there is now, so
    /// the callback arrives without an interface actually changing.
    #[cfg(test)]
    pub fn register_initial(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(true, changed, quiet)
    }
    fn register_with(
        initial: bool,
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        let context = std::sync::Arc::new(Signal::default());
        let pointer = std::sync::Arc::as_ptr(&context) as *const core::ffi::c_void;
        let mut interfaces = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: `context` outlives the registration, as Drop (or the cancel below) cancels it
        // before the field drops, and `interfaces` is a live out-pointer for the call.
        let registered = unsafe {
            NotifyIpInterfaceChange(
                AF_UNSPEC,
                Some(interface_changed),
                Some(pointer),
                initial,
                &mut interfaces,
            )
        };
        if registered != ERROR_SUCCESS {
            return None;
        }
        let mut addresses = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: As above, for the address registration.
        let registered = unsafe {
            NotifyUnicastIpAddressChange(
                AF_UNSPEC,
                Some(address_changed),
                Some(pointer),
                initial,
                &mut addresses,
            )
        };
        if registered != ERROR_SUCCESS {
            cancel(&[interfaces]);
            return None;
        }
        let handles = vec![interfaces, addresses];
        let signal = context.clone();
        let worker = std::thread::Builder::new()
            .name("interface-watch".into())
            .spawn(move || watch_interfaces(signal, changed, quiet));
        let Ok(worker) = worker else {
            // No callback may outlive the context it signals.
            cancel(&handles);
            return None;
        };
        Some(Self {
            handles,
            context,
            worker: Some(worker),
        })
    }
}
/// Cancel registrations; each cancel waits for a callback still running.
fn cancel(handles: &[windows::Win32::Foundation::HANDLE]) {
    for handle in handles {
        // SAFETY: each handle is a registration the Notify calls returned and nothing has
        // cancelled yet.
        unsafe {
            let _ = CancelMibChangeNotify2(*handle);
        }
    }
}
/// The interface callback: note the change, wake the worker, nothing else.
unsafe extern "system" fn interface_changed(
    context: *const core::ffi::c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: the context is the `Signal` of the live registration, valid until its cancel
    // returns, and the callback runs no longer than this function.
    unsafe { note_change(context) }
}
/// The address callback: the same as `interface_changed`, for an address row.
unsafe extern "system" fn address_changed(
    context: *const core::ffi::c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: as in `interface_changed`.
    unsafe { note_change(context) }
}
/// # Safety
/// `context` is the `Signal` of a live registration.
unsafe fn note_change(context: *const core::ffi::c_void) {
    // SAFETY: the caller's contract.
    let signal = unsafe { &*(context as *const Signal) };
    let mut state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
    state.changed = true;
    signal.wake.notify_one();
}
/// The worker: hand each change to `changed`, then wait out the window; when it closes with no
/// further change, `quiet` ends what `changed` began. The calls happen off the lock, so an
/// arriving callback never waits behind them, and the loop checks what is pending before every
/// wait, so one that arrived during a call is not slept over.
fn watch_interfaces(signal: std::sync::Arc<Signal>, changed: impl Fn(), quiet: impl Fn()) {
    let mut window: Option<std::time::Instant> = None;
    let mut state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if state.stopped {
            return;
        }
        if state.changed {
            state.changed = false;
            // A change within an open window is covered by what the first one began.
            let call = window.is_none();
            window = Some(std::time::Instant::now() + QUIET);
            drop(state);
            if call {
                changed();
            }
            state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        // Nothing pending: wait for a change, or for the window to end without one.
        let (guard, timed_out) = match window {
            Some(end) => {
                let (guard, result) = signal
                    .wake
                    .wait_timeout(
                        state,
                        end.saturating_duration_since(std::time::Instant::now()),
                    )
                    .unwrap_or_else(PoisonError::into_inner);
                (guard, result.timed_out())
            }
            None => (
                signal
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                false,
            ),
        };
        state = guard;
        if state.stopped {
            return;
        }
        if state.changed {
            continue;
        }
        if timed_out {
            window = None;
            drop(state);
            quiet();
            state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
        }
    }
}
impl Drop for InterfaceChanges {
    fn drop(&mut self) {
        // Each cancel waits for a callback still running and Windows calls no more once it
        // returns, so the context below stays valid for every callback there may be.
        cancel(&self.handles);
        {
            let mut state = self
                .context
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.stopped = true;
        }
        self.context.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests;
