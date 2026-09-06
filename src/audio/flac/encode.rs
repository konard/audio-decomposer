//! Turning planar audio into a FLAC stream.
//!
//! The encoder is a search, not a transform. For every block it works out what
//! the samples would cost described each of the ways the format allows — as a
//! constant, as themselves, as one of five fixed polynomial predictors, or as
//! coefficients fitted to this block in particular — and then writes only the
//! cheapest. Stereo gets the same treatment across channels: left and right, or
//! either one against their difference, whichever pair costs less.
//!
//! Every candidate reconstructs the input exactly. The choice between them is
//! only ever about size, which is why nothing here can cost fidelity.

use super::bits::{crc16, crc8, Writer};
use super::md5;
use super::{depth, write_stream_info, StreamInfo};
use crate::audio::Audio;
use crate::error::{Error, Result};

/// Samples per channel in a block, which is what most encoders settle on.
const BLOCK: usize = 4_096;

/// The highest fixed predictor order the format defines.
const MAX_FIXED_ORDER: usize = 4;

/// The highest fitted predictor order this encoder tries.
///
/// The format allows 32. Past a dozen the coefficients cost more to state than
/// the sharper prediction saves, which is where every other encoder stops too.
const MAX_LPC_ORDER: usize = 12;

/// Bits each fitted coefficient is stored in, including its sign.
const COEFFICIENT_PRECISION: u32 = 15;

/// The largest predictor shift the five-bit field can hold.
const MAX_SHIFT: i32 = 15;

/// The most partitions a residual may be split into, as `2^order`.
const MAX_PARTITION_ORDER: u32 = 8;

/// Rice parameters below this fit the narrower of the two coding methods.
const NARROW_PARAMETER_LIMIT: u32 = 14;

/// The largest Rice parameter the format can express.
const MAX_PARAMETER: u32 = 30;

/// Encodes a whole stream.
pub fn stream(audio: &Audio) -> Result<Vec<u8>> {
    let bits = depth(audio.format())?;
    if audio.sample_rate() >= 1 << 20 {
        return Err(Error::Unsupported(format!(
            "FLAC stores sample rates below 1048576 Hz, not {}",
            audio.sample_rate()
        )));
    }
    let channels = u32::try_from(audio.channel_count())
        .ok()
        .filter(|count| (1..=8).contains(count))
        .ok_or_else(|| Error::InvalidArgument("FLAC carries one to eight channels".into()))?;
    let codes = audio
        .codes()
        .ok_or_else(|| Error::InvalidArgument("integer format without grid".into()))?;
    let frames = audio.frames();

    let info = StreamInfo {
        // A fixed-block-size stream states one size for both bounds; the last
        // block being short is expected and does not widen the range.
        min_block: BLOCK as u32,
        max_block: BLOCK as u32,
        sample_rate: audio.sample_rate(),
        channels,
        bits,
        total_samples: frames as u64,
        md5: md5::of_samples(&codes, bits),
    };

    let mut writer = Writer::new();
    writer.unsigned(0x664C_6143, 32);
    write_stream_info(&mut writer, &info);

    let mut block = vec![Vec::with_capacity(BLOCK); codes.len()];
    for (number, start) in (0..frames).step_by(BLOCK).enumerate() {
        let end = (start + BLOCK).min(frames);
        for (plane, source) in block.iter_mut().zip(&codes) {
            plane.clear();
            plane.extend_from_slice(&source[start..end]);
        }
        frame(&mut writer, &block, bits, number as u64, &info);
    }
    Ok(writer.finish())
}

/// Writes one frame: the header, every channel, and the checksum over both.
fn frame(writer: &mut Writer, block: &[Vec<i64>], bits: u32, number: u64, info: &StreamInfo) {
    let (assignment, subframes) = channels(block, bits);
    let start = writer.written().len();
    debug_assert!(writer.is_aligned(), "a frame must start on a byte boundary");

    header(writer, block[0].len(), bits, assignment, number, info);
    let checked = crc8(&writer.written()[start..]);
    writer.unsigned(u64::from(checked), 8);

    for subframe in &subframes {
        writer.append(subframe);
    }
    writer.align();
    let sealed = crc16(&writer.written()[start..]);
    writer.unsigned(u64::from(sealed), 16);
}

