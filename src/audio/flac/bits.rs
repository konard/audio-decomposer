//! Most-significant-bit-first bit I/O, and the two CRCs a FLAC stream carries.
//!
//! DEFLATE, which [`crate::formats::gzip`] implements, packs its bits from the
//! least significant end upwards. FLAC packs them from the most significant end
//! downwards, so the two cannot share a reader.

use crate::error::Result;
use crate::format_error;

/// Reads bits from a byte slice, most significant bit first.
pub struct Reader<'a> {
    bytes: &'a [u8],
    /// How many bits have been consumed from the front of `bytes`.
    position: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading at the first bit of `bytes`.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// How many bits have been consumed so far.
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Discards bits up to the next byte boundary.
    pub const fn align(&mut self) {
        self.position = self.position.div_ceil(8) * 8;
    }

    /// The bytes consumed so far, which is only meaningful when aligned.
    pub fn consumed(&self) -> &'a [u8] {
        &self.bytes[..self.position / 8]
    }

    /// Skips whole bytes, from the next byte boundary on.
    pub fn skip_bytes(&mut self, count: usize) -> Result<()> {
        self.align();
        let end = self
            .position
            .checked_add(count * 8)
            .filter(|end| *end <= self.bytes.len() * 8)
            .ok_or_else(|| format_error!("FLAC metadata block runs past the end of the stream"))?;
        self.position = end;
        Ok(())
    }

    /// The bytes from the current position on, which must be aligned.
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.position / 8..]
    }

    /// Reads one bit.
    pub fn bit(&mut self) -> Result<u32> {
        let byte = self
            .bytes
            .get(self.position / 8)
            .ok_or_else(|| format_error!("FLAC stream ended in the middle of a value"))?;
        let bit = u32::from(byte >> (7 - self.position % 8)) & 1;
        self.position += 1;
        Ok(bit)
    }

    /// Reads `count` bits as an unsigned value. `count` may be 0 through 64.
    pub fn unsigned(&mut self, count: u32) -> Result<u64> {
        debug_assert!(count <= 64, "cannot read {count} bits into a u64");
        if count > 32 {
            let high = self.unsigned(count - 32)?;
            return Ok((high << 32) | self.unsigned(32)?);
        }
        if count == 0 {
            return Ok(0);
        }
        if self.position + count as usize > self.bytes.len() * 8 {
            return Err(format_error!("FLAC stream ended in the middle of a value"));
        }
        // Whole bytes at a time: a residual is millions of small reads, and one
        // pass per bit is the difference between decoding a song and waiting.
        let mut value = 0_u64;
        let mut left = count;
        while left > 0 {
            let byte = u64::from(self.bytes[self.position / 8]);
            let offset = (self.position % 8) as u32;
            let available = 8 - offset;
            let take = available.min(left);
            let chunk = (byte >> (available - take)) & (u64::MAX >> (64 - take));
            value = (value << take) | chunk;
            self.position += take as usize;
            left -= take;
        }
        Ok(value)
    }

    /// Reads `count` bits as a two's-complement signed value.
    pub fn signed(&mut self, count: u32) -> Result<i64> {
        if count == 0 {
            return Ok(0);
        }
        let value = self.unsigned(count)?;
        // Sign-extend from `count` bits by shifting the sign bit up to the top
        // and back down again with an arithmetic shift.
        let shift = 64 - count;
        Ok(((value << shift) as i64) >> shift)
    }

    /// Reads a unary value: the number of zero bits before the next one bit.
    pub fn unary(&mut self) -> Result<u32> {
        let mut zeros = 0_u32;
        loop {
            let index = self.position / 8;
            let byte = *self
                .bytes
                .get(index)
                .ok_or_else(|| format_error!("FLAC stream ended in the middle of a value"))?;
            let offset = (self.position % 8) as u32;
            // Only the bits at and after the cursor count, lifted to the top
            // of a word so that `leading_zeros` answers the question directly.
            let rest = u32::from(byte & (0xFF >> offset)) << (24 + offset);
            if rest == 0 {
                let skipped = 8 - offset;
                zeros += skipped;
                self.position += skipped as usize;
                if zeros > 1 << 20 {
                    return Err(format_error!("FLAC unary value is implausibly long"));
                }
                continue;
            }
            let leading = rest.leading_zeros();
            zeros += leading;
            self.position += leading as usize + 1;
            return Ok(zeros);
        }
    }

    /// Reads the extended UTF-8 number a frame header identifies itself with.
    ///
    /// FLAC borrows UTF-8's shape to code a frame or sample number, stretching
    /// it to seven bytes so that 36 bits fit.
    pub fn utf8_number(&mut self) -> Result<u64> {
        let first = self.unsigned(8)? as u8;
        let (extra, mut value) = match first.leading_ones() {
            0 => (0, u64::from(first)),
            2 => (1, u64::from(first & 0x1F)),
            3 => (2, u64::from(first & 0x0F)),
            4 => (3, u64::from(first & 0x07)),
            5 => (4, u64::from(first & 0x03)),
            6 => (5, u64::from(first & 0x01)),
            7 => (6, 0),
            _ => {
                return Err(format_error!(
                    "FLAC frame number has a malformed first byte"
                ))
            }
        };
        for _ in 0..extra {
            let byte = self.unsigned(8)? as u8;
            if byte & 0xC0 != 0x80 {
                return Err(format_error!("FLAC frame number has a malformed byte"));
            }
            value = (value << 6) | u64::from(byte & 0x3F);
        }
        Ok(value)
    }
}

/// Writes bits into a byte vector, most significant bit first.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
    /// Bits not yet flushed, held in the low `filled` bits.
    partial: u64,
    filled: u32,
}

