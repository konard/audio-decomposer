//! Taking a recording apart.
//!
//! The decomposer runs a matching pursuit over the recording. It cuts the
//! timeline into events, walks them in time order, and for each one asks
//! whether some waveform it has already stored can be subtracted at some
//! position to remove it. When one can, the event costs a reference; when none
//! can, the event's audio — as it stands *after* every earlier subtraction —
//! becomes a new entry in the bank. Either way the audio is subtracted from the
//! working signal, so later events are matched against what is genuinely left.
//!
//! The pursuit is only an analysis. The reconstruction is defined by
//! [`Decomposition::render`] alone, and the residual is computed against that
//! rendering at the very end:
//!
//! ```text
//! residual = original - render(bank, placements)
//! ```
//!
//! so however well or badly the matching does, adding the residual back
//! restores the recording. For integer PCM sources it restores it sample for
//! sample; for float sources a second correction term is stored when the sum
//! would otherwise round.
//!
//! ```no_run
//! use audio_decomposer::decompose::{decompose, DecomposeOptions};
//! # fn main() -> audio_decomposer::Result<()> {
//! # let recording = audio_decomposer::Audio::silence(44_100, audio_decomposer::SampleFormat::PcmI16, 2, 0);
//! let decomposition = decompose(&recording, &DecomposeOptions::default())?;
//! assert_eq!(decomposition.reconstruct(), recording);
//! # Ok(())
//! # }
//! ```

pub mod dedup;
pub mod events;
pub mod model;
pub mod notes;

use std::collections::HashMap;

use crate::audio::{Audio, SampleFormat};
use crate::dsp::envelope;
use crate::dsp::hpss::{self, HpssOptions};
use crate::dsp::stft::{self, StftOptions};
use crate::error::Result;

pub use dedup::MatchOptions;
pub use events::{Event, EventOptions};
pub use model::{Decomposition, Gain, Note, Placement, Sample, SourceInfo, Stem};
pub use notes::NoteOptions;

/// Which parts a recording is split into before deduplication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StemMode {
    /// Keep the recording whole.
    None,
    /// Split it into a harmonic and a percussive part, plus the correction
    /// that makes the two sum back exactly.
    #[default]
    HarmonicPercussive,
}

/// Everything the decomposer needs to know.
#[derive(Clone, Debug)]
pub struct DecomposeOptions {
    /// Name of the recording, used for exported file names.
    pub name: String,
    /// How the timeline is cut into events.
    pub events: EventOptions,
    /// How willing the matcher is to reuse a waveform.
    pub matching: MatchOptions,
    /// How notes are recognised.
    pub notes: NoteOptions,
    /// How the recording is split into stems.
    pub stems: StemMode,
    /// Transform used for the stem separation.
    pub stft: StftOptions,
    /// Stem separation settings.
    pub hpss: HpssOptions,
    /// How far a match may be shifted, in seconds. This fills in
    /// [`MatchOptions::search_radius`], which is measured in frames.
    pub search_seconds: f64,
    /// How many stored waveforms are tried per event.
    pub candidates: usize,
    /// Root-mean-square amplitude below which a stretch is considered already
    /// explained and left to the residual.
    pub silence: f64,
    /// Whether notes are recognised at all.
    pub recognise_notes: bool,
}

impl Default for DecomposeOptions {
    fn default() -> Self {
        Self {
            name: "recording".to_string(),
            events: EventOptions::default(),
            matching: MatchOptions::default(),
            notes: NoteOptions::default(),
            stems: StemMode::default(),
            stft: StftOptions::default(),
            hpss: HpssOptions::default(),
            search_seconds: 0.01,
            candidates: 24,
            silence: 1e-6,
            recognise_notes: true,
        }
    }
}

impl DecomposeOptions {
    /// Options that only ever reuse a waveform when it repeats verbatim.
    ///
    /// The bank is larger, every placement is exact, and the analysis is much
    /// faster, which makes this the right setting for looped or programmed
    /// material.
    #[must_use]
    pub const fn exact_repeats_only(mut self) -> Self {
        self.matching.tolerance = 0.0;
        self.search_seconds = 0.0;
        self
    }

    /// Options with the analysis reduced to what the reconstruction needs.
    #[must_use]
    pub const fn without_analysis(mut self) -> Self {
        self.stems = StemMode::None;
        self.recognise_notes = false;
        self
    }
}

