//! Frame and subframe decoding.
//!
//! A FLAC frame holds one independently decodable block of every channel. Each
//! channel is a subframe: a constant, a verbatim copy, or a prediction plus the
//! residual the prediction got wrong. Nothing here approximates anything — the
//! residual restores the predictor's error exactly, which is what makes the
//! format lossless.

use super::bits::{crc16, crc8, Reader};
use super::StreamInfo;
use crate::error::Result;
use crate::format_error;

/// Block sizes the four-bit code stands for, `0` meaning "read it separately".
const BLOCK_SIZES: [u32; 16] = [
    0, 192, 576, 1152, 2304, 4608, 0, 0, 256, 512, 1024, 2048, 4096, 8192, 16_384, 32_768,
];

/// Sample rates the four-bit code stands for, `0` meaning "elsewhere".
const SAMPLE_RATES: [u32; 16] = [
    0, 88_200, 176_400, 192_000, 8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000, 0,
    0, 0, 0,
];

/// Sample depths the three-bit code stands for, `0` meaning "from STREAMINFO".
const SAMPLE_DEPTHS: [u32; 8] = [0, 8, 12, 0, 16, 20, 24, 32];

/// How a stereo frame's two subframes relate to left and right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channels {
    /// Each subframe is already the channel it stands for.
    Independent(usize),
    /// Left, then left minus right.
    LeftSide,
    /// Left minus right, then right.
    RightSide,
    /// The average of the two, then their difference.
    MidSide,
}

impl Channels {
    /// The assignment a four-bit frame header code stands for.
    fn from_code(code: u64) -> Result<Self> {
        match code {
            0..=7 => Ok(Self::Independent(code as usize + 1)),
            8 => Ok(Self::LeftSide),
            9 => Ok(Self::RightSide),
            10 => Ok(Self::MidSide),
            _ => Err(format_error!(
                "FLAC frame uses reserved channel assignment {code}"
            )),
        }
    }

    /// How many subframes the frame carries.
    const fn count(self) -> usize {
        match self {
            Self::Independent(channels) => channels,
            _ => 2,
        }
    }

    /// Which subframe carries a difference, and so one extra bit of range.
    const fn side(self) -> Option<usize> {
        match self {
            Self::Independent(_) => None,
            Self::LeftSide | Self::MidSide => Some(1),
            Self::RightSide => Some(0),
        }
    }

    /// Turns the decoded subframes back into plain channels, in place.
    fn resolve(self, block: &mut [Vec<i64>]) {
        let Some((first, rest)) = block.split_first_mut() else {
            return;
        };
        let Some(second) = rest.first_mut() else {
            return;
        };
        match self {
            Self::Independent(_) => {}
            Self::LeftSide => {
                // `second` is left - right, and `first` is left.
                for (left, side) in first.iter().zip(second.iter_mut()) {
                    *side = *left - *side;
                }
            }
            Self::RightSide => {
                // `first` is left - right, and `second` is right.
                for (side, right) in first.iter_mut().zip(second.iter()) {
                    *side += *right;
                }
            }
            Self::MidSide => {
                for (mid, side) in first.iter_mut().zip(second.iter_mut()) {
                    // The encoder dropped the low bit of the sum, which the low
                    // bit of the difference carries instead.
                    let sum = (*mid << 1) | (*side & 1);
                    let left = (sum + *side) >> 1;
                    let right = (sum - *side) >> 1;
                    *mid = left;
                    *side = right;
                }
            }
        }
    }
}

/// What a frame header announced about the block that follows it.
pub struct FrameHeader {
    /// Samples per channel in this frame.
    pub block_size: u32,
    /// Sample rate, which a frame may restate.
    pub sample_rate: u32,
    /// Bits per sample, which a frame may restate.
    pub bits: u32,
    channels: Channels,
}

