//! QoS, transport feedback, clock sync, HDR metadata, host timings, pipeline rates, and
//! congestion/link-rate control loop ticks.

use super::super::*;
use super::haptics::FEEDBACK_CHANNEL;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

/// Dispatches QoS, recovery, timing, and feedback control packets. Returns `true` if `kind`
/// was handled.
pub(super) fn handle_qos_packet(
    peer: &rusty_enet::Peer<ControlSocket>,
    p: &ControlPeer,
    s: &Arc<Session>,
    kind: u16,
    payload: &[u8],
    clock_syncs: &mut Vec<(PeerID, u16, Instant, Vec<u8>)>,
) -> bool {
    match kind {
        0x0200 => s.record_control_ping(Instant::now(), payload),
        // SS_RFI_REQUEST: first frame, a reserved word, last
        // frame, reserved words; older clients sent two
        // 64-bit indices, whose low words sit at the same
        // offsets. Requiring 8 bytes turned every request
        // into a keyframe.
        0x0301 if payload.len() >= 12 => {
            s.request_invalidation(
                u64::from(u32::from_le_bytes(payload[..4].try_into().unwrap())),
                u64::from(u32::from_le_bytes(payload[8..12].try_into().unwrap())),
            );
        }
        0x0301 => s.request_invalidation(0, 0),
        0x0302 => s.request_client_keyframe(),
        // This dispatch receives only client-to-host packets;
        // 0x5502 in the other direction is controller feedback.
        0x5502 => s.record_fec_status(payload),
        // Per-packet arrival feedback, unreliable and dropped when invalid.
        butterpollo_media_policy::congestion::TRANSPORT_FEEDBACK_MESSAGE_TYPE => {
            if !s.record_transport_feedback(payload) {
                tracing::trace!(bytes = payload.len(), "transport feedback ignored");
            }
        }
        butterpollo_media_policy::statistical_fec::FEC_REPORT_MESSAGE_TYPE => {
            match s.record_fec_report(payload) {
                Ok((report, used)) => tracing::debug!(
                    seq = report.seq,
                    data = report.data_shards,
                    parity = report.parity_shards,
                    missing = report.missing_shards,
                    unrecovered = report.unrecovered_blocks,
                    interval_ms = report.interval_ms,
                    used,
                    "fec report"
                ),
                Err(error) => {
                    tracing::debug!(%error, "fec report refused")
                }
            }
        }
        butterpollo_media_policy::phase_sync::REPORT_MESSAGE_TYPE => {
            match s
                .phase_sync
                .lock()
                .unwrap()
                .on_payload(Instant::now(), payload)
            {
                Some(report) => tracing::debug!(
                    lead_ns = report.lead_ns,
                    spread_ns = report.spread_ns,
                    period_ns = report.period_ns,
                    frames = report.frames,
                    "phase lock report"
                ),
                None => tracing::debug!(len = payload.len(), "invalid phase lock report"),
            }
        }
        // Deferred until the socket is unheld so `host_send_us` matches actual send time.
        butterpollo_media_policy::clock_sync::CLOCK_SYNC_MESSAGE_TYPE => {
            clock_syncs.push((peer.id(), kind, p.seen, payload.to_vec()));
        }
        butterpollo_media_policy::host_timing::HOST_TIMING_MESSAGE_TYPE => {
            match s.host_timings.on_request(payload) {
                Some(enabled) => tracing::info!(
                    client = %s.launch.client.name,
                    enabled,
                    "host timing reports"
                ),
                None => tracing::debug!(len = payload.len(), "invalid host timing request"),
            }
        }
        butterpollo_media_policy::link_notice::LINK_NOTICE_MESSAGE_TYPE => {
            match s.link_notices.on_request(payload) {
                Some(enabled) => tracing::info!(
                    client = %s.launch.client.name,
                    enabled,
                    "link notices"
                ),
                None => tracing::debug!(len = payload.len(), "invalid link notice request"),
            }
        }
        butterpollo_media_policy::pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE => {
            let counts = s.pipeline_counts();
            match s.pipeline_rates.on_request(Instant::now(), payload, counts) {
                Some(enabled) => tracing::info!(
                    client = %s.launch.client.name,
                    enabled,
                    "pipeline rate reports"
                ),
                None => tracing::debug!(len = payload.len(), "invalid pipeline rates request"),
            }
        }
        _ => return false,
    }
    true
}

/// Sends deferred clock-sync replies after the control socket is unheld so `host_send_us`
/// matches the actual send time.
pub(super) fn send_clock_syncs(
    host: &mut Host<ControlSocket>,
    peers: &mut HashMap<PeerID, ControlPeer>,
    clock_syncs: &mut Vec<(PeerID, u16, Instant, Vec<u8>)>,
) {
    for (peer_id, kind, received_at, payload) in clock_syncs.drain(..) {
        let Some(p) = peers.get_mut(&peer_id) else {
            continue;
        };
        let Some(s) = p.session.clone().filter(|s| !s.stopping()) else {
            continue;
        };
        let epoch = *s.media_epoch.read().unwrap();
        if let Some(reply) =
            butterpollo_media_policy::clock_sync::reply(epoch, received_at, &payload)
            && let Ok(message) = p.encrypt(&s, kind, &reply)
        {
            let _ = host.peer_mut(peer_id).send(
                FEEDBACK_CHANNEL,
                &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
            );
        }
    }
}

