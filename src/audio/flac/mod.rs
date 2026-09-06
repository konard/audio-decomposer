//! FLAC reading and writing.
//!
//! Public-domain music is distributed compressed, and the archives that hold it
//! — the Internet Archive, Musopen, the European libraries that scan and record
//! out-of-copyright scores — publish FLAC. A tool that can only open WAV can
//! only open what someone already converted for it.
//!
//! FLAC is the one compressed format that costs this crate nothing in
//! fidelity: it is lossless, so a decomposition of a FLAC file still
//! reconstructs to the original bit for bit, and the guarantee the rest of the
//! pipeline makes survives intact. The codec is written here rather than pulled
//! in, for the same reason WAV, AIFF, ZIP, GZIP, XML and MIDI are.
//!
//! ```no_run
//! use audio_decomposer::audio::flac;
//!
//! let audio = flac::read_file("prelude.flac")?;
//! flac::write_file("prelude-again.flac", &audio)?;
//! # Ok::<(), audio_decomposer::Error>(())
//! ```

mod bits;
mod decode;
mod encode;
mod md5;

use std::fs;
use std::path::Path;

use crate::audio::{Audio, SampleFormat};
use crate::error::{Error, Result};
use crate::format_error;

use bits::{Reader, Writer};

/// The block type of STREAMINFO, which every stream opens with.
const STREAMINFO: u64 = 0;

/// What a stream says about itself before any audio arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Smallest block the stream uses, in samples per channel.
    pub min_block: u32,
    /// Largest block the stream uses, in samples per channel.
    pub max_block: u32,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u32,
    /// Bits per sample.
    pub bits: u32,
    /// Samples per channel in the whole stream, or zero when not known.
    pub total_samples: u64,
    /// MD5 of the samples as they were before encoding, or zeros when unset.
    pub md5: [u8; 16],
}

/// Reads a FLAC file from disk.
pub fn read_file(path: impl AsRef<Path>) -> Result<Audio> {
    decode(&fs::read(path.as_ref())?)
}

/// Writes `audio` to disk as a FLAC file.
pub fn write_file(path: impl AsRef<Path>, audio: &Audio) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path.as_ref(), encode(audio)?)?;
    Ok(())
}

/// Decodes a FLAC stream held in memory.
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    let mut reader = Reader::new(bytes);
    if reader.unsigned(32)? != 0x664C_6143 {
        return Err(format_error!("not a FLAC stream: missing `fLaC` marker"));
    }
    let info = metadata(&mut reader)?;

    let (format, shift) = storage(info.bits)?;
    let mut channels = vec![Vec::new(); info.channels as usize];
    while more_frames(&reader, &info, channels[0].len()) {
        let (header, block) = decode::frame(&mut reader, &info)?;
        if block.len() != channels.len() {
            return Err(format_error!(
                "FLAC frame has {} channels where the stream declared {}",
                block.len(),
                channels.len()
            ));
        }
        if header.sample_rate != info.sample_rate {
            return Err(format_error!(
                "FLAC frame runs at {} Hz where the stream declared {}",
                header.sample_rate,
                info.sample_rate
            ));
        }
        if header.bits != info.bits {
            return Err(format_error!(
                "FLAC frame stores {} bits where the stream declared {}",
                header.bits,
                info.bits
            ));
        }
        for (channel, decoded) in channels.iter_mut().zip(block) {
            channel.extend(decoded);
        }
    }

    if info.total_samples > 0 {
        let decoded = channels[0].len() as u64;
        if decoded != info.total_samples {
            return Err(format_error!(
                "FLAC stream declared {} samples but carries {decoded}",
                info.total_samples
            ));
        }
    }
    // The digest covers the samples as the stream stores them, so it has to be
    // checked before they are lifted onto a wider grid.
    verify_md5(&info, &channels)?;
    if shift > 0 {
        for channel in &mut channels {
            for code in channel.iter_mut() {
                *code <<= shift;
            }
        }
    }
    Audio::from_codes(info.sample_rate, format, &channels)
}

/// Encodes `audio` as a FLAC stream in memory.
pub fn encode(audio: &Audio) -> Result<Vec<u8>> {
    encode::stream(audio)
}

/// Whether another frame is expected, and present.
fn more_frames(reader: &Reader, info: &StreamInfo, decoded: usize) -> bool {
    if info.total_samples > 0 {
        return (decoded as u64) < info.total_samples;
    }
    // Without a declared length the stream simply ends. Anything left that is
    // too short to hold a frame, or that does not open with a sync code, is
    // trailing metadata rather than audio.
    let rest = reader.rest();
    rest.len() >= 6 && rest[0] == 0xFF && rest[1] & 0xFC == 0xF8
}

