//! A deterministic link model for `LinkRate`: a token-bucket link with a
//! drop-tail queue, a 60 fps frame source, 20 % FEC, and client feedback that
//! arrives after a delay. Like stock Moonlight, the client reports FEC status
//! only for blocks that lost packets; a frame that lost every packet is never
//! reported.
//!
//! It aims at what the host does: a frame leaves in a burst (`spread`, 5 % of
//! the frame period), every 120th frame is three times the usual size, and the
//! RTT the controller sees is the base RTT plus the time the queue takes to
//! drain, sampled with the 100 ms pings.
//!
//! [`Sim::report`] sums a run the way the 2026-11-17 review asked: cuts,
//! failed probes, time the stream offered more than the link carries, the
//! p95 queue delay and the mean rate delivered. It reads only the change log,
//! the frames and the queue samples, so the same file runs against an older
//! controller for a before-and-after table (`field_report`, ignored).
use std::time::{Duration, Instant};

use super::*;
use crate::{fec_status::Status, packet::VideoPacketizer};

pub const PERIOD: Duration = Duration::from_micros(16_667);
pub const FEC: usize = 20;
const DATAGRAM: u64 = 1024 + 16 + 86;

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub struct Link {
    /// Link rate in bit/s at a time in seconds.
    pub rate: Box<dyn Fn(f64) -> f64>,
    pub queue_ms: f64,
    pub random_loss: f64,
    /// Gilbert burst loss: chance per packet to enter the bad state, to leave
    /// it, and to lose a packet in it.
    pub burst: Option<(f64, f64, f64)>,
    in_burst: bool,
    pub delay: f64,
    backlog: f64,
    at: f64,
    rng: Rng,
}
impl Link {
    pub fn new(rate: impl Fn(f64) -> f64 + 'static) -> Self {
        Self {
            rate: Box::new(rate),
            queue_ms: 50.,
            random_loss: 0.,
            burst: None,
            in_burst: false,
            delay: 0.002,
            backlog: 0.,
            at: 0.,
            rng: Rng::new(7),
        }
    }
    pub fn constant(mbps: f64) -> Self {
        Self::new(move |_| mbps * 1e6)
    }
    /// `a` until `from` seconds, `b` until `to`, then `a` again.
    pub fn step(a: f64, b: f64, from: f64, to: f64) -> Self {
        Self::new(move |t| {
            if (from..to).contains(&t) {
                b * 1e6
            } else {
                a * 1e6
            }
        })
    }
    /// Seconds the queue takes to drain at `t`.
    pub fn queue_delay(&self, t: f64) -> f64 {
        let rate = (self.rate)(t).max(1.);
        (self.backlog - rate / 8. * (t - self.at).max(0.)).max(0.) * 8. / rate
    }
    /// Arrival time of a datagram of `bytes` offered at `t`, or `None` if
    /// the queue is full (or the packet is lost at random).
    fn offer(&mut self, t: f64, bytes: f64) -> Option<f64> {
        let t = t.max(self.at);
        let rate = (self.rate)((self.at + t) / 2.);
        self.backlog = (self.backlog - rate / 8. * (t - self.at)).max(0.);
        self.at = t;
        let rate = (self.rate)(t);
        if self.backlog + bytes > rate / 8. * self.queue_ms / 1000. {
            return None;
        }
        self.backlog += bytes;
        if self.random_loss > 0. && self.rng.next() < self.random_loss {
            return None;
        }
        if let Some((enter, leave, loss)) = self.burst {
            let flip = self.rng.next();
            self.in_burst = if self.in_burst {
                flip >= leave
            } else {
                flip < enter
            };
            if self.in_burst && self.rng.next() < loss {
                return None;
            }
        }
        Some(t + self.backlog * 8. / rate + self.delay)
    }
}

#[derive(Clone, Copy)]
pub struct Frame {
    pub at: f64,
    pub kbps: u32,
    pub complete: bool,
    pub arrival: f64,
    pub packets: u32,
    /// Packets that reached the client.
    pub arrived: u32,
}