impl Writer {
    /// An empty writer.
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            partial: 0,
            filled: 0,
        }
    }

    /// Whether the next write starts on a byte boundary.
    pub const fn is_aligned(&self) -> bool {
        self.filled == 0
    }

    /// Bits written so far.
    pub fn len(&self) -> usize {
        self.bytes.len() * 8 + self.filled as usize
    }

    /// Writes the low `count` bits of `value`, most significant first.
    pub fn unsigned(&mut self, value: u64, count: u32) {
        debug_assert!(count <= 64, "cannot write {count} bits from a u64");
        // Splitting anything wider keeps `partial` from overflowing: fewer than
        // eight bits are ever pending, so eight plus thirty-two always fits.
        if count > 32 {
            self.unsigned(value >> 32, count - 32);
            self.unsigned(value & 0xFFFF_FFFF, 32);
            return;
        }
        if count == 0 {
            return;
        }
        self.partial = (self.partial << count) | (value & (u64::MAX >> (64 - count)));
        self.filled += count;
        while self.filled >= 8 {
            self.filled -= 8;
            self.bytes.push((self.partial >> self.filled) as u8);
        }
        self.partial &= (1 << self.filled) - 1;
    }

    /// Appends everything another writer holds, bit for bit.
    pub fn append(&mut self, other: &Self) {
        if self.filled == 0 {
            self.bytes.extend_from_slice(&other.bytes);
        } else {
            for byte in &other.bytes {
                self.unsigned(u64::from(*byte), 8);
            }
        }
        if other.filled > 0 {
            self.unsigned(other.partial, other.filled);
        }
    }

    /// Writes `value` in `count` bits of two's complement.
    pub fn signed(&mut self, value: i64, count: u32) {
        let mask = if count == 64 {
            u64::MAX
        } else {
            (1 << count) - 1
        };
        self.unsigned((value as u64) & mask, count);
    }

    /// Writes a unary value: `value` zero bits, then a one bit.
    pub fn unary(&mut self, value: u32) {
        let mut left = value;
        while left >= 32 {
            self.unsigned(0, 32);
            left -= 32;
        }
        self.unsigned(1, left + 1);
    }

    /// Writes the extended UTF-8 coding of a frame number.
    pub fn utf8_number(&mut self, value: u64) {
        // The thresholds are UTF-8's, with two more rows so that 36 bits fit.
        const LIMITS: [(u64, u32); 7] = [
            (0x80, 0),
            (0x800, 1),
            (0x1_0000, 2),
            (0x20_0000, 3),
            (0x400_0000, 4),
            (0x8000_0000, 5),
            (0x10_0000_0000, 6),
        ];
        let extra = LIMITS
            .iter()
            .find(|(limit, _)| value < *limit)
            .map_or(6, |(_, extra)| *extra);
        if extra == 0 {
            self.unsigned(value, 8);
            return;
        }
        // A leading byte carries `extra + 1` ones, a zero, then the top bits.
        let head_bits = 6 - extra;
        let ones = u64::from(u8::MAX >> (7 - extra)) << (head_bits + 1);
        self.unsigned(ones | (value >> (6 * extra)), 8);
        for index in (0..extra).rev() {
            self.unsigned(0x80 | ((value >> (6 * index)) & 0x3F), 8);
        }
    }

    /// Pads with zero bits up to the next byte boundary.
    pub fn align(&mut self) {
        if self.filled != 0 {
            self.unsigned(0, 8 - self.filled);
        }
    }

    /// The bytes written, which must be byte-aligned.
    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }

    /// The bytes written so far, for a CRC over a partly built frame.
    pub fn written(&self) -> &[u8] {
        &self.bytes
    }
}

/// CRC-8 over `x^8 + x^2 + x + 1`, which seals a FLAC frame header.
#[must_use]
pub fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x07
            };
        }
    }
    crc
}

/// CRC-16 over `x^16 + x^15 + x^2 + 1`, which seals a whole FLAC frame.
#[must_use]
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x8005
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_survive_a_round_trip() {
        let mut writer = Writer::new();
        writer.unsigned(0b101, 3);
        writer.signed(-3, 5);
        writer.unary(7);
        writer.unsigned(0xDEAD_BEEF, 32);
        writer.signed(i64::MIN, 64);
        let bytes = writer.finish();

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.unsigned(3).unwrap(), 0b101);
        assert_eq!(reader.signed(5).unwrap(), -3);
        assert_eq!(reader.unary().unwrap(), 7);
        assert_eq!(reader.unsigned(32).unwrap(), 0xDEAD_BEEF);
        assert_eq!(reader.signed(64).unwrap(), i64::MIN);
    }

    #[test]
    fn frame_numbers_survive_the_whole_range() {
        for value in [
            0,
            1,
            0x7F,
            0x80,
            0x7FF,
            0x800,
            0xFFFF,
            0x10_0000,
            0xF_FFFF_FFFF,
        ] {
            let mut writer = Writer::new();
            writer.utf8_number(value);
            let bytes = writer.finish();
            let mut reader = Reader::new(&bytes);
            assert_eq!(reader.utf8_number().unwrap(), value, "at {value:#x}");
        }
    }

    #[test]
    fn a_short_stream_is_reported_rather_than_panicking() {
        let mut reader = Reader::new(&[0x00]);
        assert!(reader.unsigned(9).is_err());
    }

    #[test]
    fn the_crcs_match_the_values_the_specification_publishes() {
        // "123456789" is the standard check vector for both polynomials. FLAC
        // reflects neither, so the 16-bit value is BUYPASS's 0xFEE8 rather than
        // the 0xBB3D of the reflected ARC variant that shares the polynomial.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }
}
