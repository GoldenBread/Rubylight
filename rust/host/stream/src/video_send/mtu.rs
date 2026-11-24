//! Video socket accounting (`reached_socket`) and path-MTU warnings for jumbo streams.

use crate::state::Session;

/// What of a frame reached the socket, for the link-rate controller: the
/// `offered` packets less the `refused` ones (the batch's drops while the
/// frame was sent), and the `bytes` the socket took with each datagram's
/// framing. A frame abandoned part way counts only what left.
pub(crate) fn reached_socket(
    offered: usize,
    refused: u64,
    bytes: usize,
    ipv6: bool,
) -> (usize, u64) {
    let packets = offered.saturating_sub(refused as usize);
    let wire = bytes + packets * butterpollo_media_policy::network_pacing::overhead(ipv6);
    (packets, wire as u64)
}

/// A stream warning when the path refused jumbo datagrams for their size with
/// fragmentation refused: a hop carries less than the host's MTU, which the
/// client's probe should have caught, so name the path rather than only the
/// socket error.
pub(crate) fn oversized_warning(s: &Session) {
    s.launch.warnings.event(
        "network_mtu",
        "Video packets larger than the network path's MTU were refused and dropped. A network adapter, switch or the device's link does not carry jumbo frames; lower the device's video packet size or make every hop's MTU carry the offered packet size plus 64 bytes of headers.",
        butterpollo_session::session::EVENT_PERIOD,
    );
}

/// [`oversized_warning`] when `batch` refused oversized datagrams since `before`.
pub(crate) fn report_oversized(s: &Session, batch: &butterpollo_win_io::net::Batch, before: u64) {
    if batch.oversized != before {
        oversized_warning(s);
    }
}

/// Whether `mtu` carries a video packet of `packet_size` bytes plus the 64
/// bytes of RTP, UDP and IPv6 header the packet-size offer reserves.
pub(super) fn mtu_carries(mtu: u32, packet_size: usize) -> bool {
    u64::from(mtu) >= packet_size as u64 + 64
}

/// The `network_mtu` warning for a jumbo stream whose route shrank after its
/// offer: packets now fragment or drop silently, since the shared socket does
/// not refuse fragmentation. The MTU is this PC's interface on the route to
/// the client, all the host can see.
pub(super) fn mtu_warning(mtu: u32, packet_size: usize) -> String {
    format!(
        "The MTU of this PC's network interface on the route to the client is now {mtu} bytes; this stream's {packet_size}-byte video packets plus 64 bytes of headers do not fit. Lower the device's video packet size, or use an interface on this PC whose MTU carries the packet size plus 64 bytes of headers."
    )
}

/// The 2 s route check's MTU check for a session, from the watcher's cached
/// interface MTU: the paced sender makes no network lookup. Only this stream
/// is warned; the shared video socket stays untouched. A stock packet size
/// fits every path the stream offered, so only jumbo sizes are checked.
pub(crate) fn check_route_mtu(s: &Session, packet_size: usize, mtu: Option<u32>) {
    if packet_size <= usize::from(rubylight_protocol::video_packet::STOCK_MAX_VIDEO_PACKET) {
        return;
    }
    let Some(mtu) = mtu else {
        return;
    };
    if !mtu_carries(mtu, packet_size) {
        s.launch.warnings.event(
            "network_mtu",
            mtu_warning(mtu, packet_size),
            butterpollo_session::session::EVENT_PERIOD,
        );
    }
}
