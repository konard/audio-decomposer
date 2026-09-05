//! The neutral session every project exporter is written against.
//!
//! A decomposition is a statement about audio: these stems sum to the
//! recording, this bank of samples placed at these offsets reproduces it, these
//! notes were recognised. A DAW project is a statement about a session: tracks
//! holding clips at points in time. [`Session`] is the translation between the
//! two, done once, so that MusicXML, DAWproject, Ableton Live, FL Studio,
//! REAPER, Ardour, LMMS and SFZ each only have to write down a session in their
//! own words.
//!
//! The file names a session points at are the ones [`crate::archive`] writes,
//! so a project exported next to an archive opens with its audio in place.

use crate::archive;
use crate::associative::schema::Manifest;
use crate::decompose::model::{Decomposition, Note};
use crate::dsp::tempo::{self, TempoOptions};

/// Beats per minute used when nothing better is known.
pub const DEFAULT_TEMPO: f64 = 120.0;

/// How many placed events the tempo estimator needs before its answer is worth
/// more than the default. Three chords do not make a tempo.
pub const MINIMUM_TEMPO_EVENTS: usize = 8;

/// What to put in the session.
#[derive(Clone, Copy, Debug, PartialEq)]
// Four independent switches, one per kind of track: an enum here would only
// make callers spell out combinations that are already obvious.
#[allow(clippy::struct_excessive_bools)]
pub struct SessionOptions {
    /// Beats per minute the timeline is laid out with.
    pub tempo: f64,
    /// Include one track per stem.
    pub stems: bool,
    /// Include one track per bank sample, with a clip at every placement.
    pub bank: bool,
    /// Include the residual as a track of its own.
    pub residual: bool,
    /// Include the recognised notes as an instrument track.
    pub notes: bool,
    /// Measure the tempo from where the events actually fall, and use `tempo`
    /// only when the measurement is too weak to trust.
    pub estimate_tempo: bool,
    /// How sure the estimator has to be, in `0.0..=1.0`, before its answer is
    /// preferred over `tempo`.
    pub tempo_confidence: f64,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            tempo: DEFAULT_TEMPO,
            stems: true,
            bank: true,
            residual: true,
            notes: true,
            estimate_tempo: true,
            tempo_confidence: 0.5,
        }
    }
}

impl SessionOptions {
    /// The same options at a different tempo, which is then taken as given.
    #[must_use]
    pub const fn at_tempo(mut self, tempo: f64) -> Self {
        self.tempo = tempo;
        self.estimate_tempo = false;
        self
    }

    /// The same options with tempo measurement switched on or off.
    #[must_use]
    pub const fn estimating_tempo(mut self, estimate: bool) -> Self {
        self.estimate_tempo = estimate;
        self
    }

    /// Only the sample bank and the notes, which is the arrangement a producer
    /// wants to edit rather than the tape it came from.
    #[must_use]
    pub const fn arrangement_only(mut self) -> Self {
        self.stems = false;
        self.residual = false;
        self
    }
}

/// What a track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// Audio clips on a timeline.
    Audio,
    /// Notes played by the sample bank.
    Instrument,
}

/// One placed piece of audio.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// Name to show on the clip.
    pub name: String,
    /// File the audio comes from, relative to the project directory.
    pub file: String,
    /// Where the clip starts on the timeline, in frames.
    pub start: usize,
    /// How long the clip is, in frames.
    pub length: usize,
    /// How far into the file the clip starts, in frames.
    pub offset: usize,
    /// Level the clip is played at.
    pub gain: f64,
    /// Channel of the recording the clip belongs to.
    pub channel: usize,
}

/// One track.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// Track name.
    pub name: String,
    /// Audio or instrument.
    pub kind: TrackKind,
    /// Clips on the timeline.
    pub clips: Vec<Clip>,
    /// Notes on the timeline.
    pub notes: Vec<Note>,
}

impl Track {
    /// Where the last thing on this track ends, in frames.
    #[must_use]
    pub fn end(&self) -> usize {
        let clips = self.clips.iter().map(|clip| clip.start + clip.length);
        let notes = self.notes.iter().map(Note::end);
        clips.chain(notes).max().unwrap_or(0)
    }
}

/// One entry of the sample bank, as an instrument sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Instrument {
    /// Bank index.
    pub id: usize,
    /// Sample name.
    pub name: String,
    /// File the waveform is in, relative to the project directory.
    pub file: String,
    /// Note the sample was recognised as, when it was recognised at all.
    pub note: Option<u8>,
    /// Frames in the waveform.
    pub frames: usize,
    /// Loudest sample in the waveform.
    pub peak: f64,
}