/// Reads one frame, returning its header and one vector of codes per channel.
pub fn frame(reader: &mut Reader, info: &StreamInfo) -> Result<(FrameHeader, Vec<Vec<i64>>)> {
    let start = reader.position() / 8;
    let header = frame_header(reader, info, start)?;

    let mut block = Vec::with_capacity(header.channels.count());
    for index in 0..header.channels.count() {
        let extra = u32::from(header.channels.side() == Some(index));
        block.push(subframe(reader, header.block_size, header.bits + extra)?);
    }
    header.channels.resolve(&mut block);

    reader.align();
    let expected = crc16(&reader.consumed()[start..]);
    let found = reader.unsigned(16)? as u16;
    if found != expected {
        return Err(format_error!(
            "FLAC frame fails its CRC-16 check: expected {expected:#06x}, found {found:#06x}"
        ));
    }
    Ok((header, block))
}

/// Reads a frame header, up to and including its CRC.
fn frame_header(reader: &mut Reader, info: &StreamInfo, start: usize) -> Result<FrameHeader> {
    let sync = reader.unsigned(14)?;
    if sync != 0x3FFE {
        return Err(format_error!("FLAC frame does not start with a sync code"));
    }
    if reader.bit()? != 0 {
        return Err(format_error!("FLAC frame sets a reserved header bit"));
    }
    // Whether the number below counts frames or samples changes nothing about
    // the audio, only about seeking, which this reader does not do.
    let _blocking_strategy = reader.bit()?;

    let block_code = reader.unsigned(4)? as usize;
    let rate_code = reader.unsigned(4)? as usize;
    let channels = Channels::from_code(reader.unsigned(4)?)?;
    let depth_code = reader.unsigned(3)? as usize;
    if reader.bit()? != 0 {
        return Err(format_error!("FLAC frame sets a reserved header bit"));
    }
    let _number = reader.utf8_number()?;

    let block_size = match block_code {
        0 => return Err(format_error!("FLAC frame uses a reserved block size code")),
        6 => reader.unsigned(8)? as u32 + 1,
        7 => reader.unsigned(16)? as u32 + 1,
        code => BLOCK_SIZES[code],
    };
    let sample_rate = match rate_code {
        0 => info.sample_rate,
        12 => reader.unsigned(8)? as u32 * 1_000,
        13 => reader.unsigned(16)? as u32,
        14 => reader.unsigned(16)? as u32 * 10,
        15 => return Err(format_error!("FLAC frame uses an invalid sample rate code")),
        code => SAMPLE_RATES[code],
    };
    let bits = match SAMPLE_DEPTHS[depth_code] {
        0 => info.bits,
        depth => depth,
    };

    let expected = crc8(&reader.consumed()[start..]);
    let found = reader.unsigned(8)? as u8;
    if found != expected {
        return Err(format_error!(
            "FLAC frame header fails its CRC-8 check: expected {expected:#04x}, found {found:#04x}"
        ));
    }
    if bits == 0 || bits > 32 {
        return Err(format_error!("FLAC frame claims {bits} bits per sample"));
    }
    Ok(FrameHeader {
        block_size,
        sample_rate,
        bits,
        channels,
    })
}

/// Reads one channel of one frame.
fn subframe(reader: &mut Reader, block_size: u32, bits: u32) -> Result<Vec<i64>> {
    if reader.bit()? != 0 {
        return Err(format_error!("FLAC subframe sets a reserved leading bit"));
    }
    let kind = reader.unsigned(6)?;
    // Samples whose low bits are all zero are stored shifted down, and the
    // count of dropped bits is coded here rather than costing a bit a sample.
    let wasted = if reader.bit()? == 1 {
        reader.unary()? + 1
    } else {
        0
    };
    if wasted >= bits {
        return Err(format_error!(
            "FLAC subframe wastes {wasted} of {bits} bits, leaving nothing"
        ));
    }
    let bits = bits - wasted;

    let mut samples = match kind {
        0 => vec![reader.signed(bits)?; block_size as usize],
        1 => {
            let mut samples = Vec::with_capacity(block_size as usize);
            for _ in 0..block_size {
                samples.push(reader.signed(bits)?);
            }
            samples
        }
        8..=12 => fixed(reader, block_size, bits, kind as usize - 8)?,
        32..=63 => lpc(reader, block_size, bits, kind as usize - 31)?,
        _ => {
            return Err(format_error!(
                "FLAC subframe uses reserved type {kind:#08b}"
            ))
        }
    };

    if wasted > 0 {
        for sample in &mut samples {
            *sample <<= wasted;
        }
    }
    Ok(samples)
}

