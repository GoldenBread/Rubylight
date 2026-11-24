//! Tests for FEC telemetry, frame latency stages, and stutter reporting.

use super::*;
fn fec(frame: u32, block: u8, data: u16, parity: u16, missing: u16) -> Vec<u8> {
    let mut payload = frame.to_be_bytes().to_vec();
    for word in [11, 0, missing, 10, 2, data, parity] {
        payload.extend(word.to_be_bytes());
    }
    payload.extend([20, block, 2]);
    payload
}
#[test]
fn fec_reports_count_frames_once_and_failed_blocks_override_recovery() {
    let mut p = Performance::default();
    let now = Instant::now();
    use crate::adaptive_fec::Block;
    let recovered = fec(7, 0, 9, 1, 1);
    assert_eq!(
        p.record_fec_status(&recovered, Some(9)),
        Some(Block::Recovered { lost: 1, data: 10 })
    );
    // A duplicate is counted, but not passed on twice.
    assert_eq!(p.record_fec_status(&recovered, Some(9)), None);
    assert_eq!(p.snapshot(now)["fec_recovered_frames"], 1);
    assert_eq!(
        p.record_fec_status(&fec(7, 1, 7, 1, 2), Some(9)),
        Some(Block::Unrecoverable)
    );
    assert_eq!(
        p.record_fec_status(&fec(8, 0, 10, 0, 0), Some(9)),
        Some(Block::Clean)
    );
    p.record_fec_status(&fec(9, 1, 8, 2, 2), Some(9));
    p.record_fec_status(&fec(9, 0, 9, 1, 1), Some(9));
    let snapshot = p.snapshot(now);
    assert_eq!(snapshot["fec_reports"], 6);
    assert_eq!(snapshot["fec_invalid_reports"], 0);
    assert_eq!(snapshot["fec_duplicate_reports"], 1);
    assert_eq!(snapshot["fec_recovered_frames"], 1);
    assert_eq!(snapshot["fec_unrecoverable_frames"], 1);
    assert_eq!(snapshot["fec_missing_packets"], 6);
    // Out-of-order reports must not reclassify a lost frame as recovered.
    p.record_fec_status(&fec(10, 1, 7, 1, 2), Some(10));
    p.record_fec_status(&fec(10, 0, 9, 1, 1), Some(10));
    assert_eq!(p.snapshot(now)["fec_recovered_frames"], 1);
    assert_eq!(p.snapshot(now)["fec_unrecoverable_frames"], 2);
    assert_eq!(p.snapshot(now)["sample_frames"], 0);
}
#[test]
fn fec_telemetry_rejects_invalid_unsent_stale_and_inconsistent_reports() {
    let mut p = Performance::default();
    p.record_fec_status(&fec(u32::MAX, 0, 9, 1, 1), Some(0));
    p.record_fec_status(&fec(0, 0, 9, 1, 1), Some(0));
    for (payload, last) in [
        (vec![0; 20], Some(0)),
        (fec(1, 0, 9, 1, 1), Some(0)),
        (fec(1, 0, 9, 1, 1), None),
        (fec(1, 0, 9, 1, 1), Some(1025)),
    ] {
        assert_eq!(p.record_fec_status(&payload, last), None);
    }
    let mut inconsistent = fec(0, 0, 9, 1, 1);
    inconsistent[20] = 1;
    assert_eq!(p.record_fec_status(&inconsistent, Some(0)), None);
    assert_eq!(p.fec_invalid_reports, 5);
    assert_eq!(p.fec_recovered_frames, 2);
    assert_eq!(p.fec_missing_packets, 2);
}
#[test]
fn reordered_fec_reports_evict_stale_frames_without_recounting_recent_ones() {
    let mut p = Performance::default();
    p.record_fec_status(&fec(1024, 0, 9, 1, 1), Some(1024));
    for frame in 1..1024 {
        p.record_fec_status(&fec(frame, 0, 10, 0, 0), Some(1024));
    }
    p.record_fec_status(&fec(1025, 0, 10, 0, 0), Some(1025));
    p.record_fec_status(&fec(1024, 0, 9, 1, 1), Some(1025));
    assert_eq!(p.fec_frames.len, 1024);
    assert_eq!(p.fec_recovered_frames, 1);
    assert_eq!(p.fec_missing_packets, 1);
    assert_eq!(p.fec_duplicate_reports, 1);
}
#[test]
fn fec_history_and_counters_are_bounded() {
    let mut p = Performance::default();
    for frame in 0..1500 {
        p.record_fec_status(&fec(frame, 0, 10, 0, 0), Some(frame));
    }
    assert_eq!(p.fec_frames.len, 1024);
    assert_eq!(p.fec_recovered_frames, 0);
    assert_eq!(p.fec_missing_packets, 0);
    p.fec_reports = u64::MAX;
    p.fec_invalid_reports = u64::MAX;
    p.record_fec_status(&[], Some(1500));
    assert_eq!(p.fec_reports, u64::MAX);
    assert_eq!(p.fec_invalid_reports, u64::MAX);
    p.fec_missing_packets = u64::MAX;
    p.record_fec_status(&fec(1500, 0, 9, 1, 1), Some(1500));
    assert_eq!(p.fec_missing_packets, u64::MAX);
}
/// The block bookkeeping as it was before [`FecFrames`]: a `retain` and a
/// search over every kept frame for each report.
#[derive(Default)]
struct RetainedFrames {
    frames: Vec<FecFrame>,
    invalid: u64,
    duplicate: u64,
    recovered: u64,
    unrecoverable: u64,
    missing: u64,
}
impl RetainedFrames {
    fn record(
        &mut self,
        payload: &[u8],
        last_sent: Option<u32>,
    ) -> Option<crate::adaptive_fec::Block> {
        let status = rubylight_wire::fec_status::Status::parse(payload)
            .ok()
            .zip(last_sent)
            .filter(|(s, last)| last.wrapping_sub(s.frame) < 1024);
        let Some((status, last)) = status else {
            self.invalid += 1;
            return None;
        };
        self.frames.retain(|f| last.wrapping_sub(f.index) < 1024);
        let index = match self.frames.iter().position(|f| f.index == status.frame) {
            Some(index) => index,
            None => {
                self.frames.push(FecFrame {
                    index: status.frame,
                    blocks: 0,
                    block_count: status.blocks,
                    recovered: false,
                    unrecoverable: false,
                });
                self.frames.len() - 1
            }
        };
        let frame = &mut self.frames[index];
        if frame.block_count != status.blocks {
            self.invalid += 1;
            return None;
        }
        let block = 1 << status.block;
        if frame.blocks & block != 0 {
            self.duplicate += 1;
            return None;
        }
        frame.blocks |= block;
        self.missing += u64::from(status.missing_before_highest);
        let unrecoverable = u32::from(status.received_data) + u32::from(status.received_parity)
            < u32::from(status.data);
        if unrecoverable && !frame.unrecoverable {
            frame.unrecoverable = true;
            self.unrecoverable += 1;
            if frame.recovered {
                self.recovered -= 1;
            }
        } else if !unrecoverable
            && status.received_data < status.data
            && !frame.recovered
            && !frame.unrecoverable
        {
            frame.recovered = true;
            self.recovered += 1;
        }
        Some(if unrecoverable {
            crate::adaptive_fec::Block::Unrecoverable
        } else if status.received_data < status.data {
            crate::adaptive_fec::Block::Recovered {
                lost: status.data - status.received_data,
                data: status.data,
            }
        } else {
            crate::adaptive_fec::Block::Clean
        })
    }
}
#[test]
fn fec_frame_slots_report_what_retaining_every_frame_did() {
    let mut p = Performance::default();
    let mut reference = RetainedFrames::default();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = |below: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % u64::from(below)) as u32
    };
    // Close to the u32 counter's wrap, which the run passes.
    let mut last = u32::MAX - 3000;
    for report in 0..40_000 {
        // Mostly a frame or none per report, sometimes a jump of a window or
        // more, and now and then backwards.
        last = match random(200) {
            0 => last.wrapping_add(1500 + random(3000)),
            1 => last.wrapping_sub(1 + random(40)),
            2 => last.wrapping_sub(1100 + random(3000)),
            3..=60 => last,
            61..=70 => last.wrapping_add(2 + random(30)),
            _ => last.wrapping_add(1),
        };
        // Up to a little older than the window, and now and then not sent yet.
        let frame = if random(50) == 0 {
            last.wrapping_add(1 + random(5))
        } else {
            last.wrapping_sub(random(1100))
        };
        // Ten data packets: some lost and repaired, some lost for good, and
        // now and then a count the parser refuses.
        let received = 8 + random(4) as u16;
        let mut payload = fec(
            frame,
            random(2) as u8,
            received,
            random(3) as u16,
            random(3) as u16,
        );
        // A frame reported with another block count than before is invalid.
        payload[20] = 1 + u8::from(random(8) != 0);
        let last_sent = (random(500) != 0).then_some(last);
        assert_eq!(
            p.record_fec_status(&payload, last_sent),
            reference.record(&payload, last_sent),
            "report {report}"
        );
    }
    assert_eq!(p.fec_invalid_reports, reference.invalid);
    assert_eq!(p.fec_duplicate_reports, reference.duplicate);
    assert_eq!(p.fec_recovered_frames, reference.recovered);
    assert_eq!(p.fec_unrecoverable_frames, reference.unrecoverable);
    assert_eq!(p.fec_missing_packets, reference.missing);
    assert!(reference.recovered > 0 && reference.unrecoverable > 0);
    assert!(reference.duplicate > 0 && reference.invalid > 0);
    // The same frames are kept, each as the reference keeps it.
    assert_eq!(p.fec_frames.len, reference.frames.len());
    for kept in &reference.frames {
        let slot = p.fec_frames.slots[(kept.index % FEC_WINDOW) as usize].as_ref();
        let slot = slot.expect("a kept frame has its slot");
        assert_eq!(
            (slot.index, slot.blocks, slot.block_count),
            (kept.index, kept.blocks, kept.block_count)
        );
        assert_eq!(
            (slot.recovered, slot.unrecoverable),
            (kept.recovered, kept.unrecoverable)
        );
    }
}
/// A picture's stamps from the nine stage times, in microseconds.
fn stamped(start: Instant, stages: [u64; 9]) -> Stamps {
    let mut points = [start; 10];
    for (index, us) in stages.into_iter().enumerate() {
        points[index + 1] = points[index] + Duration::from_micros(us);
    }
    Stamps {
        presented: points[0],
        acquired: points[1],
        published: points[2],
        claimed: points[3],
        submitted: points[4],
        encoded: points[5],
        packetized: points[6],
        prepared: points[7],
        first_sent: points[8],
        last_sent: points[9],
        streamed: None,
    }
}

mod stages;
mod stutter;