/// A configured decomposer.
#[derive(Clone, Debug, Default)]
pub struct Decomposer {
    options: DecomposeOptions,
}

impl Decomposer {
    /// A decomposer with the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A decomposer with the given options.
    #[must_use]
    pub const fn with_options(options: DecomposeOptions) -> Self {
        Self { options }
    }

    /// The options in use.
    #[must_use]
    pub const fn options(&self) -> &DecomposeOptions {
        &self.options
    }

    /// Takes a recording apart.
    pub fn decompose(&self, audio: &Audio) -> Result<Decomposition> {
        decompose(audio, &self.options)
    }
}

/// Takes a recording apart.
pub fn decompose(audio: &Audio, options: &DecomposeOptions) -> Result<Decomposition> {
    let source = SourceInfo::of(&options.name, audio);
    let stems = separate(audio, options)?;
    let (samples, placements) = build_bank(audio, options)?;

    let mut decomposition = Decomposition {
        source,
        stems,
        samples,
        placements,
        notes: Vec::new(),
        residual: audio.clone(),
        correction: None,
    };
    if options.recognise_notes {
        decomposition.notes = notes::detect(audio, &options.notes)?;
        label_samples(&mut decomposition, options);
    }
    close(&mut decomposition, audio)?;
    Ok(decomposition)
}

/// Tags a derived buffer with the narrowest format that still holds it exactly.
///
/// Differences of grid values land back on that same grid, so the residual of a
/// 16-bit recording is 16-bit data even though nothing constrained it to be.
/// Tagging every one of them `f64` "to be safe" is lossless but quadruples what
/// the archive, the stems and every exported project spend on them. `floor`
/// stops the search short of formats finer than the recording itself.
fn tag_exactly(buffer: &mut Audio, floor: SampleFormat) {
    let format = buffer.narrowest_exact_format(floor);
    buffer.set_format(format);
}

/// Computes the residual, and the correction that absorbs the rounding a float
/// source can still show.
fn close(decomposition: &mut Decomposition, audio: &Audio) -> Result<()> {
    let rendered = decomposition.render();
    let mut residual = audio.difference(&rendered)?;
    tag_exactly(&mut residual, audio.format());
    decomposition.residual = residual;

    let restored = decomposition.reconstruct();
    let mut correction = audio.difference(&restored)?;
    if correction.peak() > 0.0 {
        tag_exactly(&mut correction, audio.format());
        decomposition.correction = Some(correction);
    }
    Ok(())
}

/// Splits a recording into stems that sum back to it exactly.
fn separate(audio: &Audio, options: &DecomposeOptions) -> Result<Vec<Stem>> {
    if options.stems == StemMode::None || audio.frames() == 0 || audio.channel_count() == 0 {
        return Ok(vec![Stem::new("full".to_string(), audio.clone())]);
    }

    let mut harmonic = Vec::with_capacity(audio.channel_count());
    let mut percussive = Vec::with_capacity(audio.channel_count());
    for channel in audio.channels() {
        let spectrogram = stft::forward(channel, audio.sample_rate(), options.stft)?;
        let separation = hpss::separate(&spectrogram, options.hpss)?;
        harmonic.push(fit(stft::inverse(&separation.harmonic)?, channel.len()));
        percussive.push(fit(stft::inverse(&separation.percussive)?, channel.len()));
    }

    let mut harmonic = Audio::from_channels(audio.sample_rate(), audio.format(), harmonic)?;
    let mut percussive = Audio::from_channels(audio.sample_rate(), audio.format(), percussive)?;
    harmonic.quantize();
    percussive.quantize();

    let mut sum = Audio::silence(
        audio.sample_rate(),
        audio.format(),
        audio.channel_count(),
        audio.frames(),
    );
    sum.add_assign(&harmonic)?;
    sum.add_assign(&percussive)?;
    let mut correction = audio.difference(&sum)?;
    tag_exactly(&mut correction, audio.format());

    Ok(vec![
        Stem::new("harmonic".to_string(), harmonic),
        Stem::new("percussive".to_string(), percussive),
        Stem::new("correction".to_string(), correction),
    ])
}

/// Trims or pads a reconstructed channel to the length it must have.
fn fit(mut samples: Vec<f64>, length: usize) -> Vec<f64> {
    samples.resize(length, 0.0);
    samples
}

