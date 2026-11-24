//! Single-pass splice and headroom tests for [`Dynamic`].
use super::*;
use std::borrow::Cow;

fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![0, 0, 0, 1, kind << 1, 0x01];
    unit.extend(body);
    unit
}

fn hevc_frame() -> Vec<u8> {
    [
        nal(32, &[0x0C]),
        nal(33, &[0x01]),
        nal(34, &[0xC1]),
        nal(1, &[0xAF, 0x12, 0x34]),
    ]
    .concat()
}

fn av1_frame() -> Vec<u8> {
    [
        vec![0x12, 0x00],
        vec![0x0A, 0x01, 0x00],
        vec![0x32, 0x01, 0x10],
    ]
    .concat()
}

fn sample_readout(conversion: u64) -> HdrStatsReadout {
    let mut histogram = vec![0; crate::compute::HDR_STATS_BINS];
    histogram[100..=700].fill(10);
    let edge = |bin: usize| bin as f32 / crate::compute::HDR_STATS_BINS as f32;
    HdrStatsReadout {
        conversion,
        histogram,
        min_pq: edge(100),
        max_pq: edge(700),
    }
}

#[test]
fn output_cow_matches_output_for_all_hdr_and_dolby_vision_modes() {
    let cases = [
        (
            Wanted {
                codec: Codec::Hevc,
                t35: true,
                dolby_vision: false,
                hlg: false,
            },
            hevc_frame(),
        ),
        (
            Wanted {
                codec: Codec::Hevc,
                t35: false,
                dolby_vision: true,
                hlg: false,
            },
            hevc_frame(),
        ),
        (
            Wanted {
                codec: Codec::Hevc,
                t35: true,
                dolby_vision: true,
                hlg: false,
            },
            hevc_frame(),
        ),
        (
            Wanted {
                codec: Codec::Hevc,
                t35: true,
                dolby_vision: true,
                hlg: true,
            },
            hevc_frame(),
        ),
        (
            Wanted {
                codec: Codec::Av1,
                t35: true,
                dolby_vision: false,
                hlg: false,
            },
            av1_frame(),
        ),
        (
            Wanted {
                codec: Codec::Av1,
                t35: false,
                dolby_vision: true,
                hlg: false,
            },
            av1_frame(),
        ),
        (
            Wanted {
                codec: Codec::Av1,
                t35: true,
                dolby_vision: true,
                hlg: false,
            },
            av1_frame(),
        ),
    ];

    for (wanted, frame) in cases {
        let mut owned_dyn = Dynamic::with(wanted, true);
        let mut borrowed_dyn = Dynamic::with(wanted, true);
        owned_dyn.take(Ok(Some(sample_readout(1))), Instant::now());
        borrowed_dyn.take(Ok(Some(sample_readout(1))), Instant::now());

        let expected = output(
            Some(&mut owned_dyn),
            Unit::Whole,
            frame.clone(),
            Some(5000),
            false,
        );
        let actual = output_cow(
            Some(&mut borrowed_dyn),
            Unit::Whole,
            Cow::Borrowed(&frame),
            Some(5000),
            false,
        );
        assert_eq!(actual, expected, "mismatch for {wanted:?}");
        assert!(
            borrowed_dyn.headroom() >= actual.0.len() - frame.len(),
            "headroom must cover inserted metadata for {wanted:?}"
        );

        // Owned buffer allocated with `headroom()` splices without reallocating.
        let mut room_dyn = Dynamic::with(wanted, true);
        room_dyn.take(Ok(Some(sample_readout(1))), Instant::now());
        let with_room = crate::frame_pool::copy_with_headroom(&frame, room_dyn.headroom());
        let ptr = with_room.as_ptr();
        let (spliced, cap) = output_cow(
            Some(&mut room_dyn),
            Unit::Whole,
            Cow::Owned(with_room),
            Some(5000),
            false,
        );
        assert_eq!(
            spliced.as_ptr(),
            ptr,
            "no realloc within headroom for {wanted:?}"
        );
        assert_eq!((spliced, cap), expected);
    }
}

#[test]
fn output_cow_handles_empty_and_no_insertion_frames_identically() {
    let wanted = Wanted {
        codec: Codec::Hevc,
        t35: true,
        dolby_vision: true,
        hlg: false,
    };
    let mut a = Dynamic::with(wanted, false);
    let mut b = Dynamic::with(wanted, false);
    assert_eq!(
        output_cow(
            Some(&mut a),
            Unit::Whole,
            Cow::Borrowed(&[]),
            Some(5000),
            false
        ),
        output(Some(&mut b), Unit::Whole, Vec::new(), Some(5000), false),
    );
    let bare = nal(32, &[0x0C]);
    assert_eq!(
        output_cow(
            Some(&mut a),
            Unit::Whole,
            Cow::Borrowed(&bare),
            Some(5000),
            false
        ),
        output(Some(&mut b), Unit::Whole, bare, Some(5000), false),
    );
}