/// What one window of a run did; see [`Sim::report`].
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub cuts: usize,
    pub failed_probes: usize,
    /// Share of the window's seconds whose offered wire rate was over the
    /// link's rate.
    pub over_link: f64,
    pub p95_queue_ms: f64,
    pub delivered_mbps: f64,
    pub frame_loss: f64,
}

enum Feedback {
    Status(Status),
}

pub struct Sim {
    pub lr: LinkRate,
    pub link: Link,
    pub base: Instant,
    /// Delay from the last packet of a frame to the host reading the report.
    pub feedback: f64,
    /// Share of the frame period the packets of one frame are spread over
    /// (the host sends a frame in a burst of about 0.05).
    pub spread: f64,
    pub frames: Vec<Frame>,
    pub changes: Vec<(f64, Change)>,
    pub kbps_log: Vec<(f64, u32)>,
    /// The queue in front of the link, in ms, every 100 ms.
    pub queue_log: Vec<(f64, f64)>,
    pub keyframe_requests: bool,
    pub client_pings: bool,
    /// The lowest bitrate the simulated encoder produces, however low it is
    /// asked (H.264 on AMD stops near 14 Mbps on noise).
    pub encoder_min_kbps: u32,
    /// PyroWave pictures (see [`Sim::pyrowave`]) instead of H.26x frames.
    pub pyrowave: bool,
    /// A PyroWave picture fills this share of its frame budget, plus up to
    /// the second figure at random (a game scene: 0.5 and 0.2).
    pub fill: (f64, f64),
    /// The frame period the source sends at and the controller is told.
    pub period: Duration,
    /// The host's socket refuses a whole frame while the link's queue is
    /// over this many ms (a switch or mesh node pausing the host's adapter
    /// when its own uplink is full); the frame is lost on send (E3).
    pub refuse_above_ms: Option<f64>,
    encoder_kbps: u32,
    applied: [u32; 2],
    noise: Rng,
    inbox: Vec<(f64, Feedback)>,
}

impl Sim {
    pub fn new(link: Link, negotiated_kbps: u32, feedback_ms: f64) -> Self {
        let base = Instant::now();
        let stream = Negotiated {
            bitrate_kbps: negotiated_kbps,
            ..Default::default()
        };
        Self {
            lr: LinkRate::new(true, &stream, 0, FEC),
            link,
            base,
            feedback: feedback_ms / 1000.,
            spread: 0.05,
            frames: Vec::new(),
            changes: Vec::new(),
            kbps_log: Vec::new(),
            queue_log: Vec::new(),
            keyframe_requests: false,
            client_pings: true,
            encoder_min_kbps: 0,
            pyrowave: false,
            fill: (0.5, 0.2),
            period: PERIOD,
            refuse_above_ms: None,
            encoder_kbps: negotiated_kbps,
            applied: [negotiated_kbps; 2],
            noise: Rng::new(99),
            inbox: Vec::new(),
        }
    }

    /// A 1080p60 PyroWave stream with record framing. Its rate control holds
    /// every picture to one frame's budget, keyframes included, and a game
    /// scene fills about 60 % of it (2026-10-29: 239 of 414 KB at 200 Mbps).
    /// Packets follow [`crate::pyrowave::plan`] as the host sends a moving
    /// picture: the coarse block first with its parity, the detail blocks with
    /// none.
    pub fn pyrowave(link: Link, negotiated_kbps: u32, feedback_ms: f64) -> Self {
        let mut sim = Self::new(link, negotiated_kbps, feedback_ms);
        let stream = Negotiated {
            codec: 3,
            pyrowave_records: true,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: negotiated_kbps,
            ..Default::default()
        };
        sim.lr = LinkRate::new(true, &stream, 0, FEC);
        sim.pyrowave = true;
        sim
    }

