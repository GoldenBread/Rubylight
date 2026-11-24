//! Cauchy Reed-Solomon GF(2^8), AVX2, and GFNI parity kernel tests.

use super::*;

#[test]
fn fec_excludes_the_reserved_envelope_and_preserves_all_row_tails() {
    for (data, parity, size) in [
        (1, 2, 0),
        (8, 4, 31),
        (28, 7, 1392),
        (192, 39, 1408),
        (254, 1, 1416),
    ] {
        let mut expected: Vec<Vec<u8>> = (0..data + parity)
            .map(|row| (0..size).map(|i| (row * 73 + i * 29) as u8).collect())
            .collect();
        let mut actual: Vec<Vec<u8>> = expected
            .iter()
            .map(|bytes| {
                let mut packet = vec![0xAB; 32];
                packet.extend_from_slice(bytes);
                packet
            })
            .collect();
        cauchy_encode(&mut expected, data, parity).unwrap();
        cauchy_encode_offset::<32>(&mut actual, data, parity).unwrap();
        for (packet, bytes) in actual.iter().zip(expected) {
            assert_eq!(&packet[..32], &[0xAB; 32]);
            assert_eq!(&packet[32..], bytes);
        }
    }
    assert!(cauchy_encode_offset::<32>(&mut [vec![0; 31], vec![0; 31]], 1, 1).is_err());
}

#[test]
fn simd_cauchy_matches_scalar_for_every_coefficient_and_tail() {
    for coefficient in 0..=255 {
        for len in [1, 15, 16, 17, 31, 33, 1023] {
            let source: Vec<u8> = (0..len).map(|i| (i * 71 + 13) as u8).collect();
            let mut out = vec![0x55; len];
            axpy(&mut out, &source, coefficient);
            let expected: Vec<u8> = source
                .iter()
                .map(|v| 0x55 ^ gf_mul(*v, coefficient))
                .collect();
            assert_eq!(out, expected);
        }
    }
}
#[test]
fn batched_cauchy_matches_independent_scalar_for_row_groups_and_short_tails() {
    for (data, parity) in [
        (1, 254),
        (254, 1),
        (4, 2),
        (4, 3),
        (4, 4),
        (4, 5),
        (4, 6),
        (4, 7),
        (32, 7),
        (96, 20),
        (192, 39),
    ] {
        for length in [0, 1, 15, 31, 32, 33, 63, 65, 1416] {
            let mut shards: Vec<Vec<u8>> = (0..data + parity)
                .map(|i| (0..length).map(|j| (i * 29 + j * 71 + 13) as u8).collect())
                .collect();
            let mut expected = shards.clone();
            for (row, out) in expected[data..].iter_mut().enumerate() {
                out.fill(0);
                for (column, input) in shards[..data].iter().enumerate() {
                    let coefficient = inverse((parity + column) as u8 ^ row as u8);
                    for (out, input) in out.iter_mut().zip(input) {
                        *out ^= gf_mul(*input, coefficient);
                    }
                }
            }
            cauchy_encode(&mut shards, data, parity).unwrap();
            assert_eq!(shards, expected, "matrix {data}+{parity}, {length} bytes");
        }
    }
}

/// `gf2p8affineqb` as Intel defines it: output bit `i` is the parity of the
/// input byte and the matrix's byte `7 - i`.
fn affine(matrix: u64, value: u8) -> u8 {
    (0..8).fold(0, |out, bit| {
        let row = (matrix >> (8 * (7 - bit))) as u8;
        out | ((((row & value).count_ones() & 1) as u8) << bit)
    })
}

/// The GFNI matrices multiply in nanors' field, on any CPU.
#[test]
fn the_gfni_matrices_multiply_in_the_fec_field() {
    for coefficient in 0..=255u8 {
        for value in 0..=255u8 {
            assert_eq!(
                affine(AFFINE[coefficient as usize], value),
                gf_mul(coefficient, value),
                "{coefficient} * {value}"
            );
        }
    }
}

/// GFNI parity is bit-exact with the AVX2 parity, for every row group, both
/// offsets and lengths with and without a tail. Skipped on a CPU without
/// GFNI (Zen 3); Zen 4 and Intel Ice Lake or newer run it.
#[cfg(target_arch = "x86_64")]
#[test]
fn gfni_parity_matches_the_avx2_parity() {
    if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("gfni")) {
        eprintln!("skipped: this CPU has no GFNI");
        return;
    }
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for (data, parity, size) in [
        (1, 1, 40),
        (5, 3, 47),
        (37, 7, 1072),
        (200, 55, 1040),
        (212, 43, 1440),
        (10, 6, 1040 + 33),
    ] {
        let shards: Vec<Vec<u8>> = (0..data + parity)
            .map(|_| (0..size).map(|_| random()).collect())
            .collect();
        let (mut with, mut without) = (shards.clone(), shards.clone());
        cauchy_encode_with::<0>(&mut with, data, parity, true).unwrap();
        cauchy_encode_with::<0>(&mut without, data, parity, false).unwrap();
        assert_eq!(with, without, "{data}+{parity} x {size}");
        let (mut with, mut without) = (shards.clone(), shards);
        cauchy_encode_with::<32>(&mut with, data, parity, true).unwrap();
        cauchy_encode_with::<32>(&mut without, data, parity, false).unwrap();
        assert_eq!(with, without, "{data}+{parity} x {size}, offset 32");
    }
}

