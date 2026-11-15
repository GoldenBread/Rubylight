//! Video packet sizes above Moonlight's 1400 bytes, for paths with jumbo frames.
//!
//! A host lists the largest video packet it accepts in the serverinfo element [`SERVERINFO_FIELD`]
//! and, per stream, in the DESCRIBE attribute [`DESCRIBE_ATTRIBUTE`]. A client then announces its
//! size in Moonlight's [`ANNOUNCE_ATTRIBUTE`] and may exceed [`STOCK_MAX_VIDEO_PACKET`] only up to
//! what was offered. Without the offer, 1400 stays the most any client may ask for, and stock
//! clients keep what they send.

/// The largest video packet this version defines: 8192 bytes, a multiple of 16, which leaves
/// about 740 bytes of a 9000-byte MTU for encapsulation and for "9K" adapters that carry less.
pub const MAX_VIDEO_PACKET: u16 = 8192;
/// What stock Moonlight sends and older hosts accept; a size above it needs the offer.
pub const STOCK_MAX_VIDEO_PACKET: u16 = 1400;
/// Bytes a path's MTU must carry beyond the packet size: the 16-byte RTP header, the 8-byte UDP
/// header and 40 bytes of IPv6, the largest network header. The packet size itself covers the
/// client's 32-byte encryption header.
pub const DATAGRAM_OVERHEAD: u16 = 64;

/// Serverinfo element: the largest video packet this host accepts, in decimal bytes.
pub const SERVERINFO_FIELD: &str = "MaxVideoPacket";
/// DESCRIBE attribute: the largest video packet this stream's path carries.
pub const DESCRIBE_ATTRIBUTE: &str = "x-rl-max-packet-size";
/// ANNOUNCE attribute: Moonlight's video packet size, which a client may raise to the offer.
pub const ANNOUNCE_ATTRIBUTE: &str = "x-nv-video[0].packetSize";

/// The largest video packet a path whose interface MTU is `mtu` bytes offers, or 0 when that is no
/// more than stock clients already send (1464 bytes or less). A 1500-byte path, Wi-Fi among them,
/// offers 1424: 24 bytes above stock, safe but with little of jumbo's benefit. The size covers the
/// client's 32-byte encryption header; the datagram adds [`DATAGRAM_OVERHEAD`] bytes on top, and the
/// client's UDP probe must use this size (the packet size less the 32-byte header when encrypted),
/// not the config value, because the probe adds its own framing. Rounded down to a multiple of 16,
/// as Moonlight rounds its packet size.
pub fn for_mtu(mtu: u32) -> u16 {
    let room = mtu
        .saturating_sub(u32::from(DATAGRAM_OVERHEAD))
        .min(u32::from(MAX_VIDEO_PACKET));
    let packet = (room & !0xF) as u16;
    if packet > STOCK_MAX_VIDEO_PACKET {
        packet
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_packet_is_8192_and_a_multiple_of_16() {
        assert_eq!(MAX_VIDEO_PACKET, 8192);
        assert_eq!(MAX_VIDEO_PACKET % 16, 0);
        assert!(MAX_VIDEO_PACKET > STOCK_MAX_VIDEO_PACKET);
        // Stock Moonlight's limit is not itself a multiple of 16: it rounds the
        // size it asks for down to one.
        assert!(STOCK_MAX_VIDEO_PACKET >= 1392);
    }

    #[test]
    fn a_path_offers_its_mtu_less_the_datagram_overhead_rounded_down() {
        // 9000 is capped: MTU - 64 is 8936, and 8192 is the most this version defines.
        assert_eq!(for_mtu(9000), MAX_VIDEO_PACKET);
        assert_eq!(for_mtu(u32::MAX), MAX_VIDEO_PACKET);
        // A 9000-byte path just reaches the cap; 16 bytes less does not.
        assert_eq!(for_mtu(8256), MAX_VIDEO_PACKET);
        assert_eq!(for_mtu(8255), 8176);
        // Ethernet and Wi-Fi (1500) offer 1424, not the 1392 stock clients send.
        assert_eq!(for_mtu(1500), 1424);
        assert_eq!(for_mtu(1499), 1424);
        assert_eq!(for_mtu(1488), 1424);
        assert_eq!(for_mtu(1487), 1408);
        // At or below stock there is nothing to offer.
        assert_eq!(for_mtu(1464), 0);
        assert_eq!(for_mtu(1424), 0);
        assert_eq!(for_mtu(64), 0);
        assert_eq!(for_mtu(0), 0);
        // Every offer is a multiple of 16 and fits a u16.
        for mtu in (0..=10_000).chain([u32::MAX]) {
            let packet = for_mtu(mtu);
            assert_eq!(packet % 16, 0, "{mtu}");
            assert!(packet == 0 || packet > STOCK_MAX_VIDEO_PACKET, "{mtu}");
            assert!(packet <= MAX_VIDEO_PACKET, "{mtu}");
        }
    }
}