    /// The same stream at 120 fps, where PyroWave's severe-detail floor (373
    /// Mbps at 1080p120) is above its own requested rate: the controller has
    /// no quality floor to cut to, which is the 2026-11-17 4K120 field case.
    pub fn pyrowave_no_room(link: Link, negotiated_kbps: u32, feedback_ms: f64) -> Self {
        let mut sim = Self::pyrowave(link, negotiated_kbps, feedback_ms);
        let stream = Negotiated {
            codec: 3,
            pyrowave_records: true,
            width: 1920,
            height: 1080,
            fps: 120,
            bitrate_kbps: negotiated_kbps,
            ..Default::default()
        };
        sim.lr = LinkRate::new(true, &stream, 0, FEC);
        assert!(sim.lr.pyrowave_no_room());
        sim.period = Duration::from_micros(8_333);
        sim
    }

    /// The 2026-11-17 field stream: 4K120 PyroWave, whose floor (about 1490
    /// Mbps) is far above its own 800 Mbps, so it has no room either. The
    /// pictures fill about 77 % of their budget, so 800 Mbps asked puts about
    /// 680 Mbps on the wire, as the field log showed (641 to 676).
    pub fn pyrowave_4k120(link: Link, negotiated_kbps: u32, feedback_ms: f64) -> Self {
        let mut sim = Self::pyrowave(link, negotiated_kbps, feedback_ms);
        let stream = Negotiated {
            codec: 3,
            pyrowave_records: true,
            width: 3840,
            height: 2160,
            fps: 120,
            bitrate_kbps: negotiated_kbps,
            ..Default::default()
        };
        sim.lr = LinkRate::new(true, &stream, 0, FEC);
        assert!(sim.lr.pyrowave_no_room());
        sim.period = Duration::from_micros(8_333);
        sim.fill = (0.68, 0.18);
        sim
    }

    /// Data and parity packets per block of a PyroWave picture of `bytes`:
    /// coarse records about 5 % of it, protected at [`FEC`] percent (at least
    /// two parity packets), detail without parity, as `DetailFec` leaves it
    /// while the picture moves.
    fn pyrowave_layout(bytes: f64) -> Vec<(usize, usize)> {
        let total = ((bytes / 1024.).ceil() as usize).clamp(1, 4000);
        let blocks = crate::pyrowave::plan(total, (total / 20).max(1), FEC, 2, 0, 0)
            .expect("a PyroWave plan");
        blocks
            .iter()
            .map(|b| (b.data, crate::pyrowave::parity(b.data, b.percentage, 2)))
            .collect()
    }

    pub fn at(&self, t: f64) -> Instant {
        self.base + Duration::from_secs_f64(t)
    }

    fn send_frame(&mut self, t: f64, index: u32) {
        let kbps = self.applied[0].max(self.encoder_min_kbps);
        self.applied.rotate_left(1);
        self.applied[1] = self.encoder_kbps;
        let average = f64::from(kbps) * 125. * self.period.as_secs_f64();
        let layout = if self.pyrowave {
            Self::pyrowave_layout(average * (self.fill.0 + self.fill.1 * self.noise.next()))
        } else {
            let scale = match index {
                0 => 2.,
                i if i % 120 == 0 => 3.,
                _ => 1.,
            };
            let packetizer = VideoPacketizer {
                sequence: 0,
                frame: index,
                iv_counter: 0,
                packet_size: 1024,
                fec_percent: FEC,
                min_fec: 0,
                key: None,
            };
            packetizer.block_layout((average * (0.9 + 0.2 * self.noise.next()) * scale) as usize)
        };
        let packets: usize = layout.iter().map(|(d, p)| d + p).sum();
        if self
            .refuse_above_ms
            .is_some_and(|ms| self.link.queue_delay(t) * 1000. > ms)
        {
            // Nothing left the host: no packets on the link, no report.
            self.lr.on_sent(index, 0, 0, self.at(t));
            self.lr.on_send_loss(index, self.at(t));
            self.frames.push(Frame {
                at: t,
                kbps,
                complete: false,
                arrival: t,
                packets: 0,
                arrived: 0,
            });
            return;
        }
        self.lr.on_sent(
            index,
            packets as u32,
            (packets as u64 * DATAGRAM) as u32,
            self.at(t),
        );
        let (mut n, mut last_arrival, mut complete) = (0usize, t, true);
        let mut arrived = 0u32;
        let blocks = layout.len() as u8;
        for (block, &(data, parity)) in layout.iter().enumerate() {
            let (mut got_data, mut got_parity) = (0u16, 0u16);
            for i in 0..data + parity {
                let when = t + n as f64 / packets as f64 * self.spread * PERIOD.as_secs_f64();
                n += 1;
                if let Some(arrival) = self.link.offer(when, DATAGRAM as f64) {
                    last_arrival = last_arrival.max(arrival);
                    if i < data {
                        got_data += 1;
                    } else {
                        got_parity += 1;
                    }
                }
            }
            let received = got_data + got_parity;
            arrived += u32::from(received);
            // A PyroWave picture shows without a detail block (blurred there).
            if usize::from(received) < data && (!self.pyrowave || block == 0) {
                complete = false;
            }
            if usize::from(received) < data + parity {
                let status = Status {
                    frame: index,
                    highest_received: 0,
                    next_contiguous: 0,
                    missing_before_highest: 0,
                    data: data as u16,
                    parity: parity as u16,
                    received_data: got_data,
                    received_parity: got_parity,
                    percentage: FEC as u8,
                    block: block as u8,
                    blocks,
                };
                // A block that lost everything is never reported.
                if received > 0 {
                    self.inbox
                        .push((last_arrival + self.feedback, Feedback::Status(status)));
                }
            }
        }
        self.frames.push(Frame {
            at: t,
            kbps,
            complete,
            arrival: last_arrival,
            packets: packets as u32,
            arrived,
        });
    }