/// Runs the matching pursuit that produces the bank and its placements.
///
/// The pursuit runs over the timeline twice. Onset positions are only accurate
/// to a few samples, and when a boundary lands after the attack it belongs to,
/// the waveform taken from the previous event carries the first frames of the
/// next one, which then get rendered twice: once from that waveform's tail and
/// once from the placement of the repeat. Matching reports where each repeat
/// really starts, so the first pass is used to move the boundaries there and
/// the second pass builds the bank from boundaries that agree with the audio.
/// When the first pass already agrees with them nothing is recomputed.
fn build_bank(audio: &Audio, options: &DecomposeOptions) -> Result<(Vec<Sample>, Vec<Placement>)> {
    let mut matching = options.matching;
    matching.search_radius = events::seconds_to_frames(options.search_seconds, audio.sample_rate());
    let floor = silence_floor(audio, options);
    let found = events::detect(audio, &options.events)?;

    let first = pursue(audio, &found, &matching, options, floor);
    if matching.search_radius == 0 {
        return Ok((first.samples, first.placements));
    }
    let realigned = realign(&found, &first.starts);
    if realigned == found {
        return Ok((first.samples, first.placements));
    }
    let second = pursue(audio, &realigned, &matching, options, floor);
    Ok((second.samples, second.placements))
}

/// What one pass of the pursuit produced.
struct Pursuit {
    /// The waveforms stored by this pass.
    samples: Vec<Sample>,
    /// Where each of them was placed.
    placements: Vec<Placement>,
    /// The position chosen for each event, in the order the events were given,
    /// or `None` for an event that held nothing worth storing.
    starts: Vec<Option<i64>>,
}

/// Amplitude below which a window counts as empty.
///
/// Content quieter than half a step of the source grid cannot survive the
/// quantization [`model::render`] ends with, so storing it would only fill the
/// bank with waveforms that render as silence.
fn silence_floor(audio: &Audio, options: &DecomposeOptions) -> f64 {
    let grid = audio.format().quantum().map_or(0.0, |step| step / 2.0);
    options.silence.max(grid)
}

/// One pass of the pursuit over a fixed set of events.
fn pursue(
    audio: &Audio,
    found: &[Event],
    matching: &MatchOptions,
    options: &DecomposeOptions,
    floor: f64,
) -> Pursuit {
    let mut remaining: Vec<Vec<f64>> = audio.channels().to_vec();
    let mut samples: Vec<Sample> = Vec::new();
    let mut by_fingerprint: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut placements = Vec::new();
    let mut starts = Vec::with_capacity(found.len());

    for event in found {
        let window = event.slice(&remaining).to_vec();
        if window.is_empty() || envelope::rms(&window) <= floor {
            starts.push(None);
            continue;
        }

        let reused = reuse(
            &window,
            event,
            &samples,
            &by_fingerprint,
            &remaining,
            matching,
            options.candidates,
        );
        let placement = reused.unwrap_or_else(|| {
            let id = samples.len();
            by_fingerprint
                .entry(dedup::fingerprint(&window))
                .or_default()
                .push(id);
            samples.push(Sample::new(id, format!("sample-{:04}", id + 1), window));
            Placement::new(id, event.channel, event.start as i64, Gain::UNIT)
        });

        dedup::subtract(
            &mut remaining[placement.channel],
            placement.start,
            &samples[placement.sample].waveform,
            placement.gain,
        );
        starts.push(Some(placement.start));
        placements.push(placement);
    }

    Pursuit {
        samples,
        placements,
        starts,
    }
}

/// Moves each event boundary to the position the matcher chose for it, keeping
/// the events of a channel in order and still tiling the timeline.
///
/// A position is only taken when it stays inside the event and after the
/// boundary before it; anything else would reorder the timeline, so the
/// original boundary is kept instead.
fn realign(found: &[Event], starts: &[Option<i64>]) -> Vec<Event> {
    let mut moved = found.to_vec();
    let channels = found
        .iter()
        .map(|event| event.channel)
        .max()
        .map_or(0, |last| last + 1);
    for channel in 0..channels {
        let indices: Vec<usize> = found
            .iter()
            .enumerate()
            .filter(|(_, event)| event.channel == channel)
            .map(|(index, _)| index)
            .collect();
        let mut previous: Option<usize> = None;
        for index in indices {
            let event = found[index];
            let mut start = match starts[index] {
                Some(start) if start >= 0 && (start as usize) < event.end() => start as usize,
                _ => event.start,
            };
            if let Some(before) = previous {
                if start <= moved[before].start {
                    start = event.start.max(moved[before].start + 1);
                }
                moved[before].length = start - moved[before].start;
            }
            moved[index].start = start;
            previous = Some(index);
        }
        if let Some(last) = previous {
            moved[last].length = found[last].end().saturating_sub(moved[last].start);
        }
    }
    moved
}

