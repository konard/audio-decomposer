//! Cutting a recording into the events that become bank samples.
//!
//! An event is a half-open range of frames on one channel. Events tile the
//! timeline without overlapping, which is what makes the first pass of the
//! decomposition exact: if every event is stored as its own bank sample and
//! placed back where it came from, the rendering reproduces the recording. The
//! deduplication pass then replaces as many of those samples as it can with
//! references to earlier ones, and only the difference it fails to explain
//! reaches the residual.
//!
//! Boundaries come from spectral-flux onsets. By default they are detected on
//! the mono mix so that every channel is cut in the same places, which is what
//! lets identical left and right content collapse into one bank sample with two
//! placements.

use crate::audio::Audio;
use crate::dsp::onset::{self, OnsetOptions};
use crate::dsp::stft::{self, StftOptions};
use crate::error::Result;

/// How a recording is cut into events.
#[derive(Clone, Copy, Debug)]
pub struct EventOptions {
    /// Transform used for onset detection.
    pub stft: StftOptions,
    /// Onset detector settings.
    pub onset: OnsetOptions,
    /// Shortest event, in seconds. A boundary closer than this to the last
    /// one kept is dropped, so the short run joins the event that follows it.
    pub minimum_seconds: f64,
    /// Longest event, in seconds. Longer stretches are split evenly, so a
    /// sustained passage still offers repeatable chunks to deduplicate.
    pub maximum_seconds: f64,
    /// Detect onsets on each channel separately instead of on the mono mix.
    pub per_channel: bool,
    /// Snap each boundary to the attack it belongs to. Without this the
    /// boundaries land wherever the transform's window did, and two
    /// occurrences of one sound are cut in different places, which is enough
    /// to stop them from deduplicating.
    pub refine: bool,
}

impl Default for EventOptions {
    fn default() -> Self {
        Self {
            stft: StftOptions::default(),
            onset: OnsetOptions::default(),
            minimum_seconds: 0.01,
            maximum_seconds: 2.0,
            per_channel: false,
            refine: true,
        }
    }
}

/// A stretch of one channel that is analysed as a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Channel the event belongs to.
    pub channel: usize,
    /// First frame of the event.
    pub start: usize,
    /// Number of frames.
    pub length: usize,
}

impl Event {
    /// An event from its parts.
    #[must_use]
    pub const fn new(channel: usize, start: usize, length: usize) -> Self {
        Self {
            channel,
            start,
            length,
        }
    }

    /// One past the last frame.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.start + self.length
    }

    /// The samples of this event, read from a buffer.
    #[must_use]
    pub fn slice<'a>(&self, channels: &'a [Vec<f64>]) -> &'a [f64] {
        let Some(channel) = channels.get(self.channel) else {
            return &[];
        };
        let start = self.start.min(channel.len());
        let end = self.end().min(channel.len());
        &channel[start..end]
    }
}

/// Finds the frame boundaries a signal should be cut at.
pub fn boundaries(samples: &[f64], sample_rate: u32, options: &EventOptions) -> Result<Vec<usize>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let spectrogram = stft::forward(samples, sample_rate, options.stft)?;
    let mut onsets = onset::detect(&spectrogram, options.onset);
    if options.refine {
        let span = options.stft.fft_size + options.stft.hop;
        let block = (options.stft.fft_size / 32).max(8);
        onset::refine(&mut onsets, samples, sample_rate, span, block);
    }
    let ranges = onset::segments(&onsets, samples.len());
    let minimum = seconds_to_frames(options.minimum_seconds, sample_rate).max(1);
    let maximum = seconds_to_frames(options.maximum_seconds, sample_rate);
    Ok(shape(&ranges, samples.len(), minimum, maximum))
}

/// Cuts a recording into events.
pub fn detect(audio: &Audio, options: &EventOptions) -> Result<Vec<Event>> {
    if audio.frames() == 0 || audio.channel_count() == 0 {
        return Ok(Vec::new());
    }
    let sample_rate = audio.sample_rate();
    let mut events = Vec::new();
    if options.per_channel {
        for (index, channel) in audio.channels().iter().enumerate() {
            let cuts = boundaries(channel, sample_rate, options)?;
            events.extend(events_from(index, &cuts, audio.frames()));
        }
    } else {
        let cuts = boundaries(&audio.mono_mix(), sample_rate, options)?;
        for index in 0..audio.channel_count() {
            events.extend(events_from(index, &cuts, audio.frames()));
        }
    }
    events.sort_by_key(|event| (event.start, event.channel));
    Ok(events)
}

/// Turns a list of boundaries into the events of one channel.
fn events_from(channel: usize, cuts: &[usize], frames: usize) -> Vec<Event> {
    let mut events = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        events.push(Event::new(channel, pair[0], pair[1] - pair[0]));
    }
    if let (Some(&last), true) = (cuts.last(), !cuts.is_empty()) {
        if last < frames {
            events.push(Event::new(channel, last, frames - last));
        }
    }
    events
}

