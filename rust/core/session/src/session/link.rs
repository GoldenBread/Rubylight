//! What the session says about link-rate changes: the log lines' and the
//! client notice's bookkeeping.
use std::time::{Duration, Instant};

/// The session warning code while the link-rate controller holds the video
/// bitrate below the requested one.
pub const LINK_WARNING: &str = "network_bitrate_limited";

/// Update the bookkeeping for a change, under the lock, and return what to
/// say about it.
pub(super) fn note_link_change(
    link: &mut butterpollo_media_policy::link_control::LinkControl,
    now: Instant,
    change: butterpollo_media_policy::link_rate::Change,
) -> LinkReport {
    use butterpollo_media_policy::link_rate::ChangeKind;
    let was_limited = std::mem::replace(&mut link.limited, change.kind != ChangeKind::Full);
    match change.kind {
        ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor => {
            if change.evidence.loss_share > 0. {
                link.loss_share = change.evidence.loss_share;
            }
            link.lowest_kbps = match link.lowest_kbps {
                0 => change.to_kbps,
                lowest => lowest.min(change.to_kbps),
            };
        }
        ChangeKind::Full => link.lowest_kbps = 0,
        ChangeKind::Probe | ChangeKind::ProbeFailed => {}
    }
    // Routine changes go to the log at most once a second; the first
    // step down and the floors have their own WARN. A PyroWave stream with no
    // room under it reaches its floor again after every quiet spell: once.
    let no_room = link.rate.pyrowave_no_room();
    let warned = match change.kind {
        ChangeKind::StepDown => !was_limited,
        ChangeKind::Floor if no_room => !std::mem::replace(&mut link.floor_warned, true),
        ChangeKind::Floor | ChangeKind::EncoderFloor => true,
        _ => false,
    };
    let log_info = !warned
        && link
            .logged
            .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1));
    if log_info {
        link.logged = Some(now);
    }
    LinkReport {
        change,
        was_limited,
        warned,
        no_room,
        ceiling_kbps: link.rate.ceiling_kbps(),
        loss_share: link.loss_share,
        lowest_kbps: link.lowest_kbps,
        log_info,
    }
}

/// What to say about a link-rate change, worked out under the controller's
/// lock and said after it.
pub(super) struct LinkReport {
    pub(super) change: butterpollo_media_policy::link_rate::Change,
    pub(super) was_limited: bool,
    /// The change has its own WARN line.
    pub(super) warned: bool,
    /// PyroWave's floor is the stream's own bitrate.
    pub(super) no_room: bool,
    pub(super) ceiling_kbps: u32,
    pub(super) loss_share: f64,
    pub(super) lowest_kbps: u32,
    pub(super) log_info: bool,
}

/// What a client that asked for notices (0x5538) is told about a change.
pub(super) fn link_notice(
    report: &LinkReport,
) -> butterpollo_media_policy::link_notice::LinkNotice {
    use butterpollo_media_policy::link_notice::{LinkNotice, LinkReason, LinkState};
    use butterpollo_media_policy::link_rate::ChangeKind;
    let (state, reason) = match report.change.kind {
        ChangeKind::Full => (LinkState::Full, LinkReason::None),
        ChangeKind::Floor | ChangeKind::EncoderFloor => (LinkState::Floor, LinkReason::Network),
        ChangeKind::StepDown | ChangeKind::Probe | ChangeKind::ProbeFailed => {
            (LinkState::Limited, LinkReason::Network)
        }
    };
    let current_kbps = report.change.to_kbps.max(1);
    LinkNotice {
        state,
        reason,
        ceiling_kbps: report.ceiling_kbps.max(current_kbps),
        current_kbps,
    }
}

/// A bitrate for a sentence: "15 Mbps", "4.5 Mbps".
pub(super) fn mbps(kbps: u32) -> String {
    if kbps >= 10_000 {
        format!("{} Mbps", (f64::from(kbps) / 1000.).round())
    } else {
        format!("{:.1} Mbps", f64::from(kbps) / 1000.)
    }
}

use super::{Negotiated, Ordering, Session};

