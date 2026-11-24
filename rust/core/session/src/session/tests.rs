//! Tests for stream sessions, launches, registries, link rate, FEC, pacing, and keyframe loss.
use super::*;
use crate::state::Client;

mod fec;
mod keyframe_loss;
mod lifecycle;
mod link_rate;
mod pacing;
mod registry;

fn fec_report(frame: u32, received_data: u16, received_parity: u16) -> [u8; 21] {
    // Ten data and two parity packets at 20 %, one block.
    let mut report = [0; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [(4, 11u16), (10, 10), (12, 2)] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[14..16].copy_from_slice(&received_data.to_be_bytes());
    report[16..18].copy_from_slice(&received_parity.to_be_bytes());
    report[18] = 20;
    report[20] = 1;
    report
}
/// A session with the link controller on, `frames` frames of 100 packets
/// sent 17 ms apart ending now, and the wire frame marker past them.
fn link_session(frames: u32, enabled: bool) -> (Arc<Session>, Instant) {
    let s = Session::new(
        launch("link-rate", Role::Stream),
        Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(enabled, 0);
    let end = Instant::now();
    for frame in 1..=frames {
        let at = end - Duration::from_millis(17 * u64::from(frames - frame));
        s.record_link_sent_at(frame, 100, 112_600, at);
    }
    s.stats
        .video_frame
        .store(u64::from(frames) + 10, Ordering::Release);
    (s, end)
}
fn lose_frames(s: &Session, frames: std::ops::RangeInclusive<u32>) {
    for frame in frames {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
}
fn ask_for_notices(s: &Session) {
    use butterpollo_media_policy::link_notice::LinkNoticeRequest;
    assert_eq!(
        s.link_notices
            .on_request(&LinkNoticeRequest { enabled: true }.encode()),
        Some(true)
    );
}
fn notice_at(
    s: &Session,
    at: Instant,
) -> Option<butterpollo_media_policy::link_notice::LinkNotice> {
    let bytes = s.link_notices.take(at)?;
    Some(
        butterpollo_media_policy::link_notice::LinkNotice::decode(&bytes)
            .expect("the host sends valid notices"),
    )
}
/// One block of a client FEC report: `data` and `parity` packets, of which
/// `received` arrived (data first).
fn fec_block(frame: u32, block: u8, blocks: u8, data: u16, received: u16) -> [u8; 21] {
    let parity = (u32::from(data) * 20).div_ceil(100) as u16;
    let mut report = [0; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [(10, data), (12, parity), (14, received.min(data))] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[16..18].copy_from_slice(&received.saturating_sub(data).to_be_bytes());
    report[18] = 20;
    report[19] = block;
    report[20] = blocks;
    report
}
const FAST: u64 = 800_000_000;
const SAFE: u64 = 62_000_000;
const FRAME: Duration = Duration::from_micros(16_667);
fn fence_at(s: &Session) -> Option<Instant> {
    s.link_rate.lock().unwrap().rate.fence_at()
}
/// A link session whose first keyframe (wire frame 1) was lost at `t0`,
/// with the pace `base` requested for every frame.
fn first_keyframe_lost(base: u64, t0: Instant) -> Arc<Session> {
    let (s, _) = link_session(60, true);
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps_at(base, SAFE, 1, true, t0);
    s.keyframe_loss_bps_at(base, SAFE, 2, false, t0 + FRAME);
    let latched = s
        .keyframe_loss
        .lock()
        .unwrap()
        .unrecoverable(1, 2, FRAME, t0 + Duration::from_millis(100))
        .unwrap();
    assert_eq!(
        latched.kind,
        butterpollo_media_policy::keyframe_loss::LatchKind::Startup
    );
    s.report_keyframe_loss(Some(latched), None);
    s
}
fn link_warning(s: &Session) -> Option<String> {
    s.launch
        .warnings
        .snapshot()
        .into_iter()
        .find(|w| w.code == LINK_WARNING)
        .map(|w| w.message)
}
fn pacing_traffic(s: &Session, frames: u64) {
    s.stats.frames.store(frames, Ordering::Relaxed);
    s.stats.packets.store(frames * 70, Ordering::Relaxed);
    s.stats.bytes.store(frames * 83_334, Ordering::Relaxed);
    // The wire frame marker stores last_sent + 1, not last_sent.
    s.stats.video_frame.store(frames + 1, Ordering::Release);
}
fn launch(id: &str, role: Role) -> Launch {
    Launch {
        id: id.into(),
        client: Client {
            name: "fixture".into(),
            cert: String::new(),
            uuid: "client".into(),
            perm: u32::MAX,
            enabled: true,
            extra: Default::default(),
        },
        peer: "127.0.0.1".parse().unwrap(),
        app_id: 1,
        background: None,
        key: [0; 16],
        key_id: 1,
        ping: "ping".into(),
        connect_data: 1,
        role,
        created: Instant::now(),
        rtsp_encrypted: true,
        rtsp_counter: Arc::new(AtomicU32::new(1)),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: false,
        host_audio: false,
        requested_rate: 0,
        options: Default::default(),
        audio_preparation: Default::default(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        nucleus: None,
        held: None,
        warnings: Default::default(),
    }
}
/// A stream sending frames 1..=40 on a good link; `FAST`/`SAFE` as above.
fn steady_stream(name: &str) -> std::sync::Arc<Session> {
    let s = Session::new(launch(name, Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(800_000_000, 62_000_000, 1, true);
    for frame in 2..=40 {
        s.keyframe_loss_bps(800_000_000, 62_000_000, frame, false);
    }
    s.stats.video_frame.store(41, Ordering::Release);
    s
}
fn webrtc_launch(id: &str, client: &str) -> Launch {
    let mut launch = launch(id, Role::Stream);
    launch.transport = Transport::WebRtc;
    launch.client.uuid = client.into();
    launch
}