/// Looks for a stored waveform that explains this window.
fn reuse(
    window: &[f64],
    event: &Event,
    samples: &[Sample],
    by_fingerprint: &HashMap<u64, Vec<usize>>,
    remaining: &[Vec<f64>],
    matching: &MatchOptions,
    candidate_limit: usize,
) -> Option<Placement> {
    let start = event.start as i64;
    if let Some(identical) = by_fingerprint
        .get(&dedup::fingerprint(window))
        .into_iter()
        .flatten()
        .find(|id| samples[**id].waveform == window)
    {
        return Some(Placement::new(*identical, event.channel, start, Gain::UNIT));
    }

    let signal = remaining.get(event.channel)?;
    let mut best: Option<(f64, Placement)> = None;
    for id in candidates(window.len(), samples, candidate_limit) {
        let sample = &samples[id];
        let Some(alignment) = dedup::best_alignment(signal, start, &sample.waveform, matching)
        else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(remainder, _)| alignment.remainder < *remainder)
        {
            best = Some((
                alignment.remainder,
                Placement::new(id, event.channel, alignment.start, alignment.gain),
            ));
        }
    }
    best.map(|(_, placement)| placement)
}

/// Orders the stored waveforms worth trying for a window of this length.
///
/// A waveform far longer or shorter than the window cannot explain it, and
/// trying every remaining one would make the pursuit quadratic in the size of
/// the bank, so the list is cut to the closest lengths, most recent first.
fn candidates(length: usize, samples: &[Sample], limit: usize) -> Vec<usize> {
    if length == 0 {
        return Vec::new();
    }
    let mut ranked: Vec<(f64, usize)> = samples
        .iter()
        .filter(|sample| !sample.is_empty())
        .map(|sample| {
            let ratio = sample.len() as f64 / length as f64;
            (ratio.log2().abs(), sample.id)
        })
        .filter(|(distance, _)| *distance <= 1.0)
        .collect();
    ranked.sort_by(|left, right| left.0.total_cmp(&right.0).then(right.1.cmp(&left.1)));
    ranked.truncate(limit);
    ranked.into_iter().map(|(_, id)| id).collect()
}