/// Writes a frame header, up to but not including its CRC.
fn header(
    writer: &mut Writer,
    block_size: usize,
    bits: u32,
    assignment: u64,
    number: u64,
    info: &StreamInfo,
) {
    /// Block sizes with a code of their own, paired with that code.
    const SIZES: [(usize, u64); 13] = [
        (192, 1),
        (576, 2),
        (1_152, 3),
        (2_304, 4),
        (4_608, 5),
        (256, 8),
        (512, 9),
        (1_024, 10),
        (2_048, 11),
        (4_096, 12),
        (8_192, 13),
        (16_384, 14),
        (32_768, 15),
    ];
    /// Sample rates with a code of their own, paired with that code.
    const RATES: [(u32, u64); 11] = [
        (88_200, 1),
        (176_400, 2),
        (192_000, 3),
        (8_000, 4),
        (16_000, 5),
        (22_050, 6),
        (24_000, 7),
        (32_000, 8),
        (44_100, 9),
        (48_000, 10),
        (96_000, 11),
    ];

    let block_code = SIZES
        .iter()
        .find(|(size, _)| *size == block_size)
        .map_or(if block_size <= 256 { 6 } else { 7 }, |(_, code)| *code);
    let rate_code = RATES
        .iter()
        .find(|(rate, _)| *rate == info.sample_rate)
        .map_or(0, |(_, code)| *code);
    let depth_code = match bits {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        32 => 7,
        // Anything else is stated once, in STREAMINFO, and not repeated here.
        _ => 0,
    };

    writer.unsigned(0x3FFE, 14);
    writer.unsigned(0, 1); // reserved
    writer.unsigned(0, 1); // fixed block size, so the number below counts frames
    writer.unsigned(block_code, 4);
    writer.unsigned(rate_code, 4);
    writer.unsigned(assignment, 4);
    writer.unsigned(depth_code, 3);
    writer.unsigned(0, 1); // reserved
    writer.utf8_number(number);
    match block_code {
        6 => writer.unsigned(block_size as u64 - 1, 8),
        7 => writer.unsigned(block_size as u64 - 1, 16),
        _ => {}
    }
}

/// Chooses how the channels relate, and encodes them that way.
///
/// Two channels of music are mostly the same signal, so storing one of them as
/// a difference leaves far less for the predictor to explain. Which of the
/// three pairings wins depends on the music, so all four candidate channels are
/// encoded and the cheapest combination is kept.
fn channels(block: &[Vec<i64>], bits: u32) -> (u64, Vec<Writer>) {
    if block.len() != 2 {
        let independent = block
            .iter()
            .map(|plane| subframe(plane, bits))
            .collect::<Vec<_>>();
        return (block.len() as u64 - 1, independent);
    }

    let (left, right) = (&block[0], &block[1]);
    let side: Vec<i64> = left.iter().zip(right).map(|(l, r)| l - r).collect();
    // The average loses its low bit, which the difference still carries, so
    // dropping it here costs nothing.
    let mid: Vec<i64> = left.iter().zip(right).map(|(l, r)| (l + r) >> 1).collect();

    let left = subframe(left, bits);
    let right = subframe(right, bits);
    let mid = subframe(&mid, bits);
    // A difference needs one more bit of range than either channel it came from.
    let side = subframe(&side, bits + 1);

    let candidates = [
        (1_u64, left.len() + right.len()),
        (8, left.len() + side.len()),
        (9, side.len() + right.len()),
        (10, mid.len() + side.len()),
    ];
    let best = candidates
        .iter()
        .min_by_key(|(_, cost)| *cost)
        .map_or(1, |(code, _)| *code);
    match best {
        8 => (8, vec![left, side]),
        9 => (9, vec![side, right]),
        10 => (10, vec![mid, side]),
        _ => (1, vec![left, right]),
    }
}

/// How a block of one channel is described, and what describing it costs.
struct Candidate {
    predictor: Predictor,
    residual: Plan,
    /// Bits the whole subframe would take, header included.
    cost: u64,
}

/// The rule a candidate predicts the next sample with.
enum Predictor {
    /// One of the five polynomials the format defines, by order.
    Fixed(usize),
    /// Coefficients fitted to this block, with the shift they are scaled by.
    Fitted { coefficients: Vec<i64>, shift: u32 },
}

impl Predictor {
    /// Samples stored raw before the residual starts.
    fn order(&self) -> usize {
        match self {
            Self::Fixed(order) => *order,
            Self::Fitted { coefficients, .. } => coefficients.len(),
        }
    }

