//! RIFF/WAVE reading and writing.
//!
//! The reader accepts 8/16/24/32-bit integer PCM, 32/64-bit IEEE float, and the
//! `WAVE_FORMAT_EXTENSIBLE` wrapper around both, skipping any auxiliary chunk it
//! does not need. The writer emits the canonical chunk layout, so a file this
//! crate wrote and read back is byte-identical.

use std::fs;
use std::path::Path;

use crate::audio::{Audio, SampleFormat};
use crate::error::{Error, Result};
use crate::format_error;

const FORMAT_PCM: u16 = 1;
const FORMAT_IEEE_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Reads a WAV file from disk.
pub fn read_file(path: impl AsRef<Path>) -> Result<Audio> {
    let bytes = fs::read(path.as_ref())?;
    decode(&bytes)
}

/// Writes `audio` to disk as a WAV file.
pub fn write_file(path: impl AsRef<Path>, audio: &Audio) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path.as_ref(), encode(audio)?)?;
    Ok(())
}

/// Decodes a WAV file held in memory.
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format_error!("not a RIFF/WAVE file"));
    }

    let mut position = 12;
    let mut format: Option<WaveFormat> = None;
    let mut data: Option<&[u8]> = None;

    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = read_u32(bytes, position + 4)? as usize;
        let body_start = position + 8;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format_error!("chunk `{}` runs past end of file", chunk_name(id)))?;

        match id {
            b"fmt " => format = Some(parse_format(&bytes[body_start..body_end])?),
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }

        // RIFF chunks are word aligned: an odd size is followed by a pad byte.
        position = body_end + usize::from(size % 2 == 1);
    }

    let format = format.ok_or_else(|| format_error!("missing `fmt ` chunk"))?;
    let data = data.ok_or_else(|| format_error!("missing `data` chunk"))?;
    decode_samples(&format, data)
}

/// Encodes `audio` as a WAV file in memory.
pub fn encode(audio: &Audio) -> Result<Vec<u8>> {
    let format = audio.format();
    let channels = u16::try_from(audio.channel_count())
        .map_err(|_| Error::InvalidArgument("too many channels for WAV".into()))?;
    let bytes_per_sample = format.bytes();
    let block_align = channels as usize * bytes_per_sample;
    let data_size = audio.frames() * block_align;

    let mut data = Vec::with_capacity(data_size);
    match format {
        SampleFormat::F32 => write_planar(audio, &mut data, |value, out| {
            out.extend_from_slice(&(value as f32).to_le_bytes());
        }),
        SampleFormat::F64 => write_planar(audio, &mut data, |value, out| {
            out.extend_from_slice(&value.to_le_bytes());
        }),
        _ => {
            let codes = audio
                .codes()
                .ok_or_else(|| Error::InvalidArgument("integer format without grid".into()))?;
            let frames = audio.frames();
            for frame in 0..frames {
                for channel in &codes {
                    write_code(format, channel[frame], &mut data);
                }
            }
        }
    }

    let float = format.is_float();
    let format_tag = if float { FORMAT_IEEE_FLOAT } else { FORMAT_PCM };
    // Non-PCM formats require `cbSize` and a `fact` chunk (RIFF specification).
    let fmt_size: u32 = if float { 18 } else { 16 };
    let fact_size: u32 = if float { 12 } else { 0 };
    let riff_size = 4 + (8 + fmt_size) + fact_size + 8 + data_size as u32 + (data_size % 2) as u32;

    let mut out = Vec::with_capacity(riff_size as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_size.to_le_bytes());
    out.extend_from_slice(&format_tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&audio.sample_rate().to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate() * block_align as u32).to_le_bytes());
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&format.bits().to_le_bytes());
    if float {
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(b"fact");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(audio.frames() as u32).to_le_bytes());
    }

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    out.extend_from_slice(&data);
    if data_size % 2 == 1 {
        out.push(0);
    }
    Ok(out)
}

fn write_planar(audio: &Audio, out: &mut Vec<u8>, mut write: impl FnMut(f64, &mut Vec<u8>)) {
    for frame in 0..audio.frames() {
        for channel in audio.channels() {
            write(channel[frame], out);
        }
    }
}

fn write_code(format: SampleFormat, code: i64, out: &mut Vec<u8>) {
    match format {
        SampleFormat::PcmU8 => out.push((code + 128) as u8),
        SampleFormat::PcmI16 => out.extend_from_slice(&(code as i16).to_le_bytes()),
        SampleFormat::PcmI24 => out.extend_from_slice(&(code as i32).to_le_bytes()[0..3]),
        _ => out.extend_from_slice(&(code as i32).to_le_bytes()),
    }
}

