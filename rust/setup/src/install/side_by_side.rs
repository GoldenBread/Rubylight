//! Side-by-side port allocation, profile creation and host health probing.
use super::profile::profile_empty;
use crate::{detect, host_service, log::line};
use anyhow::{Context, Result, bail};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The base ports a side-by-side installation tries, in turn. They stay clear
/// of the default 47984-48010 other hosts use and of the Windows ephemeral
/// range above 49151, and of each other's ranges.
pub(super) const SIDE_BY_SIDE_PORTS: [u16; 6] = [48589, 48689, 48789, 48889, 48989, 49089];
/// `webrtc_port`'s offset from the base port; `webrtc_media_port` and the
/// guests' four media ports follow it.
pub(super) const WEBRTC_OFFSET: u16 = 30;
/// Every port a base can use: `Ports::from_base`'s, then WebRTC's.
pub(super) fn ports_of(base: u16) -> std::ops::RangeInclusive<u16> {
    base.saturating_sub(5)..=base.saturating_add(WEBRTC_OFFSET + 5)
}
/// Whether no program holds `port`, on TCP or UDP. The host binds the same
/// way, so what is free here is free for it: another program's bind, a Windows
/// excluded range or another host stopped for the moment but holding the port.
pub(super) fn port_free(port: u16) -> bool {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
    let v4 = [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST];
    let free = v4.into_iter().map(IpAddr::V4).all(|address| {
        TcpListener::bind((address, port)).is_ok() && UdpSocket::bind((address, port)).is_ok()
    });
    // A PC without IPv6 has nothing there to collide with.
    let v6 = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let held = |error: std::io::Error| matches!(error.kind(), AddrInUse | PermissionDenied);
    free && !TcpListener::bind((v6, port)).err().is_some_and(held)
        && !UdpSocket::bind((v6, port)).err().is_some_and(held)
}
/// The first base port whose whole range is free.
pub(super) fn pick_base_port(free: impl Fn(u16) -> bool) -> Result<u16> {
    SIDE_BY_SIDE_PORTS
        .into_iter()
        .find(|&base| ports_of(base).all(&free))
        .with_context(|| {
            format!(
                "no free ports for Rubylight beside another host: every range from {} to {} is in use. Close what uses them, then run setup again. Nothing has been changed.",
                ports_of(SIDE_BY_SIDE_PORTS[0]).start(),
                ports_of(SIDE_BY_SIDE_PORTS[5]).end(),
            )
        })
}
/// The new profile's settings: its ports, and a name of its own for clients,
/// which would otherwise show two hosts with the PC's name.
pub(super) fn side_by_side_conf(base: u16, pc: Option<&str>) -> String {
    let name = match pc.map(str::trim).filter(|pc| !pc.is_empty()) {
        Some(pc) => format!("{pc} (Rubylight)"),
        None => "Rubylight".to_owned(),
    };
    format!(
        "port = {base}\nwebrtc_port = {}\nwebrtc_media_port = {}\nsunshine_name = {name}\n",
        base + WEBRTC_OFFSET,
        base + WEBRTC_OFFSET + 1
    )
}
/// Marks a profile that side by side made, so its identity was never imported
/// from another host: it is kept when Rubylight is installed side by side again
/// after an uninstall that kept its settings.
pub(crate) const SIDE_BY_SIDE_MARK: &str = "side-by-side";
/// Readies the profile of a side-by-side installation, which never takes
/// settings or an identity from another host. An installed Rubylight's profile
/// and one an earlier side-by-side installation left stay; a profile from
/// before moves aside unchanged. Otherwise the profile is new, with its own
/// base port. Returns whether this run made it; `moved` is where the earlier
/// profile went.
pub(super) fn prepare_side_by_side_profile(
    profile: &Path,
    installed: bool,
    pc: Option<&str>,
    free: impl Fn(u16) -> bool,
    moved: &mut Option<PathBuf>,
) -> Result<bool> {
    let empty = profile_empty(profile)?;
    if !empty && (installed || profile.join(SIDE_BY_SIDE_MARK).is_file()) {
        if !installed {
            // Its port was free when it was chosen; nothing has held it since.
            let base = web_port(profile) - 1;
            if !ports_of(base).all(&free) {
                bail!(
                    "Rubylight's kept settings in {} use base port {base}, and a port of its range is in use now. Close what uses it, or set another port in sunshine.conf there, then run setup again. Nothing has been changed.",
                    profile.display()
                );
            }
        }
        return Ok(false);
    }
    let base = pick_base_port(free)?;
    if !empty {
        *moved = Some(move_profile_aside(profile)?);
    }
    std::fs::create_dir_all(profile)?;
    std::fs::write(profile.join("sunshine.conf"), side_by_side_conf(base, pc))?;
    std::fs::write(profile.join(SIDE_BY_SIDE_MARK), b"")?;
    Ok(true)
}
pub(super) fn earlier_settings_moved(aside: &Path) -> String {
    format!(
        "Side by side starts with new settings, so setup moved the earlier Rubylight settings to {}.",
        aside.display()
    )
}
/// `profile` under the next free name beside it, unchanged.
pub(super) fn move_profile_aside(profile: &Path) -> Result<PathBuf> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = profile.file_name().context("profile has no name")?;
    let aside = (0..100)
        .map(|n| profile.with_file_name(format!("{}.previous-{seconds}-{n}", name.display())))
        .find(|aside| std::fs::symlink_metadata(aside).is_err())
        .context("no free name to move the earlier settings to")?;
    std::fs::rename(profile, &aside)
        .with_context(|| format!("moving {} to {}", profile.display(), aside.display()))?;
    Ok(aside)
}
/// What the finished window adds for a side-by-side installation: clients
/// must be given the port, and may otherwise reach the other host.
pub fn side_by_side_result(web_port: u16, address: &str, hosts: &[String]) -> String {
    let beside = if hosts.is_empty() {
        String::new()
    } else {
        format!(" beside {}", detect::in_words(hosts))
    };
    format!(
        "Rubylight runs{beside} as the {} service, with its own settings. In Moonlight or Nightjar, add this PC as {address}:{}: without the port, a client reaches whatever host uses the default one.",
        host_service::SIDE_BY_SIDE,
        web_port - 1
    )
}
/// This PC's address on its default route, else its name. Connecting a UDP
/// socket sends nothing; it only picks the route.
pub fn pc_address() -> String {
    let routed = std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("192.0.2.1", 9))?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback());
    match routed {
        Some(ip) => ip.to_string(),
        None => pc_name().unwrap_or_else(|| "this PC's address".to_owned()),
    }
}
pub(super) fn pc_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
}
/// The text of a profile's sunshine.conf, empty if it cannot be read. The
/// host also reads one saved with a byte order mark or as UTF-16.
pub(super) fn conf_text(profile: &Path) -> String {
    let bytes = std::fs::read(profile.join("sunshine.conf")).unwrap_or_default();
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| unit(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    String::from_utf8_lossy(text).into_owned()
}
/// The configured base port of a profile (47989 unless set).
pub fn web_port(profile: &Path) -> u16 {
    let base = conf_text(profile)
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "port")
                .then(|| {
                    value
                        .split('#')
                        .next()?
                        .trim()
                        .trim_matches('"')
                        .parse::<u16>()
                        .ok()
                })
                .flatten()
        })
        .filter(|port| (1029..=65514).contains(port))
        .unwrap_or(47989);
    base + 1
}
/// Where the host answers serverinfo: its bind_address, or this PC when
/// that is blank, all interfaces or not an address (the host then listens
/// on all of them or only on this PC). The first value that is not empty
/// counts, as the host reads it.
pub(super) fn probe_address(conf: &str) -> IpAddr {
    conf.lines()
        .find_map(|line| {
            let (key, value) = line.split('#').next()?.split_once('=')?;
            let value = value.trim().trim_matches('"').trim();
            (key.trim() == "bind_address" && !value.is_empty()).then_some(value)
        })
        .and_then(|value| value.parse::<IpAddr>().ok())
        .filter(|address| !address.is_unspecified())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}