/// A decomposition seen as a DAW session.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// Session name.
    pub name: String,
    /// Sample rate of the audio.
    pub sample_rate: u32,
    /// Beats per minute.
    pub tempo: f64,
    /// Length of the recording in frames.
    pub frames: usize,
    /// Channels the recording had.
    pub channels: usize,
    /// Tracks, in the order they should appear.
    pub tracks: Vec<Track>,
    /// The sample bank, playable as one instrument.
    pub instruments: Vec<Instrument>,
}

impl Session {
    /// Views a decomposition as a session.
    #[must_use]
    pub fn of(decomposition: &Decomposition, options: &SessionOptions) -> Self {
        Self::from_manifest(&archive::manifest_of(decomposition), options)
    }

    /// Views an already-named decomposition as a session.
    ///
    /// Taking the manifest rather than the decomposition means the session
    /// points at exactly the files an archive of it holds, even when two stems
    /// wanted the same name.
    #[must_use]
    pub fn from_manifest(manifest: &Manifest, options: &SessionOptions) -> Self {
        let source = &manifest.source;
        let mut tracks = Vec::new();

        if options.stems {
            for stem in &manifest.stems {
                tracks.push(Track {
                    name: stem.name.clone(),
                    kind: TrackKind::Audio,
                    clips: vec![Clip {
                        name: stem.name.clone(),
                        file: stem.file.clone(),
                        start: 0,
                        length: source.frames,
                        offset: 0,
                        gain: 1.0,
                        channel: 0,
                    }],
                    notes: Vec::new(),
                });
            }
        }

        if options.bank {
            for sample in &manifest.samples {
                let mut by_channel: Vec<Vec<Clip>> = vec![Vec::new(); source.channels.max(1)];
                for placement in manifest
                    .placements
                    .iter()
                    .filter(|placement| placement.sample == sample.id)
                {
                    let Some(clip) = clip_of(&sample.file, &sample.name, sample.frames, placement)
                    else {
                        continue;
                    };
                    if let Some(lane) = by_channel.get_mut(placement.channel) {
                        lane.push(clip);
                    }
                }
                for (channel, clips) in by_channel.into_iter().enumerate() {
                    if clips.is_empty() {
                        continue;
                    }
                    tracks.push(Track {
                        name: track_name(&sample.name, channel, source.channels),
                        kind: TrackKind::Audio,
                        clips,
                        notes: Vec::new(),
                    });
                }
            }
        }

        if options.residual {
            tracks.push(Track {
                name: "residual".to_string(),
                kind: TrackKind::Audio,
                clips: vec![Clip {
                    name: "residual".to_string(),
                    file: manifest.residual.clone(),
                    start: 0,
                    length: source.frames,
                    offset: 0,
                    gain: 1.0,
                    channel: 0,
                }],
                notes: Vec::new(),
            });
        }

        if options.notes && !manifest.notes.is_empty() {
            tracks.push(Track {
                name: "notes".to_string(),
                kind: TrackKind::Instrument,
                clips: Vec::new(),
                notes: manifest.notes.clone(),
            });
        }

        Self {
            name: source.name.clone(),
            sample_rate: source.sample_rate,
            tempo: tempo_of(manifest, options),
            frames: source.frames,
            channels: source.channels,
            tracks,
            instruments: manifest
                .samples
                .iter()
                .map(|sample| Instrument {
                    id: sample.id,
                    name: sample.name.clone(),
                    file: sample.file.clone(),
                    note: sample.note,
                    frames: sample.frames,
                    peak: sample.peak,
                })
                .collect(),
        }
    }

    /// A point in time in seconds.
    #[must_use]
    pub fn seconds(&self, frames: usize) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        frames as f64 / f64::from(self.sample_rate)
    }

    /// A point in time in beats.
    #[must_use]
    pub fn beats(&self, frames: usize) -> f64 {
        self.seconds(frames) * self.tempo / 60.0
    }

    /// How long the session is, counting anything placed past the recording.
    #[must_use]
    pub fn end(&self) -> usize {
        self.tracks
            .iter()
            .map(Track::end)
            .chain(std::iter::once(self.frames))
            .max()
            .unwrap_or(0)
    }

    /// Every file the session refers to, without repeats, in a stable order.
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let clips = self.tracks.iter().flat_map(|track| &track.clips);
        for file in clips
            .map(|clip| &clip.file)
            .chain(self.instruments.iter().map(|instrument| &instrument.file))
        {
            if !files.contains(file) {
                files.push(file.clone());
            }
        }
        files
    }

    /// Tracks holding audio.
    pub fn audio_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
    }

    /// Tracks holding notes.
    pub fn instrument_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Instrument)
    }

    /// Every note in the session, whichever track it sits on.
    pub fn notes(&self) -> impl Iterator<Item = &Note> {
        self.tracks.iter().flat_map(|track| &track.notes)
    }
}