    /// Bits the predictor itself costs to state.
    fn overhead(&self) -> u64 {
        match self {
            Self::Fixed(_) => 0,
            Self::Fitted { coefficients, .. } => {
                4 + 5 + coefficients.len() as u64 * u64::from(COEFFICIENT_PRECISION)
            }
        }
    }
}

/// Encodes one channel of one block, the smallest way it can be described.
fn subframe(samples: &[i64], bits: u32) -> Writer {
    let mut writer = Writer::new();
    if samples.iter().all(|sample| *sample == samples[0]) {
        // Silence, and any held value, costs a header and one sample.
        head(&mut writer, 0, 0);
        writer.signed(samples[0], bits);
        return writer;
    }

    // Samples whose low bits are always zero — a quiet 16-bit master padded out
    // to 24 bits, say — are stored shifted down and the shift stated once.
    let wasted = samples
        .iter()
        .map(|sample| sample.trailing_zeros())
        .min()
        .unwrap_or(0)
        .min(bits - 1);
    let shifted: Vec<i64>;
    let samples = if wasted == 0 {
        samples
    } else {
        shifted = samples.iter().map(|sample| sample >> wasted).collect();
        &shifted
    };
    let bits = bits - wasted;

    // Verbatim is the floor: whatever the predictors make of the block, storing
    // the samples themselves is always available and always bounded.
    let verbatim = 7 + u64::from(bits) * samples.len() as u64;
    let best = candidates(samples, bits)
        .into_iter()
        .filter(|candidate| candidate.cost < verbatim)
        .min_by_key(|candidate| candidate.cost);

    if let Some(candidate) = best {
        predicted(&mut writer, samples, bits, wasted, &candidate);
    } else {
        head(&mut writer, 1, wasted);
        for sample in samples {
            writer.signed(*sample, bits);
        }
    }
    writer
}

/// Every predictor worth costing for this block.
fn candidates(samples: &[i64], bits: u32) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for order in 0..=MAX_FIXED_ORDER.min(samples.len()) {
        candidates.push(costed(Predictor::Fixed(order), samples, bits));
    }
    for predictor in fitted(samples) {
        candidates.push(costed(predictor, samples, bits));
    }
    candidates
}

/// Wraps a predictor and its planned residual into a costed candidate.
fn costed(predictor: Predictor, samples: &[i64], bits: u32) -> Candidate {
    let order = predictor.order();
    let errors = errors(samples, &predictor);
    let residual = plan(&errors, samples.len(), order);
    let cost = 7 + predictor.overhead() + u64::from(bits) * order as u64 + residual.cost;
    Candidate {
        predictor,
        residual,
        cost,
    }
}

/// What a predictor fails to guess, sample by sample.
fn errors(samples: &[i64], predictor: &Predictor) -> Vec<i64> {
    let order = predictor.order();
    (order..samples.len())
        .map(|index| samples[index] - predict(&samples[..index], predictor))
        .collect()
}

/// The value a predictor expects to come after `past`.
fn predict(past: &[i64], predictor: &Predictor) -> i64 {
    match predictor {
        Predictor::Fixed(order) => super::decode::fixed_prediction(past, *order),
        Predictor::Fitted {
            coefficients,
            shift,
        } => {
            let sum: i64 = coefficients
                .iter()
                .enumerate()
                .map(|(back, coefficient)| coefficient * past[past.len() - 1 - back])
                .sum();
            // An arithmetic shift, matching the decoder bit for bit; anything
            // else would make the residual restore the wrong samples.
            sum >> shift
        }
    }
}

/// Writes a subframe as a predictor plus the error it makes.
fn predicted(writer: &mut Writer, samples: &[i64], bits: u32, wasted: u32, candidate: &Candidate) {
    let order = candidate.predictor.order();
    match &candidate.predictor {
        Predictor::Fixed(fixed) => head(writer, 8 + *fixed as u64, wasted),
        Predictor::Fitted { .. } => head(writer, 31 + order as u64, wasted),
    }
    for sample in &samples[..order] {
        writer.signed(*sample, bits);
    }
    if let Predictor::Fitted {
        coefficients,
        shift,
    } = &candidate.predictor
    {
        writer.unsigned(u64::from(COEFFICIENT_PRECISION) - 1, 4);
        writer.unsigned(u64::from(*shift), 5);
        for coefficient in coefficients {
            writer.signed(*coefficient, COEFFICIENT_PRECISION);
        }
    }
    write_residual(writer, &candidate.residual, samples.len(), order);
}

