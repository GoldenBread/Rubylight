//! Networking for a Linux host: UDP sockets that answer each client from the address it
//! reached, batched sends with UDP segmentation, this PC's interfaces, routes and gateways, and
//! notice of their changes. Not available yet: QoS marks on a stream's datagrams.

mod batch;
mod changes;
mod interfaces;
mod netlink;
mod reply;
mod route;
mod segments;
mod socket;
mod sys;
mod wait;

pub use batch::{
    Batch, DatagramWait, send_datagram, send_datagram_pass, send_datagram_with_wait,
};
pub use changes::InterfaceChanges;
pub use interfaces::{
    LanInterface, default_gateways, host_name, lan_addresses, lan_interfaces, local_mac,
    unicast_addresses,
};
pub use reply::ReplySocket;
pub use route::{Link, routed_link, routed_link_bps, routed_mtu, routed_same_link};
pub use segments::{Segments, Sent};
pub use socket::{
    QosFlow, SEND_BUFFER_DEFAULT, configure_udp, datagram_lost, set_dont_fragment, wait_readable,
    wait_readable_or_wake, warm_up_qos,
};
pub use wait::{AUDIO_ENOBUFS_WAIT, DatagramPass, WRITABLE_WAIT, WritableWaits};

#[cfg(test)]
mod tests;
