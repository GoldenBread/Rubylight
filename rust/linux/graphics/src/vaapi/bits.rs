//! A most-significant-bit-first writer of raw header bits, and the NAL unit wrapper, shared by
//! the H.264 and HEVC header writers. Ported from the spike's `hevc_headers.rs`.

/// The bits of one header, as ITU-T H.264 and H.265 lay them out.
#[derive(Default)]
pub struct Bits {
    pub bytes: Vec<u8>,
    /// Bits used in the last byte, 0 when it is full or there is none.
    pub used: u32,
}

impl Bits {
    pub fn put(&mut self, count: u32, value: u32) {
        debug_assert!(count <= 32 && (count == 32 || value >> count == 0));
        for shift in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let bit = (value >> shift & 1) as u8;
            *self.bytes.last_mut().expect("a byte was just pushed") |= bit << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }

    pub fn flag(&mut self, value: bool) {
        self.put(1, u32::from(value));
    }

    /// Unsigned Exp-Golomb, `ue(v)`.
    pub fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.put(length - 1, 0);
        // `code` has `length` bits, at most 33; write it in two parts.
        if length > 16 {
            self.put(length - 16, (code >> 16) as u32);
            self.put(16, (code & 0xffff) as u32);
        } else {
            self.put(length, code as u32);
        }
    }

    /// Signed Exp-Golomb, `se(v)`.
    pub fn se(&mut self, value: i32) {
        let mapped = if value > 0 {
            value as u32 * 2 - 1
        } else {
            value.unsigned_abs() * 2
        };
        self.ue(mapped);
    }

    /// `rbsp_trailing_bits()` and `byte_alignment()`: a one, then zeros to the end of the byte.
    pub fn align(&mut self) {
        self.put(1, 1);
        while self.used != 0 {
            self.put(1, 0);
        }
    }
}

/// Wraps header bits as a NAL unit in a byte stream: start code, the NAL header bytes, and the
/// payload with emulation prevention bytes inserted.
pub fn nal(header: &[u8], payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(header);
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(write: impl FnOnce(&mut Bits)) -> (Vec<u8>, u32) {
        let mut b = Bits::default();
        write(&mut b);
        (b.bytes, b.used)
    }

    #[test]
    fn exp_golomb_matches_the_standard_table() {
        // H.265 table 9-2: 0 -> 1, 1 -> 010, 2 -> 011, 3 -> 00100, 7 -> 0001000.
        assert_eq!(bits(|b| b.ue(0)), (vec![0b1000_0000], 1));
        assert_eq!(bits(|b| b.ue(1)), (vec![0b0100_0000], 3));
        assert_eq!(bits(|b| b.ue(2)), (vec![0b0110_0000], 3));
        assert_eq!(bits(|b| b.ue(3)), (vec![0b0010_0000], 5));
        assert_eq!(bits(|b| b.ue(7)), (vec![0b0001_0000], 7));
        // Table 9-3: 1 -> ue 1, -1 -> ue 2, 2 -> ue 3, -2 -> ue 4.
        assert_eq!(bits(|b| b.se(1)), bits(|b| b.ue(1)));
        assert_eq!(bits(|b| b.se(-1)), bits(|b| b.ue(2)));
        assert_eq!(bits(|b| b.se(2)), bits(|b| b.ue(3)));
        assert_eq!(bits(|b| b.se(-2)), bits(|b| b.ue(4)));
    }

    #[test]
    fn wide_exp_golomb_values_keep_every_bit() {
        // 3840 + 1 = 0b1111_0000_0001: 11 zeros, then those 12 bits.
        let (bytes, used) = bits(|b| b.ue(3840));
        assert_eq!(bytes, vec![0b0000_0000, 0b0001_1110, 0b0000_0010]);
        assert_eq!(used, 7);
        // The largest value: 32 zeros, a one, 32 zeros.
        let (bytes, used) = bits(|b| b.ue(u32::MAX));
        assert_eq!(bytes.len(), 9);
        assert_eq!(used, 1);
        assert_eq!(bytes[4], 0b1000_0000);
        assert!(bytes.iter().enumerate().all(|(i, &v)| i == 4 || v == 0));
    }

    #[test]
    fn alignment_ends_on_a_byte() {
        assert_eq!(bits(|b| b.align()), (vec![0b1000_0000], 0));
        assert_eq!(
            bits(|b| {
                b.put(7, 0);
                b.align();
            }),
            (vec![0b0000_0001], 0)
        );
    }

    #[test]
    fn emulation_prevention_guards_start_codes() {
        let mut out = Vec::new();
        nal(&[33 << 1, 1], &[0, 0, 0, 0, 1, 0, 0, 3, 0, 0, 4], &mut out);
        assert_eq!(
            out,
            [
                0,
                0,
                0,
                1,
                33 << 1,
                1,
                0,
                0,
                3,
                0,
                0,
                3,
                1,
                0,
                0,
                3,
                3,
                0,
                0,
                4
            ]
        );
    }
}
