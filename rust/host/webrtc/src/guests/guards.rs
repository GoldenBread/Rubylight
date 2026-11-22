//! RAII drop guards for guest stream seats, sockets, and owner stream presence.
use super::{Guests, Owner};
use butterpollo_stream::state::Shared;
use std::{net::IpAddr, sync::Arc};

/// The owner's stream while it runs, for guests to join; dropping it ends every guest's.
pub struct OwnerGuard {
    guests: Arc<Guests>,
    generation: u64,
}

impl OwnerGuard {
    pub fn new(h: &Shared, owner: Owner) -> Self {
        let guests = crate::WebRtcRuntime::for_host(h).guests.clone();
        Self {
            generation: guests.owner_started(owner),
            guests,
        }
    }
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        self.guests.owner_ended(self.generation);
    }
}

/// A guest socket counted until it joins (`MAX_SOCKETS`).
pub(super) struct SocketSlot {
    pub(super) guests: Arc<Guests>,
    pub(super) peer: IpAddr,
    pub(super) counted: bool,
}

impl SocketSlot {
    pub(super) fn joined(&mut self) {
        if std::mem::replace(&mut self.counted, false) {
            self.guests.close_socket(self.peer);
        }
    }
}

impl Drop for SocketSlot {
    fn drop(&mut self) {
        self.joined();
    }
}

/// A joined guest, listed until its stream ends.
pub(super) struct Seat {
    pub(super) guests: Arc<Guests>,
    pub(super) id: u64,
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.guests.left(self.id);
    }
}
