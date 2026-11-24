//! Annex B units, emulation prevention and the bit fields of H.264 and HEVC headers.
use std::fmt;

/// Syntax a header rewrite does not handle, or a header cut short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);
impl fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for Unsupported {}
pub type Result<T> = std::result::Result<T, Unsupported>;
pub const TRUNCATED: Unsupported = Unsupported("header cut short");
/// A NAL unit's new bytes, and the index in the old unit from which its
/// bytes follow them unchanged.
pub type Replacement = (Vec<u8>, usize);
/// One NAL unit edit whose replacement bytes live in a shared `heads` scratch
/// buffer, so rewriting a multi-slice frame allocates no per-slice vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub head_start: usize,
    pub head_end: usize,
    pub rest: usize,
}

/// Reusable Annex B / RBSP scratch buffers shared across frames and slices so
/// steady-state header rewriting allocates nothing.
#[derive(Default)]
pub struct Scratch {
    pub starts: Vec<usize>,
    pub nals: Vec<(usize, usize)>,
    pub edits: Vec<Edit>,
    pub heads: Vec<u8>,
    pub rbsp: Vec<u8>,
    pub from: Vec<usize>,
    pub writer: Writer,
}

impl Scratch {
    pub fn begin(&mut self, bytes: &[u8]) {
        units_into(bytes, &mut self.starts, &mut self.nals);
        self.edits.clear();
        self.heads.clear();
    }
    pub fn unescape(&mut self, payload: &[u8], limit: usize) {
        unescape_into(payload, limit, &mut self.rbsp, &mut self.from);
    }
    pub fn push_edit(
        &mut self,
        start: usize,
        end: usize,
        head: std::ops::Range<usize>,
        rest: usize,
    ) {
        self.edits.push(Edit {
            start,
            end,
            head_start: head.start,
            head_end: head.end,
            rest,
        });
    }
    pub fn assemble(&self, bytes: &[u8], output: &mut Vec<u8>) {
        assemble_edits_into(bytes, &self.heads, &self.edits, output);
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    pub at: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    pub fn bits(&mut self, count: u32) -> Result<u32> {
        debug_assert!(count <= 32);
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8).ok_or(TRUNCATED)?;
            value = (value << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value)
    }
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.bits(1)? == 1)
    }
    pub fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Unsupported("Exp-Golomb code over 32 bits"));
            }
        }
        Ok(((1u64 << zeros) - 1 + u64::from(self.bits(zeros)?)) as u32)
    }
    /// An unsigned code no larger than `limit`.
    pub fn ue_max(&mut self, limit: u32, what: &'static str) -> Result<u32> {
        let value = self.ue()?;
        if value > limit {
            return Err(Unsupported(what));
        }
        Ok(value)
    }
    pub fn se(&mut self) -> Result<i32> {
        let value = i64::from(self.ue()?);
        Ok(if value & 1 == 1 {
            (value + 1) / 2
        } else {
            -(value / 2)
        } as i32)
    }
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.at = self
            .at
            .checked_add(count)
            .filter(|&at| at <= self.bytes.len() * 8)
            .ok_or(TRUNCATED)?;
        Ok(())
    }
}

#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
    at: usize,
}
impl Writer {
    /// Reset for another header while retaining the allocated backing buffer.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.at = 0;
    }
    pub fn bits(&mut self, count: u32, value: u32) {
        for index in (0..count).rev() {
            if self.at.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> index) & 1 == 1 {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.at % 8);
            }
            self.at += 1;
        }
    }
    pub fn flag(&mut self, on: bool) {
        self.bits(1, u32::from(on));
    }
    pub fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.bits(length - 1, 0);
        if length > 32 {
            self.bits(1, 1);
        }
        self.bits(length.min(32), code as u32);
    }
    /// Bits `start..end` of `from`, unchanged.
    pub fn copy(&mut self, from: &[u8], start: usize, end: usize) {
        for at in start..end {
            self.flag((from[at / 8] >> (7 - at % 8)) & 1 == 1);
        }
    }
    /// Finish RBSP trailing bits or slice header alignment in place, keeping
    /// the backing buffer reusable across headers.
    pub fn finish_in_place(&mut self) -> &[u8] {
        self.flag(true);
        while !self.at.is_multiple_of(8) {
            self.flag(false);
        }
        &self.bytes
    }
    /// The bytes after a one bit and zeros to the byte boundary: RBSP
    /// trailing bits, or a slice header's byte alignment.
    #[cfg(test)]
    pub fn finish(mut self) -> Vec<u8> {
        self.finish_in_place();
        self.bytes
    }
}

/// The NAL units of an Annex B buffer, each without its start code. Zero
/// bytes before a start code belong to it, as a NAL unit never ends in zero.
pub fn units(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut out = Vec::new();
    units_into(bytes, &mut starts, &mut out);
    out
}

