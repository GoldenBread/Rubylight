//! Property tests: every statistics the analysis could report, and every frame an encoder
//! could hand over, through the independent parsers in the parent test module.

use super::*;
use proptest::collection::vec;
use proptest::prelude::*;

fn unit() -> impl Strategy<Value = f32> {
    prop_oneof![0f32..=1., Just(0.), Just(1.)]
}

prop_compose! {
    fn stats()(
        min_pq in unit(),
        mean_pq in unit(),
        max_pq in unit(),
        mean_nits in prop_oneof![0f32..=10_000., Just(10_000.)],
        percentiles_pq in proptest::array::uniform9(unit()),
    ) -> FrameStats {
        FrameStats { min_pq, mean_pq, max_pq, mean_nits, percentiles_pq }
    }
}

/// Statistics with one value out of range or not a number.
fn broken() -> impl Strategy<Value = FrameStats> {
    let bad = prop_oneof![
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(-0.01f32),
        Just(1.01f32)
    ];
    (stats(), bad, 0usize..13).prop_map(|(mut s, bad, at)| {
        match at {
            0 => s.min_pq = bad,
            1 => s.mean_pq = bad,
            2 => s.max_pq = bad,
            3 => s.mean_nits = if bad.is_finite() { bad * 20_000. } else { bad },
            _ => s.percentiles_pq[at - 4] = bad,
        }
        s
    })
}

proptest! {
    #[test]
    fn hdr10plus_is_profile_a_and_carries_its_fields(stats in stats()) {
        let metadata = Hdr10Plus::from_stats(&stats).unwrap();
        let t35 = metadata.to_t35();
        assert_profile_a(&t35);
        let parsed = parse_hdr10plus(&t35);
        prop_assert_eq!(parsed.maxscl, [metadata.maxscl; 3]);
        prop_assert_eq!(parsed.average_maxrgb, metadata.average_maxrgb);
        let values: Vec<u32> = parsed.distribution.iter().map(|(_, v)| *v).collect();
        prop_assert_eq!(values, metadata.distribution.to_vec());
    }

    #[test]
    fn hdr10plus_fields_round_trip(
        maxscl in 0u32..=100_000,
        average_maxrgb in 0u32..=100_000,
        distribution in proptest::array::uniform9(0u32..=100_000),
    ) {
        let metadata = Hdr10Plus { maxscl, average_maxrgb, distribution };
        let parsed = parse_hdr10plus(&metadata.to_t35());
        prop_assert_eq!(parsed.maxscl, [maxscl; 3]);
        prop_assert_eq!(parsed.average_maxrgb, average_maxrgb);
        let values: Vec<u32> = parsed.distribution.iter().map(|(_, v)| *v).collect();
        prop_assert_eq!(values, distribution.to_vec());
    }

    #[test]
    fn vivid_fields_round_trip(codes in proptest::array::uniform4(0u16..=4095), hlg in any::<bool>()) {
        let metadata = Vivid {
            minimum_maxrgb: codes[0],
            average_maxrgb: codes[1],
            variance_maxrgb: codes[2],
            maximum_maxrgb: codes[3],
        };
        let t35 = if hlg { metadata.to_t35_hlg() } else { metadata.to_t35() };
        prop_assert_eq!(t35[5], if hlg { 2 } else { 1 });
        prop_assert_eq!(parse_vivid(&t35), codes.map(u32::from));
    }

    #[test]
    fn vivid_from_stats_stays_within_twelve_bits(stats in stats()) {
        let metadata = Vivid::from_stats(&stats).unwrap();
        let parsed = parse_vivid(&metadata.to_t35());
        prop_assert!(parsed.iter().all(|&code| code <= 4095));
        prop_assert_eq!(parsed, [
            metadata.minimum_maxrgb,
            metadata.average_maxrgb,
            metadata.variance_maxrgb,
            metadata.maximum_maxrgb,
        ].map(u32::from));
    }

    #[test]
    fn invalid_statistics_write_nothing(stats in broken()) {
        prop_assert!(!stats.is_valid());
        prop_assert_eq!(Hdr10Plus::from_stats(&stats), None);
        prop_assert_eq!(Vivid::from_stats(&stats), None);
        prop_assert_eq!(frame_units(Codec::Hevc, &stats), None);
        prop_assert_eq!(frame_units(Codec::Av1, &stats), None);
    }

    /// Any T.35 payload, of any size and with any start code prefixes inside it, comes
    /// back out of the SEI and the OBU unchanged.
    #[test]
    fn t35_payloads_survive_their_containers(
        t35 in prop_oneof![
            vec(any::<u8>(), 0..600),
            vec(prop_oneof![Just(0u8), Just(1u8), Just(3u8), any::<u8>()], 0..64),
        ],
    ) {
        prop_assert_eq!(parse_hevc_sei(&hevc_t35_sei(&t35)), vec![(4, t35.clone())]);
        prop_assert_eq!(parse_av1_metadata(&av1_t35_obu(&t35)), (4, t35));
    }

    /// Arbitrary frames never panic the walk, and units land whole at the offset.
    #[test]
    fn splicing_any_frame_inserts_at_the_offset(
        frame in prop_oneof![
            vec(prop_oneof![Just(0u8), Just(1u8), any::<u8>()], 0..96),
            vec((0u8..64, vec(any::<u8>(), 0..12)), 0..6).prop_map(|units| {
                units.into_iter().flat_map(|(kind, body)| nal(kind, &body)).collect()
            }),
            vec((0u8..16, vec(any::<u8>(), 0..12)), 0..6).prop_map(|units| {
                units.into_iter().flat_map(|(kind, body)| obu(kind, &body)).collect()
            }),
        ],
        units in vec(any::<u8>(), 0..16),
        av1 in any::<bool>(),
    ) {
        let codec = if av1 { Codec::Av1 } else { Codec::Hevc };
        let offset = insert_offset(codec, &frame);
        if let Some(at) = offset {
            prop_assert!(at <= frame.len());
        }
        let copy = spliced(codec, &frame, &units);
        let mut in_place = frame.clone();
        let changed = splice(codec, &mut in_place, &units);
        match (offset, units.is_empty()) {
            (Some(at), false) => {
                let mut expected = frame[..at].to_vec();
                expected.extend_from_slice(&units);
                expected.extend_from_slice(&frame[at..]);
                prop_assert!(changed);
                prop_assert_eq!(&in_place, &expected);
                prop_assert_eq!(copy, Some(expected));
            }
            _ => {
                prop_assert!(!changed);
                prop_assert_eq!(in_place, frame);
                prop_assert_eq!(copy, None);
            }
        }
    }
}
