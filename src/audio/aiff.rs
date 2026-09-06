//! AIFF and AIFF-C reading and writing.
//!
//! GarageBand and Logic import AIFF natively, so every exported stem and every
//! deduplicated sample can be written in a format those hosts open directly.
//! Integer AIFF samples are big-endian and signed (including 8-bit, which WAV
//! stores unsigned instead), and the sample rate is an 80-bit IEEE extended
//! float; both conversions are exact, so the round trip stays lossless.

use std::fs;
use std::path::Path;

use crate::audio::{Audio, SampleFormat};
use crate::error::{Error, Result};
use crate::format_error;

/// Reads an AIFF or AIFF-C file from disk.
pub fn read_file(path: impl AsRef<Path>) -> Result<Audio> {
    decode(&fs::read(path.as_ref())?)
}

/// Writes `audio` to disk as an AIFF file.
pub fn write_file(path: impl AsRef<Path>, audio: &Audio) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path.as_ref(), encode(audio)?)?;
    Ok(())
}

/// Decodes an AIFF or AIFF-C file held in memory.
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    if bytes.len() < 12 || &bytes[0..4] != b"FORM" {
        return Err(format_error!("not a FORM/AIFF file"));
    }
    let kind = &bytes[8..12];
    if kind != b"AIFF" && kind != b"AIFC" {
        return Err(format_error!(
            "unsupported FORM type `{}`",
            String::from_utf8_lossy(kind)
        ));
    }

    let mut position = 12;
    let mut common: Option<Common> = None;
    let mut sound: Option<&[u8]> = None;

    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = read_u32(bytes, position + 4)? as usize;
        let body_start = position + 8;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format_error!("chunk runs past end of file"))?;

        match id {
            b"COMM" => common = Some(parse_common(&bytes[body_start..body_end])?),
            b"SSND" => {
                let body = &bytes[body_start..body_end];
                if body.len() < 8 {
                    return Err(format_error!("`SSND` chunk shorter than 8 bytes"));
                }
                let offset = read_u32(body, 0)? as usize;
                sound = Some(
                    body.get(8 + offset..)
                        .ok_or_else(|| format_error!("`SSND` offset past end of chunk"))?,
                );
            }
            _ => {}
        }

        position = body_end + usize::from(size % 2 == 1);
    }

    let common = common.ok_or_else(|| format_error!("missing `COMM` chunk"))?;
    let sound = sound.ok_or_else(|| format_error!("missing `SSND` chunk"))?;
    decode_samples(&common, sound)
}

/// Encodes `audio` as an AIFF file in memory.
pub fn encode(audio: &Audio) -> Result<Vec<u8>> {
    let format = audio.format();
    if format.is_float() {
        return Err(Error::Unsupported(
            "AIFF export covers integer PCM only; use WAV for float stems".into(),
        ));
    }
    let channels = u16::try_from(audio.channel_count())
        .map_err(|_| Error::InvalidArgument("too many channels for AIFF".into()))?;
    let codes = audio
        .codes()
        .ok_or_else(|| Error::InvalidArgument("integer format without grid".into()))?;
    let frames = audio.frames();

    let mut sound = Vec::with_capacity(frames * channels as usize * format.bytes() + 8);
    sound.extend_from_slice(&0u32.to_be_bytes()); // offset
    sound.extend_from_slice(&0u32.to_be_bytes()); // block size
    for frame in 0..frames {
        for channel in &codes {
            write_code(format, channel[frame], &mut sound);
        }
    }

    let mut common = Vec::with_capacity(18);
    common.extend_from_slice(&channels.to_be_bytes());
    common.extend_from_slice(&(frames as u32).to_be_bytes());
    common.extend_from_slice(&format.bits().to_be_bytes());
    common.extend_from_slice(&encode_extended(f64::from(audio.sample_rate())));

    let body_size = 4 + (8 + common.len()) + (8 + sound.len());
    let mut out = Vec::with_capacity(body_size + 8);
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&(body_size as u32).to_be_bytes());
    out.extend_from_slice(b"AIFF");
    out.extend_from_slice(b"COMM");
    out.extend_from_slice(&(common.len() as u32).to_be_bytes());
    out.extend_from_slice(&common);
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&(sound.len() as u32).to_be_bytes());
    out.extend_from_slice(&sound);
    Ok(out)
}

#[derive(Debug)]
struct Common {
    channels: u16,
    frames: u32,
    bits: u16,
    sample_rate: u32,
}

