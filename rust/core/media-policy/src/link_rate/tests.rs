//! Shared test harness and fixtures for link-rate adaptation tests.

use std::time::{Duration, Instant};

use super::sim::*;
use super::*;

const PERIOD_MS: u64 = 17;
const PACKETS: u32 = 100;
const WIRE_BYTES: u32 = 112_600;

fn stream(kbps: u32) -> Negotiated {
    Negotiated {
        bitrate_kbps: kbps,
        ..Default::default()
    }
}

fn status(frame: u32, received: u16, parity_lost: u16) -> Status {
    Status {
        frame,
        highest_received: 0,
        next_contiguous: 0,
        missing_before_highest: 0,
        data: 80,
        parity: 16,
        received_data: received,
        received_parity: 16 - parity_lost,
        percentage: 20,
        block: 0,
        blocks: 1,
    }
}

/// A frame the client could not rebuild.
fn unrecoverable(lr: &mut LinkRate, frame: u32, now: Instant) {
    lr.on_status(&status(frame, 60, 0), now);
    lr.on_unrecoverable(frame, false, now);
}

/// A keyframe the client could not rebuild, sent just now.
fn lost_keyframe(lr: &mut LinkRate, frame: u32, now: Instant) {
    lr.on_keyframe_sent(frame, now);
    lr.on_status(&status(frame, 60, 0), now);
    lr.on_unrecoverable(frame, true, now);
}

/// A frame that lost `missing` packets, all of them repaired by FEC.
fn repaired(lr: &mut LinkRate, frame: u32, missing: u16, now: Instant) {
    lr.on_status(&status(frame, 80, missing), now);
}

/// A 60 fps host: one frame every 17 ms, a tick every 8 ms and a ping every
/// 100 ms, one millisecond at a time.
struct Rig {
    lr: LinkRate,
    base: Instant,
    ms: u64,
    frame: u32,
    kbps: u32,
    rtt: Duration,
    /// Up to this many milliseconds are added to `rtt` at every tick.
    rtt_jitter_ms: u64,
    /// False while the picture is static.
    busy: bool,
    /// The share of its bitrate the picture needs, in percent: a calm one
    /// sends a fraction of what the encoder may.
    picture_percent: u64,
    changes: Vec<(u64, Change)>,
}