    fn deliver(&mut self, t: f64) {
        let now = self.at(t);
        let mut i = 0;
        while i < self.inbox.len() {
            if self.inbox[i].0 > t {
                i += 1;
                continue;
            }
            let (_, Feedback::Status(status)) = self.inbox.remove(i);
            self.lr.on_status(&status, now);
            if self.lr.frame_lost(&status) {
                if self.keyframe_requests {
                    self.lr.on_keyframe_request(true, now);
                }
                self.lr.on_unrecoverable(status.frame, false, now);
            }
        }
    }

    pub fn run(&mut self, seconds: f64) {
        let base_rtt = 2. * (self.link.delay + self.feedback) + 0.001;
        let mut rtt = Duration::from_secs_f64(base_rtt);
        let ms = (seconds * 1000.) as u64;
        let mut index = 0u32;
        let mut next_frame = 0.;
        for k in 0..ms {
            let t = k as f64 / 1000.;
            let now = self.at(t);
            if t >= next_frame {
                self.send_frame(t, index);
                index += 1;
                next_frame += self.period.as_secs_f64();
            }
            self.deliver(t);
            if k % 100 == 0 {
                rtt = Duration::from_secs_f64(base_rtt + self.link.queue_delay(t));
                self.queue_log.push((t, self.link.queue_delay(t) * 1000.));
            }
            if self.client_pings && k % 100 == 0 {
                self.lr.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            }
            if k % 8 == 0 {
                if let Some(kbps) =
                    self.lr
                        .tick(now, rtt, FEC, self.period, self.encoder_kbps, true)
                {
                    self.encoder_kbps = kbps;
                }
                if let Some(change) = self.lr.take_change() {
                    self.changes.push((t, change));
                }
            }
            if k % 1000 == 0 {
                self.kbps_log.push((t, self.encoder_kbps));
            }
        }
    }