/// The host's serverinfo address for a profile.
pub(crate) fn probe(profile: &Path) -> SocketAddr {
    SocketAddr::new(probe_address(&conf_text(profile)), web_port(profile) - 1)
}

/// Whether the host at `profile`'s address reports a stream, a queued
/// launch or a running app.
pub(crate) fn streaming(profile: &Path) -> bool {
    serverinfo(probe(profile)).is_ok_and(|response| check_idle(&response).is_err())
}
/// Refuse to stop a host that reports a stream, a queued launch or a running
/// app, unless the user chose to end them (`end_streams`). A host that cannot
/// say is stopped as before: none is running (Windows refuses a closed
/// loopback port only after about two seconds), it is Vibepollo, Apollo or
/// Sunshine, or it was asked on a LAN bind_address, where a Rust host leaves
/// its counts blank.
pub(crate) fn ensure_idle(address: SocketAddr, end_streams: bool) -> Result<()> {
    match serverinfo(address) {
        Ok(response) => match check_idle(&response) {
            Err(_) if end_streams => {
                line("the host is streaming; ending the stream as asked");
                Ok(())
            }
            result => result,
        },
        Err(error) => {
            line(format!("no host status at {address} ({error}); continuing"));
            Ok(())
        }
    }
}
pub(super) fn serverinfo(address: SocketAddr) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        socket,
        "GET /serverinfo HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    // A reset or timeout after the answer arrived still leaves the counts.
    let mut response = Vec::new();
    let read = socket.take(65536).read_to_end(&mut response);
    if response.is_empty() {
        read?;
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
}