/// Reads a subframe predicted by one of the five fixed polynomials.
fn fixed(reader: &mut Reader, block_size: u32, bits: u32, order: usize) -> Result<Vec<i64>> {
    let mut samples = Vec::with_capacity(block_size as usize);
    for _ in 0..order {
        samples.push(reader.signed(bits)?);
    }
    residual(reader, block_size, order, &mut samples)?;
    for index in order..block_size as usize {
        samples[index] += fixed_prediction(&samples[..index], order);
    }
    Ok(samples)
}

/// The value the fixed predictor of `order` expects to come next.
pub fn fixed_prediction(past: &[i64], order: usize) -> i64 {
    let at = |back: usize| past[past.len() - back];
    match order {
        1 => at(1),
        2 => 2 * at(1) - at(2),
        3 => 3 * at(1) - 3 * at(2) + at(3),
        4 => 4 * at(1) - 6 * at(2) + 4 * at(3) - at(4),
        _ => 0,
    }
}

/// Reads a subframe predicted by coefficients the encoder chose and stored.
fn lpc(reader: &mut Reader, block_size: u32, bits: u32, order: usize) -> Result<Vec<i64>> {
    let mut samples = Vec::with_capacity(block_size as usize);
    for _ in 0..order {
        samples.push(reader.signed(bits)?);
    }
    let precision = reader.unsigned(4)? as u32 + 1;
    if precision == 16 {
        return Err(format_error!(
            "FLAC subframe uses the invalid coefficient precision of 16"
        ));
    }
    let shift = reader.signed(5)?;
    if shift < 0 {
        return Err(format_error!(
            "FLAC subframe uses a negative predictor shift"
        ));
    }
    let mut coefficients = Vec::with_capacity(order);
    for _ in 0..order {
        coefficients.push(reader.signed(precision)?);
    }
    residual(reader, block_size, order, &mut samples)?;

    for index in order..block_size as usize {
        let prediction: i64 = coefficients
            .iter()
            .enumerate()
            .map(|(back, coefficient)| coefficient * samples[index - 1 - back])
            .sum();
        samples[index] += prediction >> shift;
    }
    Ok(samples)
}

/// Reads the Rice-coded residual that follows a predictor, appending it.
fn residual(
    reader: &mut Reader,
    block_size: u32,
    order: usize,
    samples: &mut Vec<i64>,
) -> Result<()> {
    let method = reader.unsigned(2)?;
    let (parameter_bits, escape) = match method {
        0 => (4, 0x0F),
        1 => (5, 0x1F),
        _ => {
            return Err(format_error!(
                "FLAC residual uses reserved coding method {method}"
            ))
        }
    };
    let partition_order = reader.unsigned(4)? as u32;
    let partitions = 1_usize << partition_order;
    if block_size % partitions as u32 != 0 {
        return Err(format_error!(
            "FLAC residual splits {block_size} samples into {partitions} partitions"
        ));
    }
    let per_partition = (block_size >> partition_order) as usize;
    if per_partition < order {
        return Err(format_error!(
            "FLAC residual partition is shorter than the predictor's warm-up"
        ));
    }

    for partition in 0..partitions {
        // The first partition also holds the warm-up samples already read.
        let count = if partition == 0 {
            per_partition - order
        } else {
            per_partition
        };
        let parameter = reader.unsigned(parameter_bits)? as u32;
        if parameter == escape {
            // An escaped partition stores its residuals raw, which is smaller
            // whenever Rice coding would spend more than the samples are worth.
            let raw = reader.unsigned(5)? as u32;
            for _ in 0..count {
                samples.push(reader.signed(raw)?);
            }
        } else {
            for _ in 0..count {
                let quotient = u64::from(reader.unary()?);
                let remainder = reader.unsigned(parameter)?;
                let folded = (quotient << parameter) | remainder;
                // Residuals alternate sign around zero, so they are folded onto
                // the unsigned numbers Rice coding can carry.
                samples.push(((folded >> 1) as i64) ^ -((folded & 1) as i64));
            }
        }
    }
    Ok(())
}