/// The tempo the session is laid out with: measured from where the events fall
/// when there are enough of them and the measurement is convincing, and the
/// tempo the caller asked for otherwise.
///
/// The events are the sample placements and the recognised notes, which is
/// exactly the onset list a tempo estimator wants: the decomposition already
/// found where every attack is, so the tempo does not have to be looked for in
/// the waveform a second time.
fn tempo_of(manifest: &Manifest, options: &SessionOptions) -> f64 {
    if !options.estimate_tempo || manifest.source.sample_rate == 0 {
        return options.tempo;
    }
    let rate = f64::from(manifest.source.sample_rate);
    let placements = manifest
        .placements
        .iter()
        .filter(|placement| placement.start >= 0)
        .map(|placement| placement.start as f64 / rate);
    let notes = manifest.notes.iter().map(|note| note.start as f64 / rate);
    let mut times: Vec<f64> = placements.chain(notes).collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|left, right| (*left - *right).abs() < f64::EPSILON);
    if times.len() < MINIMUM_TEMPO_EVENTS {
        return options.tempo;
    }
    tempo::from_onsets(&times, TempoOptions::default())
        .filter(|measured| measured.confidence >= options.tempo_confidence)
        .map_or(options.tempo, |measured| measured.bpm)
}

/// Turns a placement into a clip, trimming the part that would start before the
/// timeline does.
fn clip_of(
    file: &str,
    name: &str,
    frames: usize,
    placement: &crate::decompose::model::Placement,
) -> Option<Clip> {
    let (start, offset) = if placement.start < 0 {
        (0, usize::try_from(-placement.start).ok()?)
    } else {
        (usize::try_from(placement.start).ok()?, 0)
    };
    let length = frames.checked_sub(offset)?;
    if length == 0 {
        return None;
    }
    Some(Clip {
        name: name.to_string(),
        file: file.to_string(),
        start,
        length,
        offset,
        gain: placement.gain.factor(),
        channel: placement.channel,
    })
}

