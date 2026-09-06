//! gzip, as Ableton Live wants its project files.
//!
//! A Live set is XML run through gzip, so exporting one means producing a gzip
//! stream and re-opening one means reading a gzip stream that Live wrote. The
//! two directions need very different amounts of code:
//!
//! - Writing uses stored DEFLATE blocks. They are the part of the format that
//!   needs no entropy coding, they are legal DEFLATE, and every decompressor
//!   reads them. Compression would buy disk space this crate does not need at
//!   the price of a dependency it does not want.
//! - Reading is a complete inflate — stored, fixed and dynamic Huffman blocks —
//!   because files written elsewhere are compressed for real.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::formats::zip::crc32;

/// The largest payload a single stored block can hold.
const STORED_BLOCK: usize = 0xffff;

/// Wraps bytes in a gzip stream.
///
/// The output has no timestamp and no name, so the same input always gives the
/// same bytes.
#[must_use]
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let mut chunks = bytes.chunks(STORED_BLOCK).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let last = u8::from(chunks.peek().is_none());
        out.push(last);
        let length = chunk.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(bytes).to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out
}

/// Unwraps a gzip stream, whatever it was compressed with.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    let start = header_length(bytes)?;
    let mut bits = Bits::new(&bytes[start..]);
    let out = inflate(&mut bits)?;
    let tail = start + bits.consumed();
    let checksum = trailer(bytes, tail, "checksum")?;
    let length = trailer(bytes, tail + 4, "length")?;
    if checksum != crc32(&out) {
        return Err(Error::Format(format!(
            "gzip checksum {:#010x} does not match the stored {checksum:#010x}",
            crc32(&out)
        )));
    }
    if length as usize != out.len() {
        return Err(Error::Format(format!(
            "gzip says {length} bytes, the stream holds {}",
            out.len()
        )));
    }
    Ok(out)
}

/// Writes bytes to a gzip file.
pub fn write_file(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, compress(bytes))?;
    Ok(())
}

/// Reads a gzip file.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    decompress(&fs::read(path)?)
}

/// Reads one of the two trailer words.
fn trailer(bytes: &[u8], offset: usize, what: &str) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Format(format!("the gzip stream has no {what}")))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

/// Validates the gzip header and returns where the DEFLATE stream starts.
fn header_length(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < 18 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Err(Error::Format("not a gzip stream".to_string()));
    }
    if bytes[2] != 8 {
        return Err(Error::Unsupported(format!(
            "gzip compression method {}",
            bytes[2]
        )));
    }
    let flags = bytes[3];
    let mut offset = 10;
    if flags & 0b0000_0100 != 0 {
        let raw = bytes
            .get(offset..offset + 2)
            .ok_or_else(|| Error::Format("the gzip extra field is cut short".to_string()))?;
        offset += 2 + u16::from_le_bytes([raw[0], raw[1]]) as usize;
    }
    for (bit, what) in [(0b0000_1000, "name"), (0b0001_0000, "comment")] {
        if flags & bit != 0 {
            let end = bytes[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| Error::Format(format!("the gzip {what} is not terminated")))?;
            offset += end + 1;
        }
    }
    if flags & 0b0000_0010 != 0 {
        offset += 2;
    }
    if offset + 8 > bytes.len() {
        return Err(Error::Format("the gzip header is cut short".to_string()));
    }
    Ok(offset)
}

/// A DEFLATE bit reader: bits come out least-significant first.
struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    /// A reader over a DEFLATE stream.
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            bit: 0,
        }
    }

    /// How many bytes the stream used, rounding a partial byte up.
    const fn consumed(&self) -> usize {
        if self.bit == 0 {
            self.position
        } else {
            self.position + 1
        }
    }

    /// Reads one bit.
    fn bit(&mut self) -> Result<u32> {
        let byte = *self
            .bytes
            .get(self.position)
            .ok_or_else(|| Error::Format("the deflate stream ends mid-symbol".to_string()))?;
        let value = u32::from(byte >> self.bit) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.position += 1;
        }
        Ok(value)
    }

    /// Reads a little-endian bit field.
    fn bits(&mut self, count: u32) -> Result<u32> {
        let mut value = 0;
        for index in 0..count {
            value |= self.bit()? << index;
        }
        Ok(value)
    }

    /// Drops the rest of the current byte.
    const fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.position += 1;
        }
    }
}