/// Reads every metadata block, returning the STREAMINFO all streams open with.
fn metadata(reader: &mut Reader) -> Result<StreamInfo> {
    let mut info = None;
    loop {
        let last = reader.bit()? == 1;
        let kind = reader.unsigned(7)?;
        let length = reader.unsigned(24)? as usize;
        if kind == STREAMINFO {
            if info.is_some() {
                return Err(format_error!("FLAC stream declares STREAMINFO twice"));
            }
            if length != 34 {
                return Err(format_error!(
                    "FLAC STREAMINFO is {length} bytes rather than 34"
                ));
            }
            info = Some(stream_info(reader)?);
        } else {
            // Seek tables, tags, cue sheets and pictures describe the stream
            // but not its samples, so nothing here needs them.
            reader.skip_bytes(length)?;
        }
        if last {
            break;
        }
    }
    let info = info.ok_or_else(|| format_error!("FLAC stream has no STREAMINFO"))?;
    if info.channels == 0 || info.channels > 8 {
        return Err(format_error!(
            "FLAC stream declares {} channels",
            info.channels
        ));
    }
    if info.sample_rate == 0 {
        return Err(format_error!("FLAC stream declares a sample rate of zero"));
    }
    Ok(info)
}

/// Reads the body of a STREAMINFO block.
fn stream_info(reader: &mut Reader) -> Result<StreamInfo> {
    let min_block = reader.unsigned(16)? as u32;
    let max_block = reader.unsigned(16)? as u32;
    let _min_frame = reader.unsigned(24)?;
    let _max_frame = reader.unsigned(24)?;
    let sample_rate = reader.unsigned(20)? as u32;
    let channels = reader.unsigned(3)? as u32 + 1;
    let bits = reader.unsigned(5)? as u32 + 1;
    let total_samples = reader.unsigned(36)?;
    let mut md5 = [0u8; 16];
    for byte in &mut md5 {
        *byte = reader.unsigned(8)? as u8;
    }
    Ok(StreamInfo {
        min_block,
        max_block,
        sample_rate,
        channels,
        bits,
        total_samples,
        md5,
    })
}

/// Writes a STREAMINFO block, marked as the last metadata in the stream.
fn write_stream_info(writer: &mut Writer, info: &StreamInfo) {
    writer.unsigned(1, 1); // last metadata block
    writer.unsigned(STREAMINFO, 7);
    writer.unsigned(34, 24);
    writer.unsigned(u64::from(info.min_block), 16);
    writer.unsigned(u64::from(info.max_block), 16);
    writer.unsigned(0, 24); // smallest frame, which nothing needs to decode
    writer.unsigned(0, 24); // largest frame, likewise
    writer.unsigned(u64::from(info.sample_rate), 20);
    writer.unsigned(u64::from(info.channels - 1), 3);
    writer.unsigned(u64::from(info.bits - 1), 5);
    writer.unsigned(info.total_samples, 36);
    for byte in info.md5 {
        writer.unsigned(u64::from(byte), 8);
    }
}

/// Checks the decoded samples against the digest the stream carries.
///
/// A stream may leave the digest zeroed, which the specification allows and
/// which means only that nothing can be checked.
fn verify_md5(info: &StreamInfo, codes: &[Vec<i64>]) -> Result<()> {
    if info.md5 == [0u8; 16] {
        return Ok(());
    }
    let digest = md5::of_samples(codes, info.bits);
    if digest != info.md5 {
        return Err(format_error!(
            "FLAC stream fails its MD5 check: the decoded audio is not what was encoded"
        ));
    }
    Ok(())
}

/// The sample format that holds `bits` exactly, and the shift onto its grid.
///
/// FLAC counts from the top of the sample, so a 20-bit code is the high 20 bits
/// of a 24-bit one; shifting rather than scaling keeps that exact.
fn storage(bits: u32) -> Result<(SampleFormat, u32)> {
    SampleFormat::ALL
        .iter()
        .copied()
        .find(|format| !format.is_float() && u32::from(format.bits()) >= bits)
        .map(|format| (format, u32::from(format.bits()) - bits))
        .ok_or_else(|| Error::Unsupported(format!("FLAC stream stores {bits} bits per sample")))
}