/// Applies the length limits to detected ranges, returning cut positions.
///
/// A boundary closer than `minimum` to the previous one is dropped; a stretch
/// longer than `maximum` is divided into equal pieces.
fn shape(ranges: &[(usize, usize)], length: usize, minimum: usize, maximum: usize) -> Vec<usize> {
    let mut cuts = vec![0_usize];
    for &(start, end) in ranges {
        if start > 0 && start.saturating_sub(*cuts.last().unwrap_or(&0)) >= minimum {
            cuts.push(start);
        }
        let _ = end;
    }
    if maximum > 0 {
        let mut divided = Vec::with_capacity(cuts.len());
        for index in 0..cuts.len() {
            let start = cuts[index];
            let end = cuts.get(index + 1).copied().unwrap_or(length);
            divided.push(start);
            let span = end.saturating_sub(start);
            if span > maximum {
                let pieces = span.div_ceil(maximum);
                let piece = span.div_ceil(pieces);
                let mut position = start + piece;
                while position < end && end - position >= minimum {
                    divided.push(position);
                    position += piece;
                }
            }
        }
        cuts = divided;
    }
    cuts.retain(|cut| *cut < length);
    cuts.dedup();
    cuts
}

/// Converts a duration in seconds to a whole number of frames.
#[must_use]
pub fn seconds_to_frames(seconds: f64, sample_rate: u32) -> usize {
    if seconds <= 0.0 || sample_rate == 0 {
        return 0;
    }
    (seconds * f64::from(sample_rate)).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleFormat;

    /// Three short bursts separated by silence.
    fn bursts(sample_rate: u32, positions: &[usize], length: usize) -> Vec<f64> {
        let mut signal = vec![0.0; length];
        for &position in positions {
            for index in 0..3000 {
                let Some(slot) = signal.get_mut(position + index) else {
                    break;
                };
                let time = index as f64 / f64::from(sample_rate);
                let decay = (-30.0 * time).exp();
                *slot += decay * (std::f64::consts::TAU * 440.0 * time).sin();
            }
        }
        signal
    }

    #[test]
    fn events_tile_the_whole_recording() {
        let sample_rate = 22_050;
        let frames = 22_050;
        let signal = bursts(sample_rate, &[0, 7000, 14_000], frames);
        let audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();

        let events = detect(&audio, &EventOptions::default()).unwrap();

        assert!(events.len() >= 3, "expected at least one event per burst");
        assert_eq!(events[0].start, 0);
        assert_eq!(events.last().unwrap().end(), frames);
        for pair in events.windows(2) {
            assert_eq!(pair[0].end(), pair[1].start, "events must not have gaps");
        }
    }

    #[test]
    fn every_channel_is_cut_in_the_same_places_by_default() {
        let sample_rate = 22_050;
        let frames = 12_000;
        let signal = bursts(sample_rate, &[0, 6000], frames);
        let audio = Audio::from_channels(
            sample_rate,
            SampleFormat::PcmI16,
            vec![signal.clone(), signal],
        )
        .unwrap();

        let events = detect(&audio, &EventOptions::default()).unwrap();

        let left: Vec<usize> = events
            .iter()
            .filter(|event| event.channel == 0)
            .map(|event| event.start)
            .collect();
        let right: Vec<usize> = events
            .iter()
            .filter(|event| event.channel == 1)
            .map(|event| event.start)
            .collect();
        assert_eq!(left, right);
        assert_eq!(events.len(), left.len() * 2);
    }

    #[test]
    fn long_stretches_are_divided_into_equal_pieces() {
        let cuts = shape(&[(0, 1000)], 1000, 10, 300);

        assert_eq!(cuts, vec![0, 250, 500, 750]);
    }

    #[test]
    fn boundaries_too_close_together_are_dropped() {
        let cuts = shape(&[(0, 100), (100, 105), (105, 400)], 400, 50, 0);

        assert_eq!(
            cuts,
            vec![0, 100],
            "the boundary five frames after the last one is not kept"
        );
    }

    #[test]
    fn empty_and_degenerate_input_yields_no_events() {
        let empty = Audio::silence(8000, SampleFormat::PcmI16, 1, 0);
        assert!(detect(&empty, &EventOptions::default()).unwrap().is_empty());

        let no_channels = Audio::silence(8000, SampleFormat::PcmI16, 0, 16);
        assert!(detect(&no_channels, &EventOptions::default())
            .unwrap()
            .is_empty());
        assert!(boundaries(&[], 8000, &EventOptions::default())
            .unwrap()
            .is_empty());
        assert_eq!(seconds_to_frames(-1.0, 8000), 0);
        assert_eq!(seconds_to_frames(1.0, 0), 0);
        assert_eq!(seconds_to_frames(0.5, 8000), 4000);
    }

    #[test]
    fn an_event_reads_its_own_samples() {
        let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
        assert_eq!(Event::new(0, 1, 2).slice(&channels), &[2.0, 3.0]);
        assert_eq!(Event::new(0, 3, 9).slice(&channels), &[4.0]);
        assert_eq!(Event::new(1, 0, 2).slice(&channels), &[] as &[f64]);
        assert_eq!(Event::new(0, 2, 2).end(), 4);
    }

    #[test]
    fn per_channel_detection_is_available() {
        let sample_rate = 22_050;
        let frames = 12_000;
        let left = bursts(sample_rate, &[0], frames);
        let right = bursts(sample_rate, &[6000], frames);
        let audio =
            Audio::from_channels(sample_rate, SampleFormat::PcmI16, vec![left, right]).unwrap();

        let options = EventOptions {
            per_channel: true,
            ..EventOptions::default()
        };
        let events = detect(&audio, &options).unwrap();

        assert!(events.iter().any(|event| event.channel == 0));
        assert!(events.iter().any(|event| event.channel == 1));
    }
}
