//! Reed-Solomon: nightjar-core's port against the output nanors recorded (golden/mod.rs).

mod golden;

use nightjar_core::fec::ReedSolomon;
use golden::net::Rng;
use golden::{check, Fnv};

fn shards(rng: &mut Rng, count: usize, size: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|_| (0..size).map(|_| rng.byte()).collect())
        .collect()
}

fn rust_encode(data: usize, set: &mut [Vec<u8>], size: usize) {
    let rs = ReedSolomon::new(data, set.len() - data).unwrap();
    let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
    rs.encode(&mut refs, size).unwrap();
}

fn rust_decode(data: usize, set: &mut [Vec<u8>], missing: &[bool], size: usize) -> bool {
    let rs = ReedSolomon::new(data, set.len() - data).unwrap();
    let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
    rs.decode(&mut refs, missing, size).is_ok()
}

#[test]
fn encode_matches_nanors() {
    let mut rng = Rng::new(1);
    for _ in 0..120 {
        let data = 1 + rng.below(200);
        let parity = 1 + rng.below((255 - data).min(60));
        let size = 1 + rng.below(1200);
        let mut set = shards(&mut rng, data + parity, size);
        let mut key = Fnv::new();
        key.u64(data as u64);
        set.iter().for_each(|s| key.bytes(s));
        rust_encode(data, &mut set, size);
        let mut out = Fnv::new();
        set.iter().for_each(|s| out.bytes(s));
        check("fec-encode", key.finish(), out.finish());
    }
}

#[test]
fn decode_matches_nanors() {
    let mut rng = Rng::new(2);
    for round in 0..160 {
        let data = 1 + rng.below(220);
        let parity = 1 + rng.below((255 - data).min(40));
        let size = 1 + rng.below(700);
        let mut set = shards(&mut rng, data + parity, size);
        // Half the rounds decode consistent codewords, the rest arbitrary bytes.
        if round % 2 == 0 {
            rust_encode(data, &mut set, size);
        }
        let erasures = rng.below(parity + 2);
        let mut missing = vec![false; data + parity];
        for _ in 0..erasures {
            missing[rng.below(data + parity)] = true;
        }
        for (shard, &gone) in set.iter_mut().zip(&missing) {
            if gone {
                shard.fill(0xEE);
            }
        }
        let mut key = Fnv::new();
        key.u64(data as u64);
        key.u64(size as u64);
        set.iter().for_each(|s| key.bytes(s));
        missing.iter().for_each(|&m| key.bytes(&[u8::from(m)]));
        let ok = rust_decode(data, &mut set, &missing, size);
        let mut out = Fnv::new();
        out.bytes(&[u8::from(ok)]);
        if ok {
            set[..data].iter().for_each(|s| out.bytes(s));
        }
        check("fec-decode", key.finish(), out.finish());
    }
}

#[test]
fn rejects_what_nanors_rejects() {
    // Whether nanors took each of these (it rejects 0 or 255+ shards in all, and no data or no parity).
    for (data, parity) in [(0, 1), (1, 0), (200, 56), (255, 1), (254, 1)] {
        let accepted = ReedSolomon::new(data, parity).is_some();
        let recorded = (data * 1000 + parity) as u64;
        check("fec-reject", recorded, u64::from(accepted));
    }
}