impl<P, A> Session<P, A> {
    /// Starts the link-rate controller for the stream: `enabled` is
    /// `Config::link_rate_adaptation`, `maximum_kbps` the user's `max_bitrate`.
    pub fn configure_link_rate(&self, enabled: bool, maximum_kbps: u32) {
        self.rebuild_link_rate(&self.config, enabled, maximum_kbps);
    }
    /// A controller for a stream whose mode changed (0x5532). A bitrate it had
    /// lowered goes back to the requested one; what it learned about the link
    /// is forgotten, and a client that took the bitrate over keeps it.
    pub fn rebuild_link_rate(&self, stream: &Negotiated, enabled: bool, maximum_kbps: u32) {
        let mut link = self.link_rate.lock().unwrap();
        // Before the bitrate from before the cut: see `store_limited`.
        self.fec_split_full.store(0, Ordering::Release);
        if let Some(kbps) = link.restore.take() {
            self.bitrate.store(kbps, Ordering::Release);
        }
        {
            // Congestion control's limit goes too; `rebuild_congestion` starts it again.
            let mut congestion = self.congestion.lock().unwrap();
            if let Some(kbps) = congestion.take_restore() {
                self.bitrate.store(kbps, Ordering::Release);
            }
            congestion.link_wanted = None;
        }
        let rate = link
            .rate
            .rebuild(enabled, stream, maximum_kbps, self.fec_percent());
        self.link_sent
            .set_live(rate.state() != butterpollo_media_policy::link_rate::State::Off);
        *link =
            butterpollo_media_policy::link_control::LinkControl::new(rate, self.link_sent.head());
        drop(link);
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices
            .note_full(self.bitrate.load(Ordering::Acquire));
    }
    /// A frame the video sender sent: its wire frame number, packets (data and
    /// parity) and wire bytes. Lock-free, so any sender path may call it.
    pub fn record_link_sent(&self, wire_frame: u32, packets: usize, wire_bytes: u64) {
        self.record_link_sent_at(wire_frame, packets, wire_bytes, Instant::now());
    }
    pub(super) fn record_link_sent_at(
        &self,
        wire_frame: u32,
        packets: usize,
        wire_bytes: u64,
        at: Instant,
    ) {
        self.link_sent.record(
            wire_frame,
            packets.min(u32::MAX as usize) as u32,
            wire_bytes,
            at,
        );
    }
    /// A client request for a keyframe (E4).
    pub(super) fn link_request(&self) {
        self.link_request_with(|rate, network, now| rate.on_keyframe_request(network, now));
    }
    /// A client request to invalidate frames `first..=last` (E4), counted once
    /// per incident.
    pub(super) fn link_invalidation(&self, first: u64, last: u64) {
        self.link_request_with(|rate, network, now| {
            rate.on_invalidation(first, last, network, now)
        });
    }
    fn link_request_with(
        &self,
        count: impl FnOnce(&mut butterpollo_media_policy::link_rate::LinkRate, bool, Instant),
    ) {
        if !self.link_sent.live() {
            return;
        }
        let now = Instant::now();
        // Every PyroWave picture is intra: no keyframe answers a request, and
        // what it lost is in the FEC status already, so a request is no
        // evidence of its own.
        let network =
            self.config.codec != 3 && self.keyframe_loss.lock().unwrap().network_request(now);
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        count(&mut link.rate, network, now);
    }
    /// One validated, deduplicated FEC block (E1 and E2).
    pub(super) fn link_status(
        &self,
        payload: &[u8],
        frame: u32,
        block: butterpollo_media_policy::adaptive_fec::Block,
    ) {
        if !self.link_sent.live() {
            return;
        }
        let Ok(status) = rubylight_wire::fec_status::Status::parse(payload) else {
            return;
        };
        let now = Instant::now();
        let (network, abandoned, keyframe) = {
            let loss = self.keyframe_loss.lock().unwrap();
            (
                matches!(
                    block,
                    butterpollo_media_policy::adaptive_fec::Block::Unrecoverable
                ) && loss.network_loss(frame, now),
                loss.is_abandoned(frame),
                loss.is_keyframe(frame),
            )
        };
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        // Packets of a frame the host stopped sending are its own loss,
        // counted once on send (E3), not as the network's (E2).
        if !abandoned {
            link.rate.on_status(&status, now);
        }
        if network && link.rate.frame_lost(&status) {
            link.rate.on_unrecoverable(frame, keyframe, now);
        }
    }
    /// The link-rate state for the session API and the `stream timings` line.
    pub fn link_rate_snapshot(&self) -> serde_json::Value {
        self.link_rate.lock().unwrap().rate.snapshot(Instant::now())
    }
    /// Call from the feedback tick. Applies the controller's bitrate and says
    /// so, loudly: a WARN on leaving the requested rate, a session warning for
    /// the console, an INFO for later changes. Returns `(old, new)` kbps when
    /// the encoder bitrate changed.
    pub fn tick_link_rate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        if !self.link_sent.live() {
            return None;
        }
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        let period = self.stream_mode().period();
        let current = self.bitrate.load(Ordering::Acquire);
        let busy = link.busy(
            now,
            self.stats.frames.load(Ordering::Relaxed),
            self.stats.bytes.load(Ordering::Relaxed),
            period,
            current,
            if self.config.codec == 3 {
                butterpollo_media_policy::link_control::PYROWAVE_BYTE_SHARE
            } else {
                butterpollo_media_policy::link_control::BYTE_SHARE
            },
        );
        let target = link
            .rate
            .tick(now, rtt, self.fec_percent(), period, current, busy);
        let mut congestion = self.congestion.lock().unwrap();
        let applied = if congestion.active() {
            // Congestion control writes the bitrate: the lower of both limits.
            congestion.link_wanted = target;
            congestion.combine(current)
        } else {
            match target {
                Some(kbps) => {
                    link.restore.get_or_insert(current);
                    kbps
                }
                None => link.restore.take().unwrap_or(current),
            }
        };
        let full = link.restore.or(congestion.limited_from());
        drop(congestion);
        self.store_limited(current, applied, full);
        // Say it once the lock is released: logging and the warning list must
        // not run under it.
        let report = link
            .rate
            .take_change()
            .map(|change| note_link_change(&mut link, now, change));
        drop(link);
        if let Some(report) = report {
            self.link_notices.note(link_notice(&report));
            self.announce_link_change(report);
        }
        (applied != current).then_some((current, applied))
    }
    /// The WARN, the console warning or the INFO for a change.
    pub(super) fn announce_link_change(&self, report: LinkReport) {
        use butterpollo_media_policy::link_rate::ChangeKind;
        let LinkReport {
            change,
            was_limited,
            warned,
            no_room,
            ceiling_kbps: ceiling,
            loss_share,
            lowest_kbps,
            log_info,
        } = report;
        let e = change.evidence;
        let kind = match change.kind {
            ChangeKind::StepDown => "step_down",
            ChangeKind::Probe => "probe",
            ChangeKind::ProbeFailed => "probe_failed",
            ChangeKind::Floor => "floor",
            ChangeKind::EncoderFloor => "encoder_floor",
            ChangeKind::Full => "full",
        };
        match change.kind {
            ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor => {
                let message = match change.kind {
                    ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor
                        if change.reason == "send_losses" =>
                    {
                        format!(
                            "The PC could not send the stream fast enough (its CPU or network adapter fell behind and the socket refused video packets), so the host lowered the video bitrate to {} and raises it again once the PC keeps up. Close other work on the PC, or lower the resolution or frame rate in Moonlight.",
                            mbps(change.to_kbps)
                        )
                    }
                    ChangeKind::Floor if no_room => format!(
                        "This network loses PyroWave video at {}, and PyroWave cannot go lower at this resolution and frame rate and keep a usable picture, so the picture will keep breaking up. HEVC or AV1 will hold up better on this device; or check the network path (Wi-Fi signal, cable, router).",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::Floor if self.config.codec == 3 => format!(
                        "The network to this device cannot carry even {} of video, the lowest bitrate at which PyroWave keeps a usable picture, so the picture will keep breaking up. Use HEVC or AV1 for this device, or check the network path (Wi-Fi signal, cable, router).",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::Floor => format!(
                        "The network to this device cannot carry even {} of video, the lowest bitrate the host uses, so the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::EncoderFloor => format!(
                        "The network to this device still loses video at {}, and the encoder does not go lower for this picture, so the host stopped lowering the bitrate there and the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    _ => {
                        let loss = if change.reason == "queue" {
                            "the network was holding video back in a growing queue".to_string()
                        } else if loss_share > 0. {
                            format!("{:.0} % of packets lost", loss_share * 100.)
                        } else {
                            "video frames were lost".to_string()
                        };
                        format!(
                            "The network to this device could not carry {} of video ({loss}). The host lowered the bitrate, as low as {} so far, and raises it again as the link allows. Lower the bitrate in Moonlight to avoid this.",
                            mbps(ceiling),
                            mbps(lowest_kbps)
                        )
                    }
                };
                self.launch.warnings.set(LINK_WARNING, message);
            }
            ChangeKind::Full => self.launch.warnings.clear(LINK_WARNING),
            ChangeKind::Probe | ChangeKind::ProbeFailed => {}
        }
        match change.kind {
            ChangeKind::StepDown if !was_limited && change.reason == "send_losses" => {
                tracing::warn!(
                    stream_id = %self.launch.id,
                    link_rate_change = kind,
                    reason = change.reason,
                    from_kbps = change.from_kbps,
                    to_kbps = change.to_kbps,
                    delivered_kbps = change.delivered_kbps,
                    unrecoverable_frames = e.unrecoverable_frames,
                    send_losses = e.send_losses,
                    keyframe_requests = e.keyframe_requests,
                    loss_windows = e.loss_windows,
                    loss_share_percent = e.loss_share * 100.,
                    "the PC could not send the stream fast enough (its socket refused video packets); lowering the video bitrate"
                )
            }
            ChangeKind::StepDown if !was_limited => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                ceiling_kbps = ceiling,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_windows = e.loss_windows,
                loss_share_percent = e.loss_share * 100.,
                "the network to this device cannot carry the stream; lowering the video bitrate"
            ),
            ChangeKind::Floor if warned && change.reason == "send_losses" => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                send_losses = e.send_losses,
                loss_share_percent = e.loss_share * 100.,
                "the PC could not send the stream fast enough even at the lowest video bitrate (its socket refused video packets)"
            ),
            ChangeKind::Floor if warned => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_share_percent = e.loss_share * 100.,
                pyrowave_no_room = no_room,
                "the network to this device cannot carry the stream even at the lowest video bitrate"
            ),
            ChangeKind::EncoderFloor => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                "the encoder sends more than the video bitrate asked for; treating what it sends as its lowest rate"
            ),
            _ if log_info => {
                if change.kind == ChangeKind::Full {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        to_kbps = change.to_kbps,
                        "video bitrate back at the requested rate"
                    );
                } else {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        reason = change.reason,
                        from_kbps = change.from_kbps,
                        to_kbps = change.to_kbps,
                        delivered_kbps = change.delivered_kbps,
                        "video bitrate changed to follow the network"
                    );
                }
            }
            _ => {}
        }
    }
}