impl Rig {
    fn new(kbps: u32) -> Self {
        Self::with(LinkRate::new(true, &stream(kbps), 0, 20), kbps)
    }
    fn with(lr: LinkRate, kbps: u32) -> Self {
        Self {
            lr,
            base: Instant::now(),
            ms: 0,
            frame: 0,
            kbps,
            rtt: Duration::from_millis(20),
            rtt_jitter_ms: 0,
            busy: true,
            picture_percent: 100,
            changes: Vec::new(),
        }
    }
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.ms)
    }
    /// Runs `ms` milliseconds; `each` sees every frame right after it is sent.
    fn run(&mut self, ms: u64, mut each: impl FnMut(&mut LinkRate, u32, Instant)) {
        for _ in 0..ms {
            self.ms += 1;
            let now = self.now();
            if self.ms.is_multiple_of(PERIOD_MS) {
                self.frame = self.frame.wrapping_add(1);
                // The encoder follows its bitrate, so the frames shrink with it.
                let share = |full: u32| {
                    u64::from(full) * u64::from(self.kbps) / 40_000 * self.picture_percent / 100
                };
                let bytes = share(WIRE_BYTES) as u32;
                let packets = share(PACKETS).max(1) as u32;
                self.lr.on_sent(self.frame, packets, bytes, now);
                each(&mut self.lr, self.frame, now);
            }
            if self.ms.is_multiple_of(100) {
                self.lr.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            }
            if self.ms.is_multiple_of(8)
                && let Some(kbps) = self.lr.tick(
                    now,
                    self.rtt
                        + Duration::from_millis(
                            self.ms.wrapping_mul(2_654_435_761) % (self.rtt_jitter_ms + 1),
                        ),
                    20,
                    Duration::from_micros(16_667),
                    self.kbps,
                    self.busy,
                )
            {
                self.kbps = kbps;
            }
            if let Some(change) = self.lr.take_change() {
                self.changes.push((self.ms, change));
            }
        }
    }
    fn clean(&mut self, ms: u64) {
        self.run(ms, |_, _, _| {});
    }
    /// Three unrecoverable frames in a row.
    fn lose_three(&mut self) {
        let mut left = 3;
        self.run(PERIOD_MS * 3 + 1, |lr, frame, now| {
            if left > 0 {
                left -= 1;
                unrecoverable(lr, frame, now);
            }
        });
    }
    fn kinds(&self) -> Vec<ChangeKind> {
        self.changes.iter().map(|(_, c)| c.kind).collect()
    }
    fn cuts(&self) -> usize {
        self.kinds()
            .iter()
            .filter(|k| **k == ChangeKind::StepDown)
            .count()
    }
}
/// The 2026-10-24 `wifi` arm (150 Mbit/s, 3 +/- 2 ms delay, 0.5 % loss, HEVC
/// 1080p60 at 14,988 kbps, 20 % FEC) from `host-samples.jsonl`: about 38 packets
/// a frame. A frame leaves the host in under a millisecond and the +/- 2 ms
/// jitter then puts its packets in an almost random order, so the client
/// rebuilds a block when its 32nd packet is in and reports it with parity in
/// and data still on the way: 17.7 % "missing" in the log, so nearly every
/// frame was reported. The old controller cut to the 2,415 kbps floor within
/// seven seconds (15 changes).
///
/// Modelled here as it is: a random arrival order per block, 0.5 % of packets
/// lost, a report at the 32nd arrival unless all 32 data packets came first.
/// One block of the `wifi` arm as the client reports it (see the test below):
/// data 32 + `parity` in a random arrival order, `loss_permille` of the packets
/// lost, a report at the 32nd arrival unless all 32 data packets came first.
/// `queued` more data packets than that were still behind a queue when parity
/// made up for them. Returns whether the block was reported.
fn wifi_block(
    seed: &mut u64,
    lr: &mut LinkRate,
    frame: u32,
    parity_shards: u16,
    loss_permille: u64,
    queued: u16,
    now: Instant,
) -> bool {
    let mut random = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    };
    let mut order: Vec<u16> = (0..32 + parity_shards).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, (random() % (i as u64 + 1)) as usize);
    }
    let (mut data, mut parity) = (0u16, 0u16);
    let mut report = None;
    for index in order {
        if random() % 1000 < loss_permille {
            continue;
        }
        if index < 32 {
            data += 1;
        } else {
            parity += 1;
        }
        if data + parity == 32 {
            report = Some((data, parity));
            break;
        }
    }
    let (data, parity) = report.unwrap_or((data, parity));
    // A block whose data all came first needs no report.
    if data == 32 {
        return false;
    }
    let (data, parity) = (
        data.saturating_sub(queued),
        (parity + queued).min(parity_shards),
    );
    lr.on_status(
        &Status {
            frame,
            highest_received: 31 + parity_shards,
            next_contiguous: 0,
            missing_before_highest: 32 + parity_shards - data - parity,
            data: 32,
            parity: parity_shards,
            received_data: data,
            received_parity: parity,
            percentage: (u32::from(parity_shards) * 100 / 32) as u8,
            block: 0,
            blocks: 1,
        },
        now,
    );
    true
}
/// A 1080p60 PyroWave stream with record framing.
fn pyrowave(kbps: u32) -> Negotiated {
    Negotiated {
        codec: 3,
        pyrowave_records: true,
        width: 1920,
        height: 1080,
        fps: 60,
        ..stream(kbps)
    }
}
const PERIOD: Duration = Duration::from_micros(16_667);

impl Rig {
    fn probes(&self) -> usize {
        self.changes
            .iter()
            .filter(|(_, c)| c.kind == ChangeKind::Probe)
            .count()
    }
    /// Runs until one more probe was issued; returns it. Repeated failures
    /// at one rate wait up to [`RETRY_MAX`] for a retry.
    fn until_probe(&mut self) -> Change {
        let seen = self.probes();
        for _ in 0..36_000 {
            self.clean(10);
            if self.probes() > seen {
                return self.last(ChangeKind::Probe).unwrap();
            }
        }
        panic!("no probe in 360 s: {:?}", self.kinds());
    }
    fn last(&self, kind: ChangeKind) -> Option<Change> {
        self.changes
            .iter()
            .rev()
            .find(|(_, c)| c.kind == kind)
            .map(|(_, c)| *c)
    }
}
fn q(mut link: Link, ms: f64) -> Link {
    link.queue_ms = ms;
    link
}

fn gilbert(mbps: f64, enter: f64, leave: f64, loss: f64) -> Link {
    let mut link = Link::constant(mbps);
    link.burst = Some((enter, leave, loss));
    link
}

fn lossy(mut link: Link, share: f64) -> Link {
    link.random_loss = share;
    link
}
/// A 33 Mbps stream whose picture needs `percent` of it, three seconds in.
fn calm(percent: u64) -> Rig {
    let mut rig = Rig::new(33_000);
    rig.picture_percent = percent;
    rig.busy = false;
    rig.clean(3_000);
    rig
}

mod calm_picture;
mod cuts_and_pyrowave;
mod evidence;
mod latch_watch;
mod learned_ceiling;
mod probing;
mod sim_probing;
mod sim_pyrowave;
mod standing_queue;