/// Names the bank samples with the notes they sound.
fn label_samples(decomposition: &mut Decomposition, options: &DecomposeOptions) {
    let sample_rate = decomposition.source.sample_rate;
    for sample in &mut decomposition.samples {
        if let Some((note, frequency, _)) =
            notes::identify(&sample.waveform, sample_rate, options.notes.pitch)
        {
            sample.note = Some(note);
            sample.frequency = Some(frequency);
            sample.name = format!(
                "{}-{}",
                sample.name,
                crate::dsp::pitch::note_name(i32::from(note))
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(sample_rate: u32, frequency: f64, length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / f64::from(sample_rate);
                let decay = (-8.0 * time).exp();
                0.6 * decay
                    * ((std::f64::consts::TAU * frequency * time).sin()
                        + 0.4 * (std::f64::consts::TAU * 2.0 * frequency * time).sin())
                    / 1.4
            })
            .collect()
    }

    /// The same note struck several times at exactly the same amplitude, with
    /// one spacing of silence at the end so every strike is complete.
    fn repeated(sample_rate: u32, times: usize, spacing: usize) -> Audio {
        let mut signal = vec![0.0; spacing * (times + 1)];
        let hit = note(sample_rate, 220.0, spacing);
        for time in 0..times {
            for (index, value) in hit.iter().enumerate() {
                signal[time * spacing + index] += value;
            }
        }
        let mut audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();
        audio.quantize();
        audio
    }

    #[test]
    fn a_repeated_note_is_stored_once_and_placed_four_times() {
        let sample_rate = 22_050;
        let audio = repeated(sample_rate, 4, 5512);
        let options = DecomposeOptions::default().without_analysis();

        let decomposition = decompose(&audio, &options).unwrap();

        assert_eq!(
            decomposition.samples.len(),
            1,
            "the four identical hits share one waveform"
        );
        assert_eq!(decomposition.placements.len(), 4);
        let (stored, covered) = decomposition.bank_frames();
        assert_eq!(covered, stored * 4, "deduplication saved three copies");
        assert_eq!(
            decomposition.residual.peak(),
            0.0,
            "an exact repeat leaves nothing behind"
        );
    }

    #[test]
    fn the_reconstruction_is_exact_for_integer_audio() {
        let sample_rate = 22_050;
        let audio = repeated(sample_rate, 3, 4096);

        let decomposition = decompose(&audio, &DecomposeOptions::default()).unwrap();

        assert_eq!(decomposition.reconstruct(), audio);
        assert_eq!(decomposition.deviation_from(&audio).unwrap(), 0.0);
        assert!(decomposition.correction.is_none());
    }

    #[test]
    fn the_stems_sum_back_to_the_recording() {
        let sample_rate = 22_050;
        let audio = repeated(sample_rate, 2, 4096);

        let decomposition = decompose(&audio, &DecomposeOptions::default()).unwrap();

        let names: Vec<&str> = decomposition
            .stems
            .iter()
            .map(|stem| stem.name.as_str())
            .collect();
        assert_eq!(names, vec!["harmonic", "percussive", "correction"]);
        assert_eq!(decomposition.stem_sum(), audio);
    }

    #[test]
    fn boundaries_move_to_the_positions_the_matcher_chose() {
        let found = vec![
            Event::new(0, 0, 100),
            Event::new(0, 100, 100),
            Event::new(0, 200, 100),
        ];
        let starts = vec![Some(0), Some(96), None];

        let moved = realign(&found, &starts);

        assert_eq!(
            moved,
            vec![
                Event::new(0, 0, 96),
                Event::new(0, 96, 104),
                Event::new(0, 200, 100),
            ],
            "the events still tile the timeline"
        );
    }

    #[test]
    fn boundary_moves_that_would_reorder_the_timeline_are_refused() {
        let found = vec![Event::new(0, 0, 100), Event::new(0, 100, 100)];

        assert_eq!(
            realign(&found, &[Some(-5), Some(400)]),
            found,
            "a position outside its own event keeps the boundary"
        );
        assert_eq!(
            realign(&found, &[Some(0), Some(0)]),
            found,
            "a position on top of the boundary before it keeps the boundary"
        );
        assert_eq!(realign(&[], &[]), Vec::new());
    }

    #[test]
    fn each_channel_is_realigned_on_its_own() {
        let found = vec![
            Event::new(0, 0, 100),
            Event::new(1, 0, 100),
            Event::new(0, 100, 100),
            Event::new(1, 100, 100),
        ];
        let starts = vec![Some(0), Some(0), Some(90), None];

        let moved = realign(&found, &starts);

        assert_eq!(moved[0], Event::new(0, 0, 90));
        assert_eq!(moved[2], Event::new(0, 90, 110));
        assert_eq!(moved[1], Event::new(1, 0, 100), "channel 1 is untouched");
        assert_eq!(moved[3], Event::new(1, 100, 100));
    }

    #[test]
    fn derived_buffers_stay_on_the_grid_the_recording_came_from() {
        // Everything decompose derives is a difference of 16-bit values, so it
        // is 16-bit data. Tagging it `f64` would triple the size of the archive
        // and of every project exported from it without adding a single bit.
        let audio = repeated(22_050, 4, 2048);
        let decomposition = decompose(&audio, &DecomposeOptions::default()).unwrap();

        assert_eq!(decomposition.residual.format(), SampleFormat::PcmI16);
        for stem in &decomposition.stems {
            assert_eq!(stem.audio.format(), SampleFormat::PcmI16, "{}", stem.name);
        }
        if let Some(correction) = &decomposition.correction {
            assert_eq!(correction.format(), SampleFormat::PcmI16);
        }

        // Narrower tags must never cost fidelity.
        assert_eq!(decomposition.reconstruct().channels(), audio.channels());
    }

    #[test]
    fn the_silence_floor_follows_the_source_grid() {
        let options = DecomposeOptions::default();
        let integer = Audio::silence(22_050, SampleFormat::PcmI16, 1, 8);
        let float = Audio::silence(22_050, SampleFormat::F64, 1, 8);

        assert_eq!(silence_floor(&integer, &options), 1.0 / 65_536.0);
        assert_eq!(
            silence_floor(&float, &options),
            options.silence,
            "a float source has no grid to fall back on"
        );
    }

    #[test]
    fn identical_channels_share_their_bank_samples() {
        let sample_rate = 22_050;
        let mono = repeated(sample_rate, 2, 4096);
        let channel = mono.channel(0).to_vec();
        let audio = Audio::from_channels(
            sample_rate,
            SampleFormat::PcmI16,
            vec![channel.clone(), channel],
        )
        .unwrap();

        let decomposition =
            decompose(&audio, &DecomposeOptions::default().without_analysis()).unwrap();

        assert_eq!(decomposition.samples.len(), 1);
        assert_eq!(decomposition.placements.len(), 4);
        assert!(decomposition
            .placements
            .iter()
            .any(|placement| placement.channel == 1));
        assert_eq!(decomposition.reconstruct(), audio);
    }

    #[test]
    fn a_quieter_repeat_is_placed_with_a_gain() {
        let sample_rate = 22_050;
        let spacing = 4096;
        let hit = note(sample_rate, 220.0, spacing);
        let mut signal = vec![0.0; spacing * 2];
        for (index, value) in hit.iter().enumerate() {
            signal[index] = *value;
            signal[spacing + index] = value * 0.5;
        }
        let mut audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();
        audio.quantize();

        let decomposition =
            decompose(&audio, &DecomposeOptions::default().without_analysis()).unwrap();

        assert_eq!(decomposition.samples.len(), 1, "one waveform, two gains");
        let gains: Vec<f64> = decomposition
            .placements
            .iter()
            .map(|placement| placement.gain.factor())
            .collect();
        assert_eq!(gains[0], 1.0);
        assert!((gains[1] - 0.5).abs() < 0.01, "{gains:?}");
        assert_eq!(decomposition.reconstruct(), audio);
    }

    #[test]
    fn exact_mode_refuses_approximate_repeats() {
        let sample_rate = 22_050;
        let spacing = 4096;
        let hit = note(sample_rate, 220.0, spacing);
        let mut signal = vec![0.0; spacing * 2];
        for (index, value) in hit.iter().enumerate() {
            signal[index] = *value;
            signal[spacing + index] = value * 0.5;
        }
        let mut audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();
        audio.quantize();

        let options = DecomposeOptions::default()
            .without_analysis()
            .exact_repeats_only();
        let decomposition = decompose(&audio, &options).unwrap();

        assert_eq!(decomposition.samples.len(), 2, "the quiet copy is its own");
        assert_eq!(decomposition.reconstruct(), audio);
    }

    #[test]
    fn notes_and_sample_names_carry_the_recognised_pitch() {
        let sample_rate = 22_050;
        let audio = repeated(sample_rate, 2, 8192);

        let decomposition = decompose(&audio, &DecomposeOptions::default()).unwrap();

        assert!(!decomposition.notes.is_empty());
        assert!(decomposition
            .samples
            .iter()
            .any(|sample| sample.note.is_some()));
        assert!(decomposition
            .samples
            .iter()
            .any(|sample| sample.name.contains('A')));
    }

    #[test]
    fn silence_and_empty_recordings_decompose_to_nothing() {
        let silence = Audio::silence(22_050, SampleFormat::PcmI16, 1, 8192);
        let decomposition = decompose(&silence, &DecomposeOptions::default()).unwrap();

        assert!(decomposition.samples.is_empty());
        assert!(decomposition.placements.is_empty());
        assert_eq!(decomposition.reconstruct(), silence);

        let empty = Audio::silence(22_050, SampleFormat::PcmI16, 1, 0);
        let decomposition = decompose(&empty, &DecomposeOptions::default()).unwrap();
        assert_eq!(decomposition.reconstruct(), empty);
        assert_eq!(decomposition.stem_sum(), empty);
    }

    #[test]
    fn a_configured_decomposer_behaves_like_the_function() {
        let audio = repeated(22_050, 2, 4096);
        let options = DecomposeOptions::default().without_analysis();
        let decomposer = Decomposer::with_options(options.clone());

        assert_eq!(decomposer.options().candidates, options.candidates);
        assert_eq!(
            decomposer.decompose(&audio).unwrap(),
            decompose(&audio, &options).unwrap()
        );
        assert_eq!(Decomposer::new().options().name, "recording");
    }
}