/// [`units`] writing into caller-owned scratch buffers to avoid per-frame
/// allocations.
pub fn units_into(bytes: &[u8], starts: &mut Vec<usize>, out: &mut Vec<(usize, usize)>) {
    starts.clear();
    out.clear();
    let mut index = 0;
    while index + 2 < bytes.len() {
        // No start code begins at a byte that is not zero: skip eight
        // bytes at a time while none is.
        if let Some(word) = bytes.get(index..index + 8) {
            let word = u64::from_le_bytes(word.try_into().unwrap());
            if word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080 == 0 {
                index += 8;
                continue;
            }
        }
        if bytes[index + 2] > 1 {
            index += 3;
        } else if bytes[index + 2] == 1 && bytes[index] == 0 && bytes[index + 1] == 0 {
            starts.push(index + 3);
            index += 3;
        } else {
            index += 1;
        }
    }
    out.reserve(starts.len());
    for (position, &start) in starts.iter().enumerate() {
        let mut end = starts
            .get(position + 1)
            .map_or(bytes.len(), |&next| next - 3);
        while end > start && bytes[end - 1] == 0 {
            end -= 1;
        }
        out.push((start, end));
    }
}

/// Up to `limit` RBSP bytes of a NAL unit's payload, each with the index of
/// the payload byte it came from.
pub fn unescape(payload: &[u8], limit: usize) -> (Vec<u8>, Vec<usize>) {
    let (mut rbsp, mut from) = (Vec::new(), Vec::new());
    unescape_into(payload, limit, &mut rbsp, &mut from);
    (rbsp, from)
}

