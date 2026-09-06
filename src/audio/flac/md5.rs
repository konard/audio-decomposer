//! The digest a FLAC stream carries of the audio it was made from.
//!
//! Every frame is already guarded by a CRC, but a CRC only proves that a frame
//! arrived as it was written. The digest proves something stronger and more
//! useful here: that the samples coming out of the decoder are the samples that
//! went into the encoder. That is exactly the guarantee this crate makes about
//! the whole pipeline, so it is worth writing and worth checking.
//!
//! MD5 is used because the format specifies it, not because anything here needs
//! a cryptographic hash. It is guarding against damaged files, not attackers.

/// Per-round left-rotation amounts.
const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The digest of the interleaved samples, as the format defines it.
///
/// Samples are laid out the way an uncompressed file would store them: frame by
/// frame, channel by channel, little-endian two's complement, in whole bytes.
pub fn of_samples(codes: &[Vec<i64>], bits: u32) -> [u8; 16] {
    let width = bits.div_ceil(8) as usize;
    let frames = codes.first().map_or(0, Vec::len);
    let mut hasher = Md5::new();
    let mut frame_bytes = Vec::with_capacity(width * codes.len());
    for frame in 0..frames {
        frame_bytes.clear();
        for channel in codes {
            let code = channel[frame];
            frame_bytes.extend_from_slice(&code.to_le_bytes()[..width]);
        }
        hasher.update(&frame_bytes);
    }
    hasher.finish()
}

/// The incremental state of a digest.
struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    filled: usize,
    length: u64,
}

impl Md5 {
    /// The state the algorithm starts from.
    const fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buffer: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    /// Absorbs more input, hashing whole blocks as they complete.
    fn update(&mut self, mut bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        while !bytes.is_empty() {
            let take = (64 - self.filled).min(bytes.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.filled = 0;
            }
        }
    }

    /// Pads the last block and returns the digest.
    fn finish(mut self) -> [u8; 16] {
        let bit_length = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0x00]);
        }
        // `update` counts these bytes too, but the length was captured first.
        let block = {
            let mut block = self.buffer;
            block[56..].copy_from_slice(&bit_length.to_le_bytes());
            block
        };
        self.compress(&block);

        let mut digest = [0u8; 16];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        digest
    }

    /// Mixes one 64-byte block into the state.
    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 16];
        for (word, chunk) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        let [mut a, mut b, mut c, mut d] = self.state;
        for (round, shift) in SHIFTS.iter().enumerate() {
            let (mixed, index) = match round / 16 {
                0 => ((b & c) | (!b & d), round),
                1 => ((d & b) | (!d & c), (5 * round + 1) % 16),
                2 => (b ^ c ^ d, (3 * round + 5) % 16),
                _ => (c ^ (b | !d), (7 * round) % 16),
            };
            // The constants are the integer parts of 2^32 * |sin(i + 1)|.
            let constant = (((round + 1) as f64).sin().abs() * 4_294_967_296.0) as u32;
            let sum = a
                .wrapping_add(mixed)
                .wrapping_add(constant)
                .wrapping_add(words[index]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(*shift));
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest of `bytes`, as hexadecimal.
    fn hex(bytes: &[u8]) -> String {
        let mut hasher = Md5::new();
        hasher.update(bytes);
        render(&hasher.finish())
    }

    /// Sixteen bytes written out the way digests are usually quoted.
    fn render(digest: &[u8; 16]) -> String {
        use std::fmt::Write;
        digest.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
    }

    #[test]
    fn the_digests_match_the_values_the_specification_publishes() {
        assert_eq!(hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            hex(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            hex(&b"1234567890".repeat(8)),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn input_is_hashed_the_same_however_it_is_split() {
        let whole = hex(&(0..=255u8).collect::<Vec<_>>());
        let mut hasher = Md5::new();
        for chunk in (0..=255u8).collect::<Vec<_>>().chunks(7) {
            hasher.update(chunk);
        }
        let split = render(&hasher.finish());
        assert_eq!(whole, split);
    }

    #[test]
    fn samples_are_laid_out_interleaved_and_little_endian() {
        // Two channels of one frame, 16 bits: 0x0102 then -1.
        let codes = vec![vec![0x0102], vec![-1]];
        let mut expected = Md5::new();
        expected.update(&[0x02, 0x01, 0xFF, 0xFF]);
        assert_eq!(of_samples(&codes, 16), expected.finish());
    }
}