/// Names the track a sample plays on, mentioning the channel only when there is
/// more than one.
fn track_name(sample: &str, channel: usize, channels: usize) -> String {
    match (channels, channel) {
        (0 | 1, _) => sample.to_string(),
        (2, 0) => format!("{sample} L"),
        (2, 1) => format!("{sample} R"),
        _ => format!("{sample} {}", channel + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{Audio, SampleFormat};
    use crate::decompose::model::{Gain, Placement, Sample, SourceInfo, Stem};

    fn audio(channels: Vec<Vec<f64>>) -> Audio {
        Audio::from_channels(8_000, SampleFormat::PcmI16, channels).unwrap()
    }

    fn decomposition() -> Decomposition {
        let source = audio(vec![vec![0.5, -0.5, 0.0, 0.0], vec![0.0, 0.0, 0.25, 0.0]]);
        let mut samples = vec![
            Sample::new(0, "kick".to_string(), vec![0.5, -0.5]),
            Sample::new(1, "hat".to_string(), vec![0.25]),
        ];
        samples[0].note = Some(36);
        Decomposition {
            source: SourceInfo::of("song", &source),
            stems: vec![Stem::new("harmonic".to_string(), source.clone())],
            samples,
            placements: vec![
                Placement::new(0, 0, 0, Gain::UNIT),
                Placement::new(0, 1, 2, Gain::from_factor(0.5)),
                Placement::new(1, 1, 2, Gain::UNIT),
            ],
            notes: vec![Note {
                channel: 0,
                start: 0,
                length: 2,
                note: 36,
                velocity: 100,
                frequency: 65.406_391_325_149_66,
                confidence: 1.0,
            }],
            residual: audio(vec![vec![0.0; 4], vec![0.0; 4]]),
            correction: None,
        }
    }

    /// A manifest whose placements land on a steady grid, which is what the
    /// tempo estimator is supposed to hear.
    fn steady(interval: f64, count: usize) -> Manifest {
        let rate = 8_000;
        let source = audio(vec![vec![0.0; rate as usize * 12]]);
        let mut manifest = archive::manifest_of(&Decomposition {
            source: SourceInfo::of("steady", &source),
            stems: Vec::new(),
            samples: vec![Sample::new(0, "kick".to_string(), vec![0.5, -0.5])],
            placements: Vec::new(),
            notes: Vec::new(),
            residual: source.clone(),
            correction: None,
        });
        manifest.placements = (0..count)
            .map(|index| {
                let start = (index as f64 * interval * f64::from(rate)).round() as i64;
                Placement::new(0, 0, start, Gain::UNIT)
            })
            .collect();
        manifest
    }

    #[test]
    fn the_tempo_is_measured_from_where_the_events_fall() {
        for (interval, bpm) in [(0.5, 120.0), (0.4, 150.0), (0.75, 80.0)] {
            let manifest = steady(interval, 20);
            let session = Session::from_manifest(&manifest, &SessionOptions::default());
            assert!(
                (session.tempo - bpm).abs() < 2.0,
                "expected {bpm} BPM from a hit every {interval} s, found {}",
                session.tempo
            );
        }
    }

    #[test]
    fn a_handful_of_events_is_not_a_tempo() {
        let session = Session::from_manifest(&steady(0.5, 4), &SessionOptions::default());
        assert!((session.tempo - DEFAULT_TEMPO).abs() < 1e-12);
    }

    #[test]
    fn a_tempo_that_was_asked_for_is_not_second_guessed() {
        let manifest = steady(0.4, 20);
        let session = Session::from_manifest(&manifest, &SessionOptions::default().at_tempo(90.0));
        assert!((session.tempo - 90.0).abs() < 1e-12);

        let measured = SessionOptions {
            tempo: 90.0,
            estimate_tempo: true,
            ..SessionOptions::default()
        };
        let session = Session::from_manifest(&manifest, &measured);
        assert!((session.tempo - 150.0).abs() < 2.0, "{}", session.tempo);
    }

    #[test]
    fn a_decomposition_becomes_tracks_and_instruments() {
        let session = Session::of(&decomposition(), &SessionOptions::default());

        let names: Vec<_> = session.tracks.iter().map(|track| &track.name).collect();
        assert_eq!(
            names,
            ["harmonic", "kick L", "kick R", "hat R", "residual", "notes"]
        );
        assert_eq!(session.instruments.len(), 2);
        assert_eq!(session.instruments[0].note, Some(36));
        assert_eq!(session.instruments[0].file, "samples/kick.wav");
        assert_eq!(session.notes().count(), 1);
        assert_eq!(session.audio_tracks().count(), 5);
        assert_eq!(session.instrument_tracks().count(), 1);
    }

    #[test]
    fn clips_carry_their_place_length_and_level() {
        let session = Session::of(&decomposition(), &SessionOptions::default());
        let right = session
            .tracks
            .iter()
            .find(|track| track.name == "kick R")
            .unwrap();

        assert_eq!(right.clips.len(), 1);
        assert_eq!(right.clips[0].start, 2);
        assert_eq!(right.clips[0].length, 2);
        assert_eq!(right.clips[0].offset, 0);
        assert!((right.clips[0].gain - 0.5).abs() < 1e-9);
        assert_eq!(right.end(), 4);
    }

    #[test]
    fn a_placement_that_starts_before_the_timeline_is_trimmed() {
        let mut decomposition = decomposition();
        decomposition.placements = vec![Placement::new(0, 0, -1, Gain::UNIT)];

        let session = Session::of(&decomposition, &SessionOptions::default());
        let clip = &session
            .tracks
            .iter()
            .find(|track| track.name == "kick L")
            .unwrap()
            .clips[0];

        assert_eq!((clip.start, clip.offset, clip.length), (0, 1, 1));
    }

    #[test]
    fn a_placement_hidden_entirely_before_the_timeline_is_dropped() {
        let mut decomposition = decomposition();
        decomposition.placements = vec![Placement::new(1, 0, -5, Gain::UNIT)];

        let session = Session::of(&decomposition, &SessionOptions::default());

        assert!(session.tracks.iter().all(|track| track.name != "hat"));
    }

    #[test]
    fn the_arrangement_can_be_asked_for_on_its_own() {
        let options = SessionOptions::default().arrangement_only().at_tempo(90.0);
        let session = Session::of(&decomposition(), &options);

        let names: Vec<_> = session.tracks.iter().map(|track| &track.name).collect();
        assert_eq!(names, ["kick L", "kick R", "hat R", "notes"]);
        assert!((session.tempo - 90.0).abs() < 1e-12);
    }

    #[test]
    fn time_is_reported_in_seconds_and_beats() {
        let mut session = Session::of(&decomposition(), &SessionOptions::default());

        assert!((session.seconds(8_000) - 1.0).abs() < 1e-12);
        assert!((session.beats(8_000) - 2.0).abs() < 1e-12);
        assert_eq!(session.end(), 4);

        session.sample_rate = 0;
        assert_eq!(session.seconds(8_000), 0.0);
        assert_eq!(session.beats(8_000), 0.0);
    }

    #[test]
    fn every_file_is_listed_once() {
        let session = Session::of(&decomposition(), &SessionOptions::default());

        assert_eq!(
            session.files(),
            [
                "stems/harmonic.wav",
                "samples/kick.wav",
                "samples/hat.wav",
                "residual.wav",
            ]
        );
    }

    #[test]
    fn a_mono_recording_does_not_get_channel_suffixes() {
        assert_eq!(track_name("kick", 0, 1), "kick");
        assert_eq!(track_name("kick", 0, 2), "kick L");
        assert_eq!(track_name("kick", 1, 2), "kick R");
        assert_eq!(track_name("kick", 2, 4), "kick 3");
    }
}