/// A canonical Huffman decoder, built from code lengths.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Builds the decoder for a set of code lengths.
    fn new(lengths: &[u8]) -> Self {
        let mut counts = [0u16; 16];
        for length in lengths {
            counts[*length as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, length) in lengths.iter().enumerate() {
            if *length != 0 {
                symbols[offsets[*length as usize] as usize] = symbol as u16;
                offsets[*length as usize] += 1;
            }
        }
        Self { counts, symbols }
    }

    /// Decodes the next symbol.
    fn decode(&self, bits: &mut Bits) -> Result<u16> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..16 {
            code |= bits.bit()? as i32;
            let count = i32::from(self.counts[length]);
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Format("invalid huffman code".to_string()))
    }
}

/// Extra bits and base values for the length symbols 257..=285.
const LENGTHS: [(u16, u32); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// Extra bits and base values for the distance symbols 0..=29.
const DISTANCES: [(u16, u32); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

/// The order the code-length lengths are stored in.
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decodes a whole DEFLATE stream.
fn inflate(bits: &mut Bits) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let last = bits.bit()?;
        match bits.bits(2)? {
            0 => stored(bits, &mut out)?,
            1 => {
                let (literals, distances) = fixed_tables();
                block(bits, &literals, &distances, &mut out)?;
            }
            2 => {
                let (literals, distances) = dynamic_tables(bits)?;
                block(bits, &literals, &distances, &mut out)?;
            }
            _ => return Err(Error::Format("reserved deflate block type".to_string())),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

/// Copies an uncompressed block.
fn stored(bits: &mut Bits, out: &mut Vec<u8>) -> Result<()> {
    bits.align();
    let length = bits.bits(16)? as usize;
    let complement = bits.bits(16)?;
    if complement != !(length as u32) & 0xffff {
        return Err(Error::Format(
            "a stored block has a broken length field".to_string(),
        ));
    }
    let start = bits.position;
    let chunk = bits
        .bytes
        .get(start..start + length)
        .ok_or_else(|| Error::Format("a stored block is cut short".to_string()))?;
    out.extend_from_slice(chunk);
    bits.position += length;
    Ok(())
}

/// The tables a fixed-Huffman block uses.
fn fixed_tables() -> (Huffman, Huffman) {
    let mut lengths = [8u8; 288];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    (Huffman::new(&lengths), Huffman::new(&[5u8; 30]))
}

/// Reads the tables a dynamic-Huffman block carries in front of it.
fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman)> {
    let literal_count = bits.bits(5)? as usize + 257;
    let distance_count = bits.bits(5)? as usize + 1;
    let code_count = bits.bits(4)? as usize + 4;
    let mut code_lengths = [0u8; 19];
    for index in 0..code_count {
        code_lengths[ORDER[index]] = bits.bits(3)? as u8;
    }
    let codes = Huffman::new(&code_lengths);
    let mut lengths = vec![0u8; literal_count + distance_count];
    let mut index = 0;
    while index < lengths.len() {
        let symbol = codes.decode(bits)?;
        match symbol {
            0..=15 => {
                lengths[index] = symbol as u8;
                index += 1;
            }
            16 => {
                let previous = *lengths
                    .get(index.wrapping_sub(1))
                    .ok_or_else(|| Error::Format("nothing to repeat".to_string()))?;
                let repeat = bits.bits(2)? as usize + 3;
                fill(&mut lengths, &mut index, previous, repeat)?;
            }
            17 => {
                let repeat = bits.bits(3)? as usize + 3;
                fill(&mut lengths, &mut index, 0, repeat)?;
            }
            18 => {
                let repeat = bits.bits(7)? as usize + 11;
                fill(&mut lengths, &mut index, 0, repeat)?;
            }
            other => return Err(Error::Format(format!("invalid length code {other}"))),
        }
    }
    let (literals, distances) = lengths.split_at(literal_count);
    Ok((Huffman::new(literals), Huffman::new(distances)))
}

/// Writes a repeated code length, refusing to run past the table.
fn fill(lengths: &mut [u8], index: &mut usize, value: u8, repeat: usize) -> Result<()> {
    if *index + repeat > lengths.len() {
        return Err(Error::Format("a code length table overruns".to_string()));
    }
    lengths[*index..*index + repeat].fill(value);
    *index += repeat;
    Ok(())
}

/// Decodes the body of a Huffman-coded block.
fn block(
    bits: &mut Bits,
    literals: &Huffman,
    distances: &Huffman,
    out: &mut Vec<u8>,
) -> Result<()> {
    loop {
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let (base, extra) = LENGTHS[symbol as usize - 257];
                let length = base as usize + bits.bits(extra)? as usize;
                let symbol = distances.decode(bits)? as usize;
                let (base, extra) = *DISTANCES
                    .get(symbol)
                    .ok_or_else(|| Error::Format(format!("invalid distance code {symbol}")))?;
                let distance = base as usize + bits.bits(extra)? as usize;
                if distance > out.len() {
                    return Err(Error::Format(
                        "a back reference points before the stream".to_string(),
                    ));
                }
                let start = out.len() - distance;
                for offset in 0..length {
                    out.push(out[start + offset]);
                }
            }
            other => return Err(Error::Format(format!("invalid literal code {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_survive_the_round_trip() {
        for payload in [
            Vec::new(),
            b"<Ableton/>".to_vec(),
            (0..=255u8).cycle().take(200_000).collect(),
        ] {
            let packed = compress(&payload);
            assert_eq!(&packed[..3], &[0x1f, 0x8b, 8]);
            assert_eq!(decompress(&packed).unwrap(), payload);
        }
    }

    #[test]
    fn the_same_input_always_produces_the_same_bytes() {
        assert_eq!(compress(b"stable"), compress(b"stable"));
    }

    #[test]
    fn a_stream_written_elsewhere_is_read_back() {
        // A dynamic-Huffman stream with a file name in the header, written by
        // zlib at level 9 — the shape a Live set actually arrives in.
        let packed = include_bytes!("../../tests/fixtures/dynamic-huffman.xml.gz");
        let expected = include_bytes!("../../tests/fixtures/dynamic-huffman.xml");

        assert_eq!(decompress(packed).unwrap(), expected.to_vec());
    }

    #[test]
    fn a_fixed_huffman_stream_is_read_back() {
        // `printf aaaaaaaa | gzip`, which fits in one fixed-Huffman block and
        // uses a back reference to say "seven more of those".
        let packed: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x4b, 0x4c, 0x84, 0x00,
            0x00, 0x46, 0x80, 0x84, 0xbf, 0x08, 0x00, 0x00, 0x00,
        ];

        assert_eq!(decompress(packed).unwrap(), b"aaaaaaaa");
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-gzip-{}-{:?}/set.als",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, b"<Ableton/>").unwrap();

        assert_eq!(read_file(&path).unwrap(), b"<Ableton/>");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn damage_is_reported_rather_than_ignored() {
        let complain = |bytes: &[u8]| decompress(bytes).unwrap_err().to_string();

        assert!(complain(b"").contains("not a gzip"));
        assert!(
            complain(&[0x1f, 0x8b, 1, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0])
                .contains("compression method 1")
        );

        let mut packed = compress(b"hello");
        let last = packed.len() - 5;
        packed[last] ^= 0xff;
        assert!(complain(&packed).contains("checksum"));

        let mut packed = compress(b"hello");
        let length = packed.len() - 1;
        packed[length] = 9;
        assert!(complain(&packed).contains("the stream holds"));

        let mut packed = compress(b"hello");
        packed[11] = 9;
        assert!(complain(&packed).contains("broken length field"));
    }
}