/// [`unescape`] writing into caller-owned scratch buffers.
pub fn unescape_into(payload: &[u8], limit: usize, rbsp: &mut Vec<u8>, from: &mut Vec<usize>) {
    rbsp.clear();
    from.clear();
    let capacity = limit.min(payload.len());
    rbsp.reserve(capacity);
    from.reserve(capacity);
    let mut zeros = 0;
    for (index, &byte) in payload.iter().enumerate() {
        if rbsp.len() >= limit {
            break;
        }
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        from.push(index);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

pub fn escape(rbsp: &[u8], output: &mut Vec<u8>) {
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            output.push(3);
            zeros = 0;
        }
        output.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

/// Replace a NAL unit's first `header` RBSP bytes with `rewritten`, appending
/// the escaped replacement header onto `heads` and returning its range within
/// `heads` together with the unchanged tail offset. The rest stays as it was,
/// emulation prevention included: the last byte of both headers is non-zero
/// (it holds a one bit), so no escape spans the seam.
pub fn splice_into(
    nal: &[u8],
    prefix: usize,
    rewritten: &[u8],
    from: &[usize],
    header: usize,
    heads: &mut Vec<u8>,
) -> (std::ops::Range<usize>, usize) {
    debug_assert!(header > 0 && rewritten.last().is_some_and(|&byte| byte != 0));
    let head_start = heads.len();
    heads.reserve(prefix + rewritten.len() + rewritten.len() / 2);
    heads.extend_from_slice(&nal[..prefix]);
    escape(rewritten, heads);
    (head_start..heads.len(), prefix + from[header - 1] + 1)
}

/// `bytes` with the NAL units at `edits` replaced, copied once.
pub fn assemble(bytes: &[u8], edits: Vec<((usize, usize), Replacement)>) -> Vec<u8> {
    let mut output = Vec::new();
    assemble_into(bytes, edits, &mut output);
    output
}

/// [`assemble`] into `output`, which is cleared first and keeps its capacity.
pub fn assemble_into(
    bytes: &[u8],
    edits: Vec<((usize, usize), Replacement)>,
    output: &mut Vec<u8>,
) {
    let final_len = edits.iter().fold(bytes.len(), |len, (_, (head, rest))| {
        len.saturating_sub(*rest).saturating_add(head.len())
    });
    output.clear();
    output.reserve(final_len);
    let mut cursor = 0;
    for ((start, end), (head, rest)) in edits {
        output.extend_from_slice(&bytes[cursor..start]);
        output.extend_from_slice(&head);
        output.extend_from_slice(&bytes[start + rest..end]);
        cursor = end;
    }
    output.extend_from_slice(&bytes[cursor..]);
}

/// [`assemble_into`] for edits whose replacement headers share `heads`.
pub fn assemble_edits_into(bytes: &[u8], heads: &[u8], edits: &[Edit], output: &mut Vec<u8>) {
    let final_len = edits.iter().fold(bytes.len(), |len, edit| {
        len.saturating_sub(edit.rest)
            .saturating_add(edit.head_end - edit.head_start)
    });
    output.clear();
    output.reserve(final_len);
    let mut cursor = 0;
    for edit in edits {
        output.extend_from_slice(&bytes[cursor..edit.start]);
        output.extend_from_slice(&heads[edit.head_start..edit.head_end]);
        output.extend_from_slice(&bytes[edit.start + edit.rest..edit.end]);
        cursor = edit.end;
    }
    output.extend_from_slice(&bytes[cursor..]);
}

pub fn ceil_log2(value: u32) -> u32 {
    if value <= 1 {
        0
    } else {
        32 - (value - 1).leading_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_round_trip_through_the_writer_and_reader() {
        let mut writer = Writer::default();
        let values = [0, 1, 2, 3, 7, 8, 255, 65_535, u32::MAX - 1];
        for &value in &values {
            writer.ue(value);
            writer.bits(5, value & 31);
        }
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        for &value in &values {
            assert_eq!(reader.ue().unwrap(), value);
            assert_eq!(reader.bits(5).unwrap(), value & 31);
        }
        assert!(reader.flag().unwrap());
        assert_eq!(reader.at.div_ceil(8), bytes.len());
        for (code, signed) in [(0, 0), (1, 1), (2, -1), (3, 2), (4, -2)] {
            let mut writer = Writer::default();
            writer.ue(code);
            assert_eq!(Reader::new(&writer.finish()).se().unwrap(), signed);
        }
    }
    #[test]
    fn escaping_round_trips_and_units_keep_their_zero_bytes_apart() {
        let rbsp = [0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 7, 0, 0];
        let mut escaped = Vec::new();
        escape(&rbsp, &mut escaped);
        assert_eq!(
            escaped,
            [0, 0, 3, 0, 0, 3, 1, 0, 0, 3, 2, 0, 0, 3, 3, 7, 0, 0]
        );
        assert_eq!(unescape(&escaped, usize::MAX).0, rbsp);
        let (prefix, from) = unescape(&escaped, 5);
        assert_eq!(prefix, [0, 0, 0, 0, 1]);
        assert_eq!(from, [0, 1, 3, 4, 6]);
        let stream = [
            0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1, 9, 0, 0, 0, 0, 1, 0x26,
        ];
        assert_eq!(units(&stream), [(4, 6), (9, 12), (17, 18)]);
        // Long runs without a zero, and start codes at every alignment of
        // the eight-byte skip.
        for at in 0..24 {
            let mut stream = vec![7u8; 40];
            stream[at..at + 3].copy_from_slice(&[0, 0, 1]);
            assert_eq!(units(&stream), [(at + 3, 40)], "start code at {at}");
        }
        assert!(units(&[0, 0]).is_empty());
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(920), 10);
    }
    #[test]
    fn scratch_helpers_match_allocating_versions_and_reuse_capacity() {
        let mut writer = Writer::default();
        writer.ue(42);
        writer.bits(8, 0xA5);
        let first = writer.finish_in_place().to_vec();
        let ptr = writer.bytes.as_ptr();
        writer.clear();
        writer.ue(7);
        let second = writer.finish_in_place().to_vec();
        assert_eq!(writer.bytes.as_ptr(), ptr, "writer reuses backing buffer");
        let mut fresh = Writer::default();
        fresh.ue(42);
        fresh.bits(8, 0xA5);
        assert_eq!(first, fresh.finish());
        let mut fresh = Writer::default();
        fresh.ue(7);
        assert_eq!(second, fresh.finish());

        let stream = [
            0, 0, 0, 1, 0x02, 0x01, 0x00, 0x00, 0x03, 0x01, 0x80, 0x55, 0, 0, 1, 0x02, 0x01, 0x90,
            0x66,
        ];
        let (mut starts, mut nals) = (Vec::new(), Vec::new());
        units_into(&stream, &mut starts, &mut nals);
        assert_eq!(nals, units(&stream));
        let (mut rbsp, mut from) = (Vec::new(), Vec::new());
        let (s0, e0) = nals[0];
        unescape_into(&stream[s0 + 2..e0], 4, &mut rbsp, &mut from);
        assert_eq!(
            (rbsp.clone(), from.clone()),
            unescape(&stream[s0 + 2..e0], 4)
        );

        let mut heads = Vec::new();
        let (r0, rest0) = splice_into(
            &stream[s0..e0],
            2,
            &[0x00, 0x00, 0x01, 0x80],
            &from,
            4,
            &mut heads,
        );
        let (s1, e1) = nals[1];
        unescape_into(&stream[s1 + 2..e1], 1, &mut rbsp, &mut from);
        let (r1, rest1) = splice_into(&stream[s1..e1], 2, &[0xC0], &from, 1, &mut heads);
        let edits = [
            Edit {
                start: s0,
                end: e0,
                head_start: r0.start,
                head_end: r0.end,
                rest: rest0,
            },
            Edit {
                start: s1,
                end: e1,
                head_start: r1.start,
                head_end: r1.end,
                rest: rest1,
            },
        ];
        let mut out = Vec::new();
        assemble_edits_into(&stream, &heads, &edits, &mut out);
        let expected = assemble(
            &stream,
            vec![
                ((s0, e0), (heads[r0].to_vec(), rest0)),
                ((s1, e1), (heads[r1].to_vec(), rest1)),
            ],
        );
        assert_eq!(out, expected);
    }
}