fn parse_common(body: &[u8]) -> Result<Common> {
    if body.len() < 18 {
        return Err(format_error!("`COMM` chunk shorter than 18 bytes"));
    }
    if body.len() >= 22 {
        let compression = &body[18..22];
        if compression != b"NONE" {
            return Err(Error::Unsupported(format!(
                "AIFF-C compression `{}`",
                String::from_utf8_lossy(compression)
            )));
        }
    }
    let channels = read_u16(body, 0)?;
    let frames = read_u32(body, 2)?;
    let bits = read_u16(body, 6)?;
    let sample_rate = decode_extended(&body[8..18])?;
    if channels == 0 {
        return Err(format_error!("channel count is zero"));
    }
    Ok(Common {
        channels,
        frames,
        bits,
        sample_rate,
    })
}

fn decode_samples(common: &Common, data: &[u8]) -> Result<Audio> {
    let format = match common.bits {
        8 => SampleFormat::PcmU8,
        16 => SampleFormat::PcmI16,
        24 => SampleFormat::PcmI24,
        32 => SampleFormat::PcmI32,
        bits => {
            return Err(Error::Unsupported(format!(
                "AIFF with {bits} bits per sample"
            )));
        }
    };
    let quantum = format.quantum().expect("integer format has a grid");
    let channel_count = common.channels as usize;
    let block_align = channel_count * format.bytes();
    let available = data.len() / block_align;
    let frames = (common.frames as usize).min(available);
    let mut channels = vec![Vec::with_capacity(frames); channel_count];

    for frame in 0..frames {
        let frame_start = frame * block_align;
        for (index, channel) in channels.iter_mut().enumerate() {
            let offset = frame_start + index * format.bytes();
            let code = decode_code(format, &data[offset..offset + format.bytes()]);
            channel.push(code as f64 * quantum);
        }
    }

    Audio::from_channels(common.sample_rate, format, channels)
}

fn decode_code(format: SampleFormat, bytes: &[u8]) -> i64 {
    match format {
        SampleFormat::PcmU8 => i64::from(bytes[0] as i8),
        SampleFormat::PcmI16 => i64::from(i16::from_be_bytes([bytes[0], bytes[1]])),
        SampleFormat::PcmI24 => {
            i64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], 0]) >> 8)
        }
        _ => i64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
    }
}

fn write_code(format: SampleFormat, code: i64, out: &mut Vec<u8>) {
    match format {
        SampleFormat::PcmU8 => out.push(code as i8 as u8),
        SampleFormat::PcmI16 => out.extend_from_slice(&(code as i16).to_be_bytes()),
        SampleFormat::PcmI24 => out.extend_from_slice(&(code as i32).to_be_bytes()[1..4]),
        _ => out.extend_from_slice(&(code as i32).to_be_bytes()),
    }
}

/// Encodes a positive sample rate as an 80-bit IEEE 754 extended float.
fn encode_extended(value: f64) -> [u8; 10] {
    let mut out = [0u8; 10];
    if value <= 0.0 {
        return out;
    }
    let exponent = value.log2().floor() as i32;
    let mantissa = (value / 2.0_f64.powi(exponent) * 2.0_f64.powi(63)).round() as u64;
    let biased = (exponent + 16_383) as u16;
    out[0..2].copy_from_slice(&biased.to_be_bytes());
    out[2..10].copy_from_slice(&mantissa.to_be_bytes());
    out
}