/// The GFNI path's arithmetic, emulated in software on any CPU: parity built
/// from the per-coefficient matrices matches the shipped parity, with and
/// without the 32-byte offset.
#[test]
fn parity_from_the_gfni_matrices_matches_the_shipped_parity() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    fn emulated<const OFFSET: usize>(shards: &mut [Vec<u8>], data: usize, parity: usize) {
        let (source, dest) = shards.split_at_mut(data);
        for (row, out) in dest.iter_mut().enumerate() {
            out[OFFSET..].fill(0);
            for (i, input) in source.iter().enumerate() {
                let matrix = AFFINE[INVERSES[((parity + i) as u8 ^ row as u8) as usize] as usize];
                for (out, &value) in out[OFFSET..].iter_mut().zip(&input[OFFSET..]) {
                    *out ^= affine(matrix, value);
                }
            }
        }
    }
    for (data, parity, size) in [(1, 1, 40), (5, 3, 47), (37, 7, 1072), (200, 55, 1040)] {
        let shards: Vec<Vec<u8>> = (0..data + parity)
            .map(|_| (0..size).map(|_| random()).collect())
            .collect();
        let (mut shipped, mut matrices) = (shards.clone(), shards.clone());
        cauchy_encode_with::<0>(&mut shipped, data, parity, false).unwrap();
        emulated::<0>(&mut matrices, data, parity);
        assert_eq!(matrices, shipped, "{data}+{parity} x {size}");
        let (mut shipped, mut matrices) = (shards.clone(), shards);
        cauchy_encode_with::<32>(&mut shipped, data, parity, false).unwrap();
        emulated::<32>(&mut matrices, data, parity);
        assert_eq!(matrices, shipped, "{data}+{parity} x {size}, offset 32");
    }
}

/// Parity by plain field products, one per byte, with the coefficient the
/// encoder uses for each (source, row) pair: the reference for the kernels.
#[cfg(target_arch = "x86_64")]
fn scalar_reference<const OFFSET: usize>(
    shards: &mut [Vec<u8>],
    data: usize,
    parity: usize,
    products: &[[u8; 256]],
) {
    let (source, dest) = shards.split_at_mut(data);
    for (row, out) in dest.iter_mut().enumerate() {
        out[OFFSET..].fill(0);
        for (column, input) in source.iter().enumerate() {
            let coefficient = INVERSES[(parity + column) ^ row] as usize;
            for (out, &value) in out[OFFSET..].iter_mut().zip(&input[OFFSET..]) {
                *out ^= products[coefficient][value as usize];
            }
        }
    }
}

/// The AVX2 and GFNI kernels, with and without GFNI requested, against
/// [`scalar_reference`] for each shard length in `lengths`.
#[cfg(target_arch = "x86_64")]
fn assert_parity_matches_the_scalar_reference<const OFFSET: usize>(
    lengths: impl IntoIterator<Item = usize>,
    data: usize,
    parity: usize,
    random: &mut impl FnMut() -> u8,
) {
    let products: Vec<[u8; 256]> = (0..=255u8)
        .map(|coefficient| std::array::from_fn(|value| gf_mul(coefficient, value as u8)))
        .collect();
    for size in lengths {
        let shards: Vec<Vec<u8>> = (0..data + parity)
            .map(|_| (0..size).map(|_| random()).collect())
            .collect();
        let mut expected = shards.clone();
        scalar_reference::<OFFSET>(&mut expected, data, parity, &products);
        for gfni in [false, true] {
            let mut actual = shards.clone();
            cauchy_encode_with::<OFFSET>(&mut actual, data, parity, gfni).unwrap();
            assert!(
                actual == expected,
                "{data}+{parity} x {size}, offset {OFFSET}, gfni {gfni}"
            );
        }
    }
}

/// Every row group size (4, and 1 to 3 for the rest), every tail (32-byte
/// chunks, then 16, 8 and single bytes), at both offsets, for every shard
/// length from 1 to 1500 (from 32 with the offset, which every shard holds).
#[cfg(target_arch = "x86_64")]
#[test]
fn parity_is_byte_identical_to_the_scalar_reference_for_every_length() {
    let mut state = 0x6a09_e667_f3bc_c908u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for (data, parity) in [
        (1, 1),
        (3, 2),
        (5, 1),
        (5, 2),
        (5, 3),
        (5, 4),
        (5, 5),
        (4, 7),
    ] {
        assert_parity_matches_the_scalar_reference::<0>(1..=1500, data, parity, &mut random);
        assert_parity_matches_the_scalar_reference::<32>(32..=1500, data, parity, &mut random);
    }
}

/// The Moonlight wire sizes: 1040-byte shards (default 1024-byte packets, 16
/// left over after the 32-byte chunks) and 1408 (1392-byte packets, none left),
/// with and without the 32-byte offset, and a large frame's block.
#[cfg(target_arch = "x86_64")]
#[test]
fn parity_matches_the_scalar_reference_at_the_wire_sizes() {
    let mut state = 0x243f_6a88_85a3_08d3u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for (data, parity) in [(32, 7), (96, 20), (200, 55)] {
        assert_parity_matches_the_scalar_reference::<0>(
            [1039, 1040, 1041, 1407, 1408, 1409],
            data,
            parity,
            &mut random,
        );
        assert_parity_matches_the_scalar_reference::<32>(
            [1071, 1072, 1073, 1439, 1440, 1441],
            data,
            parity,
            &mut random,
        );
    }
}