/// Writes a subframe header: its type, and how many low bits were dropped.
fn head(writer: &mut Writer, kind: u64, wasted: u32) {
    writer.unsigned(0, 1);
    writer.unsigned(kind, 6);
    if wasted == 0 {
        writer.unsigned(0, 1);
    } else {
        writer.unsigned(1, 1);
        writer.unary(wasted - 1);
    }
}

/// How a residual will be split up and coded, and what that comes to.
struct Plan {
    /// The residual folded onto the naturals, ready to write.
    folded: Vec<u64>,
    partition_order: u32,
    parameters: Vec<u32>,
    cost: u64,
}

/// Chooses the partitioning that codes `errors` in the fewest bits.
fn plan(errors: &[i64], block_size: usize, order: usize) -> Plan {
    // Rice coding assumes small values around zero, so the residual is folded
    // onto the naturals: 0, -1, 1, -2 becomes 0, 1, 2, 3.
    let folded: Vec<u64> = errors
        .iter()
        .map(|error| ((error << 1) ^ (error >> 63)) as u64)
        .collect();

    let mut best: Option<(u32, Vec<u32>, u64)> = None;
    for partition_order in 0..=MAX_PARTITION_ORDER {
        let partitions = 1_usize << partition_order;
        if block_size % partitions != 0 {
            continue;
        }
        let per_partition = block_size >> partition_order;
        if per_partition < order {
            continue;
        }
        let mut parameters = Vec::with_capacity(partitions);
        // Every partition states its own parameter, in four or five bits.
        let mut cost = partitions as u64 * 5;
        let mut offset = 0;
        for partition in 0..partitions {
            let count = partition_length(per_partition, order, partition);
            let (parameter, partition_cost) = rice(&folded[offset..offset + count]);
            offset += count;
            parameters.push(parameter);
            cost += partition_cost;
        }
        if best.as_ref().is_none_or(|(_, _, kept)| cost < *kept) {
            best = Some((partition_order, parameters, cost));
        }
    }

    let (partition_order, parameters, cost) = best.unwrap_or_else(|| (0, vec![0], 0));
    Plan {
        folded,
        partition_order,
        parameters,
        // The method and the partition order are stated once, before them all.
        cost: cost + 2 + 4,
    }
}

/// Samples in one partition, the first being short by the predictor's warm-up.
const fn partition_length(per_partition: usize, order: usize, partition: usize) -> usize {
    if partition == 0 {
        per_partition - order
    } else {
        per_partition
    }
}

/// Writes a residual out according to its plan.
fn write_residual(writer: &mut Writer, plan: &Plan, block_size: usize, order: usize) {
    let narrow = plan
        .parameters
        .iter()
        .all(|parameter| *parameter < NARROW_PARAMETER_LIMIT);
    let parameter_bits = if narrow { 4 } else { 5 };
    writer.unsigned(u64::from(!narrow), 2);
    writer.unsigned(u64::from(plan.partition_order), 4);

    let per_partition = block_size >> plan.partition_order;
    let mut offset = 0;
    for (partition, parameter) in plan.parameters.iter().enumerate() {
        let count = partition_length(per_partition, order, partition);
        writer.unsigned(u64::from(*parameter), parameter_bits);
        for value in &plan.folded[offset..offset + count] {
            writer.unary((value >> parameter) as u32);
            writer.unsigned(*value, *parameter);
        }
        offset += count;
    }
}

/// The Rice parameter that codes `values` most cheaply, and what it costs.
fn rice(values: &[u64]) -> (u32, u64) {
    if values.is_empty() {
        return (0, 0);
    }
    let total: u64 = values
        .iter()
        .fold(0, |sum, value| sum.saturating_add(*value));
    let mean = total / values.len() as u64;
    // The best parameter is about the base-two logarithm of the mean, so only
    // its immediate neighbours are worth measuring exactly.
    let estimate = (u64::BITS - mean.leading_zeros()).saturating_sub(1);
    (estimate.saturating_sub(1)..=(estimate + 1).min(MAX_PARAMETER))
        .map(|parameter| (parameter, rice_cost(values, parameter)))
        .min_by_key(|(_, cost)| *cost)
        .unwrap_or_else(|| (0, rice_cost(values, 0)))
}

/// What coding `values` with `parameter` costs, in bits.
fn rice_cost(values: &[u64], parameter: u32) -> u64 {
    values.iter().fold(0_u64, |sum, value| {
        sum.saturating_add((value >> parameter) + 1 + u64::from(parameter))
    })
}