/// Decodes an 80-bit IEEE 754 extended float into a whole sample rate.
fn decode_extended(bytes: &[u8]) -> Result<u32> {
    if bytes.len() < 10 {
        return Err(format_error!("truncated extended float"));
    }
    let biased = u16::from_be_bytes([bytes[0], bytes[1]]);
    let sign = biased & 0x8000 != 0;
    let exponent = i32::from(biased & 0x7FFF) - 16_383;
    let mantissa = u64::from_be_bytes([
        bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
    ]);
    if sign || mantissa == 0 {
        return Err(format_error!("sample rate is not a positive number"));
    }
    let value = mantissa as f64 / 2.0_f64.powi(63) * 2.0_f64.powi(exponent);
    if !value.is_finite() || value < 1.0 || value > f64::from(u32::MAX) {
        return Err(format_error!("sample rate {value} out of range"));
    }
    Ok(value.round() as u32)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_be_bytes([slice[0], slice[1]]))
        .ok_or_else(|| format_error!("truncated 16-bit field at offset {offset}"))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or_else(|| format_error!("truncated 32-bit field at offset {offset}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(format: SampleFormat, channels: usize, frames: usize) -> Audio {
        let quantum = format.quantum().unwrap();
        let data = (0..channels)
            .map(|channel| {
                (0..frames)
                    .map(|frame| ((frame * (channel + 1)) as f64 - 4.0) * quantum)
                    .collect()
            })
            .collect();
        Audio::from_channels(48_000, format, data).unwrap()
    }

    #[test]
    fn integer_formats_round_trip() {
        for format in [
            SampleFormat::PcmU8,
            SampleFormat::PcmI16,
            SampleFormat::PcmI24,
            SampleFormat::PcmI32,
        ] {
            let audio = ramp(format, 2, 11);
            let encoded = encode(&audio).unwrap();
            let decoded = decode(&encoded).unwrap();

            assert_eq!(decoded.format(), format);
            assert_eq!(decoded.sample_rate(), 48_000);
            assert_eq!(decoded.channels(), audio.channels(), "{format:?}");
            assert_eq!(encode(&decoded).unwrap(), encoded);
        }
    }

    #[test]
    fn header_matches_the_specification() {
        let bytes = encode(&ramp(SampleFormat::PcmI16, 1, 5)).unwrap();

        assert_eq!(&bytes[0..4], b"FORM");
        assert_eq!(read_u32(&bytes, 4).unwrap() as usize, bytes.len() - 8);
        assert_eq!(&bytes[8..12], b"AIFF");
        assert_eq!(&bytes[12..16], b"COMM");
        assert_eq!(read_u32(&bytes, 16).unwrap(), 18);
        assert_eq!(read_u16(&bytes, 20).unwrap(), 1);
        assert_eq!(read_u32(&bytes, 22).unwrap(), 5);
        assert_eq!(read_u16(&bytes, 26).unwrap(), 16);
        assert_eq!(&bytes[38..42], b"SSND");
    }

    #[test]
    fn extended_floats_survive_the_common_sample_rates() {
        for rate in [8_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
            let encoded = encode_extended(f64::from(rate));
            assert_eq!(decode_extended(&encoded).unwrap(), rate);
        }
        assert!(decode_extended(&[0; 10]).is_err());
        assert!(decode_extended(&[0; 4]).is_err());
        assert_eq!(encode_extended(-1.0), [0; 10]);
    }

    #[test]
    fn float_export_is_refused_with_a_pointer_to_wav() {
        let audio = Audio::silence(44_100, SampleFormat::F32, 1, 4);
        let error = encode(&audio).unwrap_err();

        assert!(matches!(error, Error::Unsupported(_)));
        assert!(error.to_string().contains("WAV"));
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(decode(b"not an aiff").is_err());

        let mut wrong_kind = encode(&ramp(SampleFormat::PcmI16, 1, 2)).unwrap();
        wrong_kind[8..12].copy_from_slice(b"AIFX");
        assert!(decode(&wrong_kind).is_err());

        let mut missing_common = encode(&ramp(SampleFormat::PcmI16, 1, 2)).unwrap();
        missing_common[12..16].copy_from_slice(b"JUNK");
        assert!(decode(&missing_common).is_err());

        let mut missing_sound = encode(&ramp(SampleFormat::PcmI16, 1, 2)).unwrap();
        missing_sound[38..42].copy_from_slice(b"JUNK");
        assert!(decode(&missing_sound).is_err());
    }

    #[test]
    fn compressed_aifc_and_odd_depths_are_reported() {
        let mut body = vec![0u8; 22];
        body[0..2].copy_from_slice(&1u16.to_be_bytes());
        body[6..8].copy_from_slice(&16u16.to_be_bytes());
        body[8..18].copy_from_slice(&encode_extended(44_100.0));
        body[18..22].copy_from_slice(b"ima4");
        assert!(matches!(
            parse_common(&body).unwrap_err(),
            Error::Unsupported(_)
        ));

        body[18..22].copy_from_slice(b"NONE");
        let common = parse_common(&body).unwrap();
        assert_eq!(common.sample_rate, 44_100);

        let odd = Common {
            channels: 1,
            frames: 1,
            bits: 12,
            sample_rate: 44_100,
        };
        assert!(matches!(
            decode_samples(&odd, &[0; 4]).unwrap_err(),
            Error::Unsupported(_)
        ));
    }

    #[test]
    fn files_round_trip_through_the_filesystem() {
        let directory = std::env::temp_dir().join(format!(
            "audio-decomposer-aiff-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("tone.aiff");
        let audio = ramp(SampleFormat::PcmI16, 1, 6);

        write_file(&path, &audio).unwrap();
        let decoded = read_file(&path).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(decoded.channels(), audio.channels());
    }
}