pub(super) const IDLE_FIELDS: [&str; 3] = [
    "RustHostSessionCount",
    "RustHostPendingSessionCount",
    "RustHostApplicationActive",
];
pub(super) fn host_count(response: &str, field: &str) -> Option<u32> {
    response
        .split_once(&format!("<{field}>"))
        .and_then(|(_, value)| value.split_once(&format!("</{field}>")))
        .and_then(|(value, _)| value.trim().parse::<u32>().ok())
}
pub(super) fn check_idle(response: &str) -> Result<()> {
    for field in IDLE_FIELDS {
        if host_count(response, field).is_some_and(|count| count != 0) {
            bail!(
                "Disconnect all streams and close games started from Moonlight before installing, then run setup again. Nothing has been changed."
            );
        }
    }
    Ok(())
}
/// The repair's idle check: proceeds only on a positive idle answer, every
/// count present and 0. A host that does not answer, or answers with blank
/// counts (it blanks them for a caller that is not on loopback), may be
/// streaming and counts as busy. Only a host process that is not running at
/// all leaves nothing to interrupt. `host_running` says whether one is.
/// `blank_ok` is for a person who asked (`--now`): a host on a LAN
/// bind_address blanks its counts for every caller, this PC included, and
/// would otherwise never be repairable. Counts that are there still count.
pub(super) fn ensure_idle_positive(
    answer: &std::io::Result<String>,
    host_running: impl FnOnce() -> bool,
    blank_ok: bool,
) -> Result<()> {
    match answer {
        Ok(response) => {
            check_idle(response)?;
            if !blank_ok
                && IDLE_FIELDS
                    .iter()
                    .any(|field| host_count(response, field).is_none())
            {
                bail!("the host does not report whether it is streaming");
            }
            Ok(())
        }
        Err(error) if host_running() => {
            bail!("the host is running but does not answer ({error})")
        }
        Err(_) => Ok(()),
    }
}
/// Wait until the host answers serverinfo at `address` as a Rust host.
pub(crate) fn wait_ready(address: SocketAddr, version: Option<&str>) -> Result<()> {
    use std::io::{Read, Write};
    let host = match address {
        SocketAddr::V4(address) => address.ip().to_string(),
        SocketAddr::V6(address) => format!("[{}]", address.ip()),
    };
    let request = format!("GET /serverinfo HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Ok(mut socket) =
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))
        {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(20)));
            let _ = socket.write_all(request.as_bytes());
            let mut response = String::new();
            let _ = socket.read_to_string(&mut response);
            if version.map_or_else(
                || response.contains("<RustHostVersion>"),
                |version| {
                    response.contains(&format!("<RustHostVersion>{version}</RustHostVersion>"))
                },
            ) {
                line("the host is answering");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("Rubylight did not start within 90 seconds; see logs\\service.log in the profile")
}