/// Fits predictors of every useful order to this block in particular.
///
/// The fixed predictors assume the signal is a smooth polynomial. Real music is
/// not: a bowed string or a struck skin has resonances that a predictor fitted
/// to the block itself can follow and a general one cannot. This is where most
/// of FLAC's compression actually comes from.
fn fitted(samples: &[i64]) -> Vec<Predictor> {
    let max_order = MAX_LPC_ORDER.min(samples.len().saturating_sub(1));
    if max_order == 0 {
        return Vec::new();
    }
    let correlation = autocorrelation(samples, max_order);
    if correlation[0] <= 0.0 {
        return Vec::new();
    }
    levinson_durbin(&correlation, max_order)
        .into_iter()
        .filter_map(|coefficients| quantise(&coefficients))
        .collect()
}

/// How much the windowed block resembles itself, at each lag up to `max_order`.
fn autocorrelation(samples: &[i64], max_order: usize) -> Vec<f64> {
    // A window tapers the block's edges, so the fit is not thrown off by the
    // discontinuity where the block was cut out of the music.
    let length = samples.len() as f64;
    let taper = (length * 0.25).max(1.0);
    let windowed: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let position = index as f64;
            let edge = (position + 0.5).min(length - 0.5 - position);
            let weight = if edge >= taper {
                1.0
            } else {
                0.5 * (1.0 - (std::f64::consts::PI * edge / taper).cos())
            };
            *sample as f64 * weight
        })
        .collect();

    (0..=max_order)
        .map(|lag| {
            windowed[lag..]
                .iter()
                .zip(&windowed)
                .map(|(later, earlier)| later * earlier)
                .sum()
        })
        .collect()
}

/// Solves for the best coefficients of every order, the cheapest way there is.
fn levinson_durbin(correlation: &[f64], max_order: usize) -> Vec<Vec<f64>> {
    let mut coefficients = vec![0.0_f64; max_order];
    let mut error = correlation[0];
    let mut orders = Vec::with_capacity(max_order);

    for order in 0..max_order {
        let mut reflection = correlation[order + 1];
        for lag in 0..order {
            reflection -= coefficients[lag] * correlation[order - lag];
        }
        reflection /= error;
        coefficients[order] = reflection;
        // Each new order corrects the previous ones, symmetrically inward.
        for lag in 0..order / 2 {
            let kept = coefficients[lag];
            coefficients[lag] = kept - reflection * coefficients[order - 1 - lag];
            coefficients[order - 1 - lag] -= reflection * kept;
        }
        if order % 2 == 1 {
            coefficients[order / 2] -= reflection * coefficients[order / 2];
        }

        if !coefficients[..=order].iter().all(|value| value.is_finite()) {
            break;
        }
        orders.push(coefficients[..=order].to_vec());
        error *= 1.0 - reflection * reflection;
        if error <= 0.0 || !error.is_finite() {
            break;
        }
    }
    orders
}

/// Puts fitted coefficients on an integer grid the format can carry.
///
/// The decoder multiplies in integers and shifts back down, so the coefficients
/// have to be integers scaled by a power of two. The rounding error of each one
/// is carried into the next, which keeps the scaled sum from drifting.
fn quantise(coefficients: &[f64]) -> Option<Predictor> {
    let largest = coefficients
        .iter()
        .copied()
        .fold(0.0_f64, |largest, value| largest.max(value.abs()));
    if largest <= 0.0 || !largest.is_finite() {
        return None;
    }
    let headroom = COEFFICIENT_PRECISION as i32 - 1;
    let shift = headroom - largest.log2().floor() as i32 - 1;
    if shift < 0 {
        // The coefficients are too large to scale onto the grid; a lower order,
        // or a fixed predictor, will have to carry this block.
        return None;
    }
    let shift = shift.min(MAX_SHIFT);

    let scale = f64::from(1_u32 << shift);
    let limit = 1_i64 << (COEFFICIENT_PRECISION - 1);
    let mut carried = 0.0_f64;
    let mut quantised = Vec::with_capacity(coefficients.len());
    for coefficient in coefficients {
        carried += coefficient * scale;
        let rounded = carried.round().clamp(-(limit as f64), (limit - 1) as f64) as i64;
        carried -= rounded as f64;
        quantised.push(rounded);
    }
    if quantised.iter().all(|coefficient| *coefficient == 0) {
        return None;
    }
    Some(Predictor::Fitted {
        coefficients: quantised,
        shift: shift as u32,
    })
}