    /// Frames sent in `[from, to)` seconds.
    pub fn window(&self, from: f64, to: f64) -> Vec<&Frame> {
        self.frames
            .iter()
            .filter(|f| f.at >= from && f.at < to)
            .collect()
    }
    /// Share of frames in the window that the client could not rebuild.
    pub fn frame_loss(&self, from: f64, to: f64) -> f64 {
        let w = self.window(from, to);
        w.iter().filter(|f| !f.complete).count() as f64 / w.len().max(1) as f64
    }
    /// Longest time between two rebuilt frames arriving in the window.
    pub fn longest_gap(&self, from: f64, to: f64) -> f64 {
        let mut last = from;
        let mut gap: f64 = 0.;
        for f in self
            .frames
            .iter()
            .filter(|f| f.complete && f.arrival >= from && f.arrival < to)
        {
            gap = gap.max(f.arrival - last);
            last = f.arrival;
        }
        gap.max(to - last)
    }
    /// Mean encoder bitrate of the frames sent in the window, kbps.
    pub fn mean_kbps(&self, from: f64, to: f64) -> f64 {
        let w = self.window(from, to);
        w.iter().map(|f| f64::from(f.kbps)).sum::<f64>() / w.len().max(1) as f64
    }
    /// Mean offered wire rate in the window, bit/s.
    pub fn mean_wire_bps(&self, from: f64, to: f64) -> f64 {
        let w = self.window(from, to);
        w.iter().map(|f| f64::from(f.packets)).sum::<f64>() * DATAGRAM as f64 * 8.
            / (to - from).max(1e-9)
    }
    pub fn cuts(&self) -> usize {
        self.changes
            .iter()
            .filter(|(_, c)| c.kind == ChangeKind::StepDown)
            .count()
    }
    /// Cuts, failed probes, time over the link, p95 queue delay, delivered
    /// rate and lost frames in `[from, to)` seconds.
    pub fn report(&self, from: f64, to: f64) -> Report {
        let count = |kind| {
            self.changes
                .iter()
                .filter(|(t, c)| (from..to).contains(t) && c.kind == kind)
                .count()
        };
        let seconds = (to - from).floor().max(1.) as usize;
        let over = (0..seconds)
            .filter(|i| {
                let at = from + *i as f64;
                self.mean_wire_bps(at, at + 1.) > (self.link.rate)(at + 0.5)
            })
            .count();
        let mut queue: Vec<f64> = self
            .queue_log
            .iter()
            .filter(|(t, _)| (from..to).contains(t))
            .map(|(_, ms)| *ms)
            .collect();
        queue.sort_by(f64::total_cmp);
        let p95 = queue
            .get((queue.len() * 95 / 100).min(queue.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.);
        let arrived: f64 = self
            .window(from, to)
            .iter()
            .map(|f| f64::from(f.arrived))
            .sum();
        Report {
            cuts: count(ChangeKind::StepDown),
            failed_probes: count(ChangeKind::ProbeFailed),
            over_link: over as f64 / seconds as f64,
            p95_queue_ms: p95,
            delivered_mbps: arrived * DATAGRAM as f64 * 8. / (to - from) / 1e6,
            frame_loss: self.frame_loss(from, to),
        }
    }
    /// First time at or after `from` that the encoder runs at `kbps` or more.
    pub fn reaches(&self, from: f64, kbps: u32) -> Option<f64> {
        self.frames
            .iter()
            .find(|f| f.at >= from && f.kbps >= kbps)
            .map(|f| f.at)
    }
}

/// One row of [`field_report`]: a named run and what it did in 5 minutes.
fn report_row(name: &str, mut sim: Sim, seconds: f64) {
    sim.run(seconds);
    let r = sim.report(0., seconds);
    println!(
        "| {name} | {} | {} | {:.0} % | {:.0} | {:.0} | {:.2} % |",
        r.cuts,
        r.failed_probes,
        r.over_link * 100.,
        r.p95_queue_ms,
        r.delivered_mbps,
        r.frame_loss * 100.
    );
}

fn queued(mut link: Link, ms: f64, delay: f64) -> Link {
    link.queue_ms = ms;
    link.delay = delay;
    link
}

/// The before-and-after table of the 2026-11-17 review, 5 minutes a run:
/// `cargo test -p butterpollo-core --release --lib field_report -- --ignored
/// --nocapture`. Only `report`, `changes`, `frames` and `queue_log` are read,
/// so the file also runs against an older controller.
#[test]
#[ignore]
fn field_report() {
    const RUN: f64 = 300.;
    println!(
        "| run | cuts | failed probes | over link | p95 queue ms | delivered Mbps | lost frames |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    // The field: 4K120 PyroWave asking 800 Mbps (about 680 on the wire) on a
    // path good for 640, behind a deep buffer (8 ms clean, 270 ms full), with
    // a 1 ms report delay.
    let field = || Sim::pyrowave_4k120(queued(Link::constant(640.), 270., 0.003), 800_000, 1.);
    report_row("4K120 PyroWave 800 on 640, 270 ms buffer", field(), RUN);
    let mut refusing = field();
    refusing.refuse_above_ms = Some(100.);
    report_row(
        "same, host socket refuses while the queue is over 100 ms",
        refusing,
        RUN,
    );
    let mut small = Sim::pyrowave_no_room(queued(Link::constant(200.), 50., 0.002), 350_000, 20.);
    small.spread = 0.4;
    report_row("1080p120 PyroWave 350 on 200 (no room), 50 ms", small, RUN);
    let mut room = Sim::pyrowave(queued(Link::constant(500.), 50., 0.002), 900_000, 20.);
    room.spread = 0.4;
    report_row("1080p60 PyroWave 900 on 500, 50 ms", room, RUN);
    report_row(
        "H.26x 75 on 50, 50 ms",
        Sim::new(queued(Link::constant(50.), 50., 0.002), 75_000, 20.),
        RUN,
    );
    report_row(
        "H.26x 75 on 50, 400 ms buffer",
        Sim::new(queued(Link::constant(50.), 400., 0.002), 75_000, 20.),
        RUN,
    );
    let mut clean = Sim::pyrowave(queued(Link::constant(1000.), 50., 0.002), 800_000, 20.);
    clean.spread = 0.4;
    report_row("1 GbE, PyroWave 800", clean, RUN);
    report_row(
        "1 GbE, H.26x 80",
        Sim::new(queued(Link::constant(1000.), 50., 0.002), 80_000, 2.),
        RUN,
    );
    let mut blip = Sim::new(
        queued(Link::step(80., 10., 20., 40.), 50., 0.002),
        40_000,
        20.,
    );
    blip.run(RUN);
    let back = blip.reaches(40., 36_000).map_or(-1., |t| t - 40.);
    let r = blip.report(0., RUN);
    println!(
        "| Wi-Fi blip, H.26x 40 on 80 -> 10 -> 80 Mbps (20-40 s), back to 90 % {back:.1} s after | {} | {} | {:.0} % | {:.0} | {:.0} | {:.2} % |",
        r.cuts,
        r.failed_probes,
        r.over_link * 100.,
        r.p95_queue_ms,
        r.delivered_mbps,
        r.frame_loss * 100.
    );
    let mut twice = Sim::new(
        queued(
            Link::new(|t| {
                if (20. ..40.).contains(&t) || (120. ..140.).contains(&t) {
                    10e6
                } else {
                    80e6
                }
            }),
            50.,
            0.002,
        ),
        40_000,
        20.,
    );
    twice.run(RUN);
    let first = twice.reaches(40., 36_000).map_or(-1., |t| t - 40.);
    let second = twice.reaches(140., 36_000).map_or(-1., |t| t - 140.);
    let r = twice.report(0., RUN);
    println!(
        "| Two Wi-Fi blips, H.26x 40 on 80 -> 10 -> 80 Mbps (20-40 s, 120-140 s), back to 90 % {first:.1} s and {second:.1} s after | {} | {} | {:.0} % | {:.0} | {:.0} | {:.2} % |",
        r.cuts,
        r.failed_probes,
        r.over_link * 100.,
        r.p95_queue_ms,
        r.delivered_mbps,
        r.frame_loss * 100.
    );
    let mut blip = Sim::pyrowave(
        queued(Link::step(1000., 150., 60., 90.), 50., 0.002),
        400_000,
        20.,
    );
    blip.spread = 0.4;
    blip.run(RUN);
    let back = blip.reaches(90., 360_000).map_or(-1., |t| t - 90.);
    let r = blip.report(0., RUN);
    println!(
        "| Wi-Fi blip, PyroWave 400 on 1000 -> 150 -> 1000 Mbps (60-90 s), back to 90 % {back:.1} s after | {} | {} | {:.0} % | {:.0} | {:.0} | {:.2} % |",
        r.cuts,
        r.failed_probes,
        r.over_link * 100.,
        r.p95_queue_ms,
        r.delivered_mbps,
        r.frame_loss * 100.
    );
}
