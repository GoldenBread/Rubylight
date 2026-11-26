//! The launch curtain's state (0x5548) on the control stream: to a stream whose launch asked
//! with `rlLaunchCurtain=1` (`extensions::launch_curtain_stream`), reliable, whenever it changed
//! (`crate::launch_curtain`), starting with the state when the peer first polls. A peer that sent
//! a lifted state is done; the loop's cost is then one atomic read, and never a wait.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;
use rubylight_protocol::launch_curtain::LAUNCH_CURTAIN_STATE_MESSAGE_TYPE;

/// One peer's curtain: whether its stream asked, the tracker's revision and the sequence it
/// last sent, and whether it sent a lifted state.
#[derive(Default)]
pub(in crate::stream) struct PeerCurtain {
    checked: bool,
    offered: bool,
    revision: Option<u64>,
    sent: Option<u32>,
    done: bool,
}

/// The state when it changed since this peer last sent it.
pub(super) fn send(
    h: &Shared,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) {
    if p.launch_curtain.done {
        return;
    }
    if !p.launch_curtain.checked {
        p.launch_curtain.checked = true;
        p.launch_curtain.offered = p
            .config(h, s)
            .is_ok_and(|config| crate::extensions::launch_curtain_stream(&config, &s.launch));
    }
    if !p.launch_curtain.offered {
        p.launch_curtain.done = true;
        return;
    }
    let revision = h.curtain.revision();
    if p.launch_curtain.revision == Some(revision) {
        return;
    }
    p.launch_curtain.revision = Some(revision);
    let state = h.curtain.state_for(&s.launch.id);
    if p.launch_curtain.sent == Some(state.sequence) {
        return;
    }
    let Some(frame) = state.encode() else {
        tracing::warn!("launch curtain state does not fit its frame");
        return;
    };
    match p.encrypt(s, LAUNCH_CURTAIN_STATE_MESSAGE_TYPE, &frame) {
        Ok(message) => {
            if let Err(e) = host
                .peer_mut(peer_id)
                .send(0, &Packet::new(message, PacketKind::Reliable))
            {
                tracing::warn!(error = %e, "launch curtain state not sent");
                return;
            }
        }
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "launch curtain state not encrypted");
            return;
        }
    }
    p.launch_curtain.sent = Some(state.sequence);
    p.launch_curtain.done = state.phase.lifts();
}