/// Sends queued HDR metadata, timing/rate reports, and ticks adaptive FEC, pacing, link
/// rate, congestion control, and link notices for `peer_id`.
pub(super) fn tick_and_send_qos(
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    poll_feedback: bool,
) {
    // Sent once the encoder has the display's metadata, as
    // Sunshine does, and again only if it changes. Moonlight
    // for Xbox sets the TV's HDMI mode on every message, so an
    // earlier one with placeholder values switched it twice
    // (issue #11). The revision is read first, so a write that
    // lands after this pass leaves the number moved and the
    // next pass re-reads.
    let revision = s.hdr_revision.load(Ordering::Acquire);
    if revision != p.hdr_seen {
        let metadata = s
            .hdr_metadata
            .read()
            .unwrap()
            .map(|m| m.wire(s.hdr_active()));
        match metadata {
            // Nothing to send, or the client has it: this revision is done.
            None => p.hdr_seen = revision,
            Some(metadata) if p.hdr_metadata == Some(metadata) => {
                p.hdr_seen = revision;
            }
            // Seen only once it was sent: a failed one is retried next pass.
            Some(metadata) => {
                if let Ok(message) = p.encrypt(s, 0x010e, &metadata)
                    && host
                        .peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))
                        .is_ok()
                {
                    p.hdr_metadata = Some(metadata);
                    p.hdr_seen = revision;
                }
            }
        }
    }
    // Present-to-sent reports for a client that asked (0x5536).
    // Unreliable: a lost one costs the client those samples only.
    if s.host_timings.queued() {
        while let Some(report) = s.host_timings.take_report() {
            if let Ok(message) = p.encrypt(
                s,
                butterpollo_media_policy::host_timing::HOST_TIMING_MESSAGE_TYPE,
                &report,
            ) {
                let _ = host.peer_mut(peer_id).send(
                    0,
                    &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
                );
            }
        }
    }
    // Pipeline frame rates for a client that asked (0x553D); unreliable
    // like the timing reports, as the next one replaces a lost one.
    if s.pipeline_rates.queued() {
        let rates = s
            .pipeline_rates
            .take(Instant::now(), || s.pipeline_counts());
        if let Some(report) = rates
            && let Ok(message) = p.encrypt(
                s,
                butterpollo_media_policy::pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE,
                &report,
            )
        {
            let _ = host.peer_mut(peer_id).send(
                0,
                &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
            );
        }
    }
    if poll_feedback && let Some((old, new)) = s.tick_fec(Instant::now()) {
        tracing::info!(
            client = %s.launch.client.name,
            old_percent = old,
            new_percent = new,
            "adaptive fec"
        );
    }
    if poll_feedback
        && let Some(step) =
            s.tick_automatic_pacing_step(Instant::now(), host.peer(peer_id).round_trip_time())
    {
        if step.reason_changed {
            tracing::info!(
                client = %s.launch.client.name,
                old_bps = step.old_bps,
                new_bps = step.new_bps,
                reason = step.reason.as_str(),
                "automatic video burst pacing; encoder bitrate unchanged"
            );
        } else {
            tracing::debug!(
                client = %s.launch.client.name,
                old_bps = step.old_bps,
                new_bps = step.new_bps,
                reason = step.reason.as_str(),
                "automatic video burst pacing; encoder bitrate unchanged"
            );
        }
    }
    if poll_feedback {
        s.tick_link_rate(Instant::now(), host.peer(peer_id).round_trip_time());
        let (change, applied) =
            s.tick_congestion(Instant::now(), host.peer(peer_id).round_trip_time());
        if let Some(change) = change {
            log_congestion_change(s, &change, applied);
        }
    }
    // The bitrate notice for a client that asked (0x5538): reliable, so
    // the overlay is not left stale, and at most one a second.
    if s.link_notices.queued() {
        let notice = s.link_notices.take(Instant::now());
        if let Some(notice) = notice {
            match p.encrypt(
                s,
                butterpollo_media_policy::link_notice::LINK_NOTICE_MESSAGE_TYPE,
                &notice,
            ) {
                Ok(message) => {
                    if let Err(e) = host
                        .peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))
                    {
                        tracing::warn!(error = %e, "link notice not sent");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "link notice not encrypted");
                }
            }
        }
    }
}

/// A congestion control decision, for the log: a cut is a warning, a release information.
fn log_congestion_change(
    s: &Session,
    change: &butterpollo_media_policy::congestion::Change,
    applied: Option<(u32, u32)>,
) {
    use butterpollo_media_policy::congestion::ChangeKind;
    let (old_kbps, new_kbps) = applied.unwrap_or((0, 0));
    let shadow = s.congestion.lock().unwrap().shadow();
    macro_rules! line {
        ($level:ident) => {
            tracing::$level!(
                client = %s.launch.client.name,
                kind = change.kind.name(),
                from_kbps = change.from_bps / 1000,
                to_kbps = change.to_bps / 1000,
                acked_kbps = change.acked_bps / 1000,
                loss_permille = (change.loss * 1000.0).round() as u32,
                old_kbps,
                new_kbps,
                shadow,
                "congestion control"
            )
        };
    }
    match change.kind {
        ChangeKind::Overuse | ChangeKind::Loss => line!(warn),
        ChangeKind::Restored | ChangeKind::Stale => line!(info),
    }
}
