//! Avoid stale UDP ICMP errors aborting a subsequent streaming client.

mod batch;
mod changes;
mod datagram;
mod interfaces;
mod qos;
mod reply;

pub use batch::{Batch, Segments, Sent};
pub use changes::InterfaceChanges;
pub use datagram::{
    AUDIO_ENOBUFS_WAIT, DatagramPass, DatagramWait, SEND_BUFFER_DEFAULT, WRITABLE_WAIT,
    WritableWaits, configure_udp, datagram_lost, send_datagram, send_datagram_pass,
    send_datagram_with_wait, set_dont_fragment, wait_readable, wait_readable_or_wake,
};
pub use interfaces::{
    LanInterface, Link, default_gateways, host_name, hyper_v_adapter, lan_addresses,
    lan_interfaces, local_mac, routed_link, routed_link_bps, routed_mtu, routed_same_link,
    tcp_owner, unicast_addresses,
};
pub use qos::{QosFlow, warm_up_qos};
pub use reply::ReplySocket;

#[cfg(test)]
mod tests;