#[derive(Debug)]
struct WaveFormat {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

fn parse_format(body: &[u8]) -> Result<WaveFormat> {
    if body.len() < 16 {
        return Err(format_error!("`fmt ` chunk shorter than 16 bytes"));
    }
    let mut tag = read_u16(body, 0)?;
    let channels = read_u16(body, 2)?;
    let sample_rate = read_u32(body, 4)?;
    let bits = read_u16(body, 14)?;

    if tag == FORMAT_EXTENSIBLE {
        if body.len() < 40 {
            return Err(format_error!(
                "extensible `fmt ` chunk shorter than 40 bytes"
            ));
        }
        // The first two bytes of the sub-format GUID carry the real format tag.
        tag = read_u16(body, 24)?;
    }

    if channels == 0 {
        return Err(format_error!("channel count is zero"));
    }
    if sample_rate == 0 {
        return Err(format_error!("sample rate is zero"));
    }
    Ok(WaveFormat {
        tag,
        channels,
        sample_rate,
        bits,
    })
}

fn decode_samples(format: &WaveFormat, data: &[u8]) -> Result<Audio> {
    let sample_format = match (format.tag, format.bits) {
        (FORMAT_PCM, 8) => SampleFormat::PcmU8,
        (FORMAT_PCM, 16) => SampleFormat::PcmI16,
        (FORMAT_PCM, 24) => SampleFormat::PcmI24,
        (FORMAT_PCM, 32) => SampleFormat::PcmI32,
        (FORMAT_IEEE_FLOAT, 32) => SampleFormat::F32,
        (FORMAT_IEEE_FLOAT, 64) => SampleFormat::F64,
        (tag, bits) => {
            return Err(Error::Unsupported(format!(
                "WAVE format tag {tag} with {bits} bits per sample"
            )));
        }
    };

    let channel_count = format.channels as usize;
    let bytes_per_sample = sample_format.bytes();
    let block_align = channel_count * bytes_per_sample;
    let frames = data.len() / block_align;
    let mut channels = vec![Vec::with_capacity(frames); channel_count];

    for frame in 0..frames {
        let frame_start = frame * block_align;
        for (index, channel) in channels.iter_mut().enumerate() {
            let offset = frame_start + index * bytes_per_sample;
            channel.push(decode_sample(
                sample_format,
                &data[offset..offset + bytes_per_sample],
            ));
        }
    }

    Audio::from_channels(format.sample_rate, sample_format, channels)
}

fn decode_sample(format: SampleFormat, bytes: &[u8]) -> f64 {
    match format {
        SampleFormat::PcmU8 => f64::from(i16::from(bytes[0]) - 128) / 128.0,
        SampleFormat::PcmI16 => f64::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
        SampleFormat::PcmI24 => {
            // Sign-extend the 24-bit value by shifting it into the top of an i32.
            let raw = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
            f64::from(raw) / 8_388_608.0
        }
        SampleFormat::PcmI32 => {
            f64::from(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                / 2_147_483_648.0
        }
        SampleFormat::F32 => {
            f64::from(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
        SampleFormat::F64 => f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
    }
}

fn chunk_name(id: &[u8]) -> String {
    String::from_utf8_lossy(id).to_string()
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
        .ok_or_else(|| format_error!("truncated 16-bit field at offset {offset}"))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or_else(|| format_error!("truncated 32-bit field at offset {offset}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(format: SampleFormat, channels: usize, frames: usize) -> Audio {
        let quantum = format.quantum().unwrap_or(1.0 / 1024.0);
        let data = (0..channels)
            .map(|channel| {
                (0..frames)
                    .map(|frame| ((frame * (channel + 1)) as f64 - 8.0) * quantum)
                    .collect()
            })
            .collect();
        Audio::from_channels(44_100, format, data).unwrap()
    }

    #[test]
    fn every_format_round_trips_through_memory() {
        for format in [
            SampleFormat::PcmU8,
            SampleFormat::PcmI16,
            SampleFormat::PcmI24,
            SampleFormat::PcmI32,
            SampleFormat::F32,
            SampleFormat::F64,
        ] {
            let audio = ramp(format, 2, 17);
            let encoded = encode(&audio).unwrap();
            let decoded = decode(&encoded).unwrap();

            assert_eq!(decoded.format(), format, "format {format:?}");
            assert_eq!(decoded.sample_rate(), 44_100);
            assert_eq!(decoded.channels(), audio.channels(), "samples {format:?}");
            assert_eq!(encode(&decoded).unwrap(), encoded, "bytes {format:?}");
        }
    }

    #[test]
    fn header_matches_the_canonical_layout() {
        let audio = ramp(SampleFormat::PcmI16, 2, 4);
        let bytes = encode(&audio).unwrap();

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(read_u32(&bytes, 16).unwrap(), 16);
        assert_eq!(read_u16(&bytes, 20).unwrap(), FORMAT_PCM);
        assert_eq!(read_u16(&bytes, 22).unwrap(), 2);
        assert_eq!(read_u32(&bytes, 24).unwrap(), 44_100);
        assert_eq!(read_u32(&bytes, 28).unwrap(), 44_100 * 4);
        assert_eq!(read_u16(&bytes, 32).unwrap(), 4);
        assert_eq!(read_u16(&bytes, 34).unwrap(), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(read_u32(&bytes, 40).unwrap() as usize, 4 * 4);
        assert_eq!(bytes.len(), 44 + 16);
        assert_eq!(read_u32(&bytes, 4).unwrap() as usize, bytes.len() - 8);
    }

    #[test]
    fn float_files_carry_a_fact_chunk() {
        let bytes = encode(&ramp(SampleFormat::F32, 1, 3)).unwrap();

        assert_eq!(read_u16(&bytes, 20).unwrap(), FORMAT_IEEE_FLOAT);
        assert_eq!(read_u32(&bytes, 16).unwrap(), 18);
        assert_eq!(&bytes[38..42], b"fact");
        assert_eq!(read_u32(&bytes, 46).unwrap(), 3);
    }

    #[test]
    fn unknown_chunks_and_pad_bytes_are_skipped() {
        let audio = ramp(SampleFormat::PcmU8, 1, 3);
        let canonical = encode(&audio).unwrap();

        // Rebuild the file with an odd-sized LIST chunk in front of `data`.
        let mut patched = Vec::new();
        patched.extend_from_slice(&canonical[0..36]);
        patched.extend_from_slice(b"LIST");
        patched.extend_from_slice(&3u32.to_le_bytes());
        patched.extend_from_slice(b"abc\0");
        patched.extend_from_slice(&canonical[36..]);
        let size = (patched.len() - 8) as u32;
        patched[4..8].copy_from_slice(&size.to_le_bytes());

        let decoded = decode(&patched).unwrap();
        assert_eq!(decoded.channels(), audio.channels());
    }

    #[test]
    fn extensible_headers_resolve_to_their_sub_format() {
        let mut body = vec![0u8; 40];
        body[0..2].copy_from_slice(&FORMAT_EXTENSIBLE.to_le_bytes());
        body[2..4].copy_from_slice(&1u16.to_le_bytes());
        body[4..8].copy_from_slice(&8_000u32.to_le_bytes());
        body[14..16].copy_from_slice(&16u16.to_le_bytes());
        body[24..26].copy_from_slice(&FORMAT_PCM.to_le_bytes());

        let format = parse_format(&body).unwrap();
        assert_eq!(format.tag, FORMAT_PCM);

        let audio = decode_samples(&format, &[0x00, 0x40, 0x00, 0xC0]).unwrap();
        assert_eq!(audio.format(), SampleFormat::PcmI16);
        assert_eq!(audio.channel(0), &[0.5, -0.5]);
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(decode(b"not a wav at all").is_err());

        let audio = ramp(SampleFormat::PcmI16, 1, 2);
        let canonical = encode(&audio).unwrap();

        let mut without_fmt = canonical.clone();
        without_fmt[12..16].copy_from_slice(b"junk");
        assert!(decode(&without_fmt).is_err());

        let mut without_data = canonical.clone();
        without_data[36..40].copy_from_slice(b"junk");
        assert!(decode(&without_data).is_err());

        let mut oversized = canonical;
        oversized[40..44].copy_from_slice(&9_999u32.to_le_bytes());
        assert!(decode(&oversized).is_err());

        let mut short_fmt = vec![0u8; 8];
        short_fmt[0..2].copy_from_slice(&FORMAT_PCM.to_le_bytes());
        assert!(parse_format(&short_fmt).is_err());
    }

    #[test]
    fn unsupported_encodings_report_their_tag() {
        let format = WaveFormat {
            tag: 0x0011,
            channels: 1,
            sample_rate: 8_000,
            bits: 4,
        };
        let error = decode_samples(&format, &[0; 4]).unwrap_err();

        assert!(matches!(error, Error::Unsupported(_)));
        assert!(error.to_string().contains("17"));
    }

    #[test]
    fn files_round_trip_through_the_filesystem() {
        let directory = std::env::temp_dir().join(format!(
            "audio-decomposer-wav-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("nested").join("tone.wav");
        let audio = ramp(SampleFormat::PcmI24, 2, 9);

        write_file(&path, &audio).unwrap();
        let decoded = read_file(&path).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(decoded.channels(), audio.channels());
    }
}