/// Bits per sample the encoder writes for a buffer's format.
fn depth(format: SampleFormat) -> Result<u32> {
    if format.is_float() {
        return Err(Error::Unsupported(
            "FLAC export covers integer PCM only; use WAV for float stems".into(),
        ));
    }
    Ok(u32::from(format.bits()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A short buffer with something for every predictor to bite on.
    fn fixture(format: SampleFormat, channels: usize, frames: usize) -> Audio {
        let quantum = format.quantum().unwrap();
        let (low, high) = format.code_range().unwrap();
        let planes = (0..channels)
            .map(|channel| {
                (0..frames)
                    .map(|frame| {
                        let phase = frame as f64 / 32.0 + channel as f64;
                        let code = (phase.sin() * (high as f64 * 0.4)) as i64
                            + (frame as i64 % 7) * (1 + channel as i64);
                        code.clamp(low, high) as f64 * quantum
                    })
                    .collect()
            })
            .collect();
        Audio::from_channels(44_100, format, planes).unwrap()
    }

    #[test]
    fn a_stream_reads_back_as_the_audio_it_was_written_from() {
        for format in [
            SampleFormat::PcmU8,
            SampleFormat::PcmI16,
            SampleFormat::PcmI24,
            SampleFormat::PcmI32,
        ] {
            for channels in [1, 2, 3] {
                let audio = fixture(format, channels, 3_000);
                let encoded = encode(&audio).unwrap();
                let decoded = decode(&encoded).unwrap();
                assert_eq!(decoded.format(), format);
                assert_eq!(decoded.sample_rate(), audio.sample_rate());
                assert_eq!(
                    decoded.channels(),
                    audio.channels(),
                    "{} channels of {}",
                    channels,
                    format.name()
                );
            }
        }
    }

    #[test]
    fn compression_actually_makes_the_file_smaller() {
        // A predictable waveform is what the fixed predictors exist for. If a
        // sine costs as much as raw PCM, the encoder is not encoding anything.
        let audio = fixture(SampleFormat::PcmI16, 2, 20_000);
        let raw = audio.frames() * audio.channel_count() * 2;
        let encoded = encode(&audio).unwrap();
        assert!(
            encoded.len() < raw / 2,
            "{} bytes for {raw} bytes of PCM",
            encoded.len()
        );
    }

    #[test]
    fn silence_costs_almost_nothing() {
        let audio = Audio::silence(44_100, SampleFormat::PcmI16, 2, 100_000);
        let encoded = encode(&audio).unwrap();
        assert!(encoded.len() < 2_000, "{} bytes of silence", encoded.len());
        assert_eq!(decode(&encoded).unwrap().channels(), audio.channels());
    }

    #[test]
    fn a_single_frame_and_an_odd_length_survive() {
        for frames in [1, 2, 17, 4_095, 4_096, 4_097] {
            let audio = fixture(SampleFormat::PcmI16, 2, frames);
            let decoded = decode(&encode(&audio).unwrap()).unwrap();
            assert_eq!(decoded.channels(), audio.channels(), "{frames} frames");
        }
    }

    #[test]
    fn the_digest_is_written_and_checked() {
        let audio = fixture(SampleFormat::PcmI16, 2, 1_000);
        let mut encoded = encode(&audio).unwrap();
        assert_ne!(&encoded[26..42], &[0u8; 16], "digest was left unset");

        // Corrupt a sample without touching the CRCs that guard the frame: the
        // digest is the only thing that can still notice.
        let digest_start = 26;
        encoded[digest_start] ^= 0xFF;
        let error = decode(&encoded).unwrap_err();
        assert!(
            matches!(error, Error::Format(ref message) if message.contains("MD5")),
            "{error}"
        );
    }

    #[test]
    fn float_export_is_refused_with_a_pointer_to_wav() {
        let audio = Audio::silence(44_100, SampleFormat::F32, 1, 8);
        let error = encode(&audio).unwrap_err();
        assert!(matches!(error, Error::Unsupported(_)), "{error}");
    }

    #[test]
    fn a_truncated_stream_is_reported_rather_than_panicking() {
        let audio = fixture(SampleFormat::PcmI16, 1, 5_000);
        let encoded = encode(&audio).unwrap();
        for cut in [4, 20, 45, 100, encoded.len() - 1] {
            assert!(decode(&encoded[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn something_that_is_not_a_flac_stream_is_refused() {
        let error = decode(b"RIFF....WAVEfmt ").unwrap_err();
        assert!(matches!(error, Error::Format(_)), "{error}");
    }

    #[test]
    fn corruption_is_caught_by_the_frame_checks() {
        let audio = fixture(SampleFormat::PcmI16, 1, 2_000);
        let mut encoded = encode(&audio).unwrap();
        let last = encoded.len() - 8;
        encoded[last] ^= 0x01;
        assert!(decode(&encoded).is_err());
    }
}
