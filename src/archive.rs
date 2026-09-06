//! Writing a decomposition to a directory, and reading it back unchanged.
//!
//! An archive is a plain directory. Nothing is packed, compressed or encoded in
//! a way a human cannot inspect, because the point of taking a recording apart
//! is being able to look at the parts:
//!
//! ```text
//! song.decomposition/
//!   decomposition.lino     the manifest, in Links Notation
//!   stems/harmonic.wav     one file per stem, summing to the recording
//!   stems/percussive.wav
//!   samples/kick-01.wav    one file per bank sample, mono
//!   residual.wav           what the bank could not explain
//!   correction.wav         present only when a float source needed one
//! ```
//!
//! Reading an archive back yields the same [`Decomposition`] the writer was
//! given, sample for sample, so the round trip
//! `write_dir` → `read_dir` → [`Decomposition::reconstruct`] returns the
//! original recording. Buffers are written in their own sample format when that
//! format can hold them exactly, and otherwise in the narrowest format that
//! can, so nothing is ever rounded on the way to disk and nothing pays for
//! precision it does not carry.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::associative::schema::{Entry, Manifest, SampleEntry};
use crate::audio::{wav, Audio};
use crate::decompose::model::{Decomposition, Sample, Stem};
use crate::error::{Error, Result};

/// Name of the manifest inside an archive directory.
pub const MANIFEST_FILE: &str = "decomposition.lino";

/// The manifest an archive of `decomposition` would carry.
///
/// This is [`Manifest::of`] with one extra guarantee: no two stems and no two
/// samples share a file, even when their names differ only in characters that
/// a file name cannot hold.
#[must_use]
pub fn manifest_of(decomposition: &Decomposition) -> Manifest {
    let mut manifest = Manifest::of(decomposition);
    let mut taken = HashSet::new();
    for stem in &mut manifest.stems {
        stem.file = unique(&mut taken, &stem.file);
    }
    for sample in &mut manifest.samples {
        sample.file = unique(&mut taken, &sample.file);
    }
    manifest
}

/// Writes a decomposition into `path`, creating the directory if needed.
///
/// Files belonging to the decomposition are overwritten; anything else already
/// in the directory is left alone, so writing a smaller decomposition over a
/// larger one leaves the older audio files behind.
pub fn write_dir(path: impl AsRef<Path>, decomposition: &Decomposition) -> Result<Manifest> {
    let root = path.as_ref();
    let manifest = manifest_of(decomposition);
    fs::create_dir_all(root)?;

    for (entry, stem) in manifest.stems.iter().zip(&decomposition.stems) {
        write_audio(&resolve(root, &entry.file)?, &stem.audio)?;
    }
    for (entry, sample) in manifest.samples.iter().zip(&decomposition.samples) {
        // A bank sample is a cutting of the recording, so it belongs on the
        // recording's grid. `write_audio` widens it again if normalising the
        // sample pushed it past what that format can count to.
        let waveform = Audio::from_mono(
            decomposition.source.sample_rate,
            decomposition.source.format,
            sample.waveform.clone(),
        )?;
        write_audio(&resolve(root, &entry.file)?, &waveform)?;
    }
    write_audio(&resolve(root, &manifest.residual)?, &decomposition.residual)?;
    if let (Some(file), Some(correction)) = (&manifest.correction, &decomposition.correction) {
        write_audio(&resolve(root, file)?, correction)?;
    }

    fs::write(root.join(MANIFEST_FILE), manifest.to_lino())?;
    Ok(manifest)
}

/// Reads the manifest of an archive without touching its audio.
pub fn read_manifest(path: impl AsRef<Path>) -> Result<Manifest> {
    let file = path.as_ref().join(MANIFEST_FILE);
    let text = fs::read_to_string(&file).map_err(|error| {
        Error::Io(std::io::Error::new(
            error.kind(),
            format!("{}: {error}", file.display()),
        ))
    })?;
    Manifest::from_lino(&text)
}

/// Reads a decomposition back out of an archive directory.
pub fn read_dir(path: impl AsRef<Path>) -> Result<Decomposition> {
    let root = path.as_ref();
    let manifest = read_manifest(root)?;
    from_manifest(root, &manifest)
}

/// Reads the audio an already-parsed manifest points at.
pub fn from_manifest(path: impl AsRef<Path>, manifest: &Manifest) -> Result<Decomposition> {
    let root = path.as_ref();
    let stems = manifest
        .stems
        .iter()
        .map(|entry| read_stem(root, entry))
        .collect::<Result<Vec<_>>>()?;
    let samples = manifest
        .samples
        .iter()
        .enumerate()
        .map(|(index, entry)| read_sample(root, index, entry))
        .collect::<Result<Vec<_>>>()?;
    let residual = wav::read_file(resolve(root, &manifest.residual)?)?;
    let correction = manifest
        .correction
        .as_ref()
        .map(|file| resolve(root, file).and_then(wav::read_file))
        .transpose()?;

    let decomposition = Decomposition {
        source: manifest.source.clone(),
        stems,
        samples,
        placements: manifest.placements.clone(),
        notes: manifest.notes.clone(),
        residual,
        correction,
    };
    check(&decomposition)?;
    Ok(decomposition)
}

/// Reads one stem.
fn read_stem(root: &Path, entry: &Entry) -> Result<Stem> {
    Ok(Stem::new(
        entry.name.clone(),
        wav::read_file(resolve(root, &entry.file)?)?,
    ))
}

/// Reads one bank sample, checking it against what the manifest promised.
fn read_sample(root: &Path, index: usize, entry: &SampleEntry) -> Result<Sample> {
    if entry.id != index {
        return Err(Error::Format(format!(
            "sample `{}` has id {} but is the {index}. entry in the bank",
            entry.name, entry.id
        )));
    }
    let audio = wav::read_file(resolve(root, &entry.file)?)?;
    if audio.channel_count() != 1 {
        return Err(Error::Mismatch(format!(
            "sample `{}` has {} channels, bank samples are mono",
            entry.name,
            audio.channel_count()
        )));
    }
    let mut sample = Sample::new(entry.id, entry.name.clone(), audio.channel(0).to_vec());
    if sample.len() != entry.frames {
        return Err(Error::Mismatch(format!(
            "sample `{}` holds {} frames, the manifest says {}",
            entry.name,
            sample.len(),
            entry.frames
        )));
    }
    if sample.peak != entry.peak {
        return Err(Error::Mismatch(format!(
            "sample `{}` peaks at {}, the manifest says {}",
            entry.name, sample.peak, entry.peak
        )));
    }
    sample.note = entry.note;
    sample.frequency = entry.frequency;
    Ok(sample)
}

/// Rejects an archive whose parts do not fit together.
fn check(decomposition: &Decomposition) -> Result<()> {
    let source = &decomposition.source;
    let mut buffers = decomposition
        .stems
        .iter()
        .map(|stem| (stem.name.as_str(), &stem.audio))
        .chain([("residual", &decomposition.residual)])
        .chain(
            decomposition
                .correction
                .iter()
                .map(|correction| ("correction", correction)),
        );
    if let Some((name, audio)) = buffers.find(|(_, audio)| {
        audio.sample_rate() != source.sample_rate
            || audio.channel_count() != source.channels
            || audio.frames() != source.frames
    }) {
        return Err(Error::Mismatch(format!(
            "`{name}` is {} Hz, {} channels, {} frames; the source is {} Hz, {} channels, {} frames",
            audio.sample_rate(),
            audio.channel_count(),
            audio.frames(),
            source.sample_rate,
            source.channels,
            source.frames
        )));
    }
    if let Some(placement) = decomposition
        .placements
        .iter()
        .find(|placement| placement.sample >= decomposition.samples.len())
    {
        return Err(Error::Format(format!(
            "a placement refers to sample {}, the bank holds {}",
            placement.sample,
            decomposition.samples.len()
        )));
    }
    Ok(())
}

/// Writes one buffer without rounding it.
fn write_audio(path: &Path, audio: &Audio) -> Result<()> {
    if audio.is_exact_in_format() {
        return wav::write_file(path, audio);
    }
    // Off its own grid: store it in the narrowest format that is still exact
    // rather than reaching straight for `f64`, which would quadruple the
    // bytes without carrying a single extra bit of the recording.
    let mut exact = audio.clone();
    let floor = exact.format();
    exact.set_format(exact.narrowest_exact_format(floor));
    wav::write_file(path, &exact)
}

/// Joins a manifest file name onto the archive root, refusing anything that
/// would step outside it.
fn resolve(root: &Path, file: &str) -> Result<PathBuf> {
    let relative = Path::new(file);
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(Error::Format(format!(
            "`{file}` is not a name inside the archive"
        )));
    }
    Ok(root.join(relative))
}

/// A file name that no other part of the archive has claimed yet.
fn unique(taken: &mut HashSet<String>, file: &str) -> String {
    if taken.insert(file.to_string()) {
        return file.to_string();
    }
    let (stem, extension) = match file.rsplit_once('.') {
        Some((stem, extension)) => (stem, format!(".{extension}")),
        None => (file, String::new()),
    };
    let mut counter: usize = 2;
    loop {
        let candidate = format!("{stem}-{counter}{extension}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::associative::schema::{CORRECTION_FILE, RESIDUAL_FILE};
    use crate::audio::SampleFormat;
    use crate::decompose::model::{Gain, Note, Placement, SourceInfo};

    /// A temporary directory that removes itself, so tests leave no litter.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "audio-decomposer-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn audio(values: &[f64]) -> Audio {
        Audio::from_mono(8_000, SampleFormat::PcmI16, values.to_vec()).unwrap()
    }

    fn decomposition() -> Decomposition {
        let source = SourceInfo::of("a song", &audio(&[0.0; 8]));
        let mut samples = vec![
            Sample::new(0, "kick".to_string(), vec![0.5, -0.5, 0.25]),
            Sample::new(1, "hat".to_string(), vec![0.125, 0.0625]),
        ];
        samples[0].note = Some(36);
        samples[0].frequency = Some(65.406_391_325_149_66);
        Decomposition {
            source,
            stems: vec![
                Stem::new("harmonic".to_string(), audio(&[0.25; 8])),
                Stem::new("percussive".to_string(), audio(&[-0.25; 8])),
            ],
            samples,
            placements: vec![
                Placement::new(0, 0, 0, Gain::UNIT),
                Placement::new(1, 0, 4, Gain::from_numerator(32_768)),
            ],
            notes: vec![Note {
                channel: 0,
                start: 0,
                length: 3,
                note: 36,
                velocity: 100,
                frequency: 65.406_391_325_149_66,
                confidence: 0.75,
            }],
            residual: audio(&[0.0; 8]),
            correction: None,
        }
    }

    #[test]
    fn an_archive_round_trips_through_a_directory() {
        let scratch = Scratch::new("round-trip");
        let original = decomposition();

        let manifest = write_dir(scratch.path(), &original).unwrap();
        assert_eq!(manifest.stems[0].file, "stems/harmonic.wav");
        assert_eq!(manifest.samples[1].file, "samples/hat.wav");
        assert!(scratch.path().join(MANIFEST_FILE).is_file());
        assert!(scratch.path().join(RESIDUAL_FILE).is_file());
        assert!(!scratch.path().join(CORRECTION_FILE).exists());

        let restored = read_dir(scratch.path()).unwrap();
        assert_eq!(restored.source, original.source);
        assert_eq!(restored.stems, original.stems);
        assert_eq!(restored.placements, original.placements);
        assert_eq!(restored.notes, original.notes);
        assert_eq!(restored.residual, original.residual);
        assert_eq!(restored.samples, original.samples);
        assert_eq!(restored.reconstruct(), original.reconstruct());
    }

    #[test]
    fn a_correction_is_stored_when_there_is_one() {
        let scratch = Scratch::new("correction");
        let mut original = decomposition();
        original.source.format = SampleFormat::F32;
        original.correction =
            Some(Audio::from_mono(8_000, SampleFormat::F64, vec![1.0e-9; 8]).unwrap());

        write_dir(scratch.path(), &original).unwrap();
        assert!(scratch.path().join(CORRECTION_FILE).is_file());

        let restored = read_dir(scratch.path()).unwrap();
        assert_eq!(restored.correction, original.correction);
        assert_eq!(restored, original);
    }

    #[test]
    fn buffers_that_their_format_cannot_hold_are_stored_as_double() {
        let scratch = Scratch::new("exactness");
        let mut original = decomposition();
        let precise = vec![0.1; 8];
        original.stems[0] = Stem::new(
            "harmonic".to_string(),
            Audio::from_mono(8_000, SampleFormat::F32, precise.clone()).unwrap(),
        );
        original.stems[1] = Stem::new(
            "percussive".to_string(),
            Audio::from_mono(8_000, SampleFormat::F32, vec![0.5; 8]).unwrap(),
        );

        write_dir(scratch.path(), &original).unwrap();
        let restored = read_dir(scratch.path()).unwrap();

        assert_eq!(restored.stems[0].audio.channel(0), precise.as_slice());
        assert_eq!(restored.stems[0].audio.format(), SampleFormat::F64);
        assert_eq!(restored.stems[1].audio.format(), SampleFormat::F32);
    }

    #[test]
    fn names_that_share_a_file_name_are_kept_apart() {
        let scratch = Scratch::new("collisions");
        let mut original = decomposition();
        original.stems[1].name = "harmonic!".to_string();
        original.samples[1].name = "kick".to_string();

        let manifest = write_dir(scratch.path(), &original).unwrap();
        assert_eq!(manifest.stems[0].file, "stems/harmonic.wav");
        assert_eq!(manifest.stems[1].file, "stems/harmonic_.wav");
        assert_eq!(manifest.samples[0].file, "samples/kick.wav");
        assert_eq!(manifest.samples[1].file, "samples/kick-2.wav");

        let restored = read_dir(scratch.path()).unwrap();
        assert_eq!(restored.samples[1].waveform, original.samples[1].waveform);
    }

    #[test]
    fn every_stem_and_sample_really_lands_in_its_own_file() {
        let mut taken = HashSet::new();
        assert_eq!(unique(&mut taken, "a/b.wav"), "a/b.wav");
        assert_eq!(unique(&mut taken, "a/b.wav"), "a/b-2.wav");
        assert_eq!(unique(&mut taken, "a/b.wav"), "a/b-3.wav");
        assert_eq!(unique(&mut taken, "a/b-2.wav"), "a/b-2-2.wav");
        assert_eq!(unique(&mut taken, "plain"), "plain");
        assert_eq!(unique(&mut taken, "plain"), "plain-2");
    }

    #[test]
    fn an_archive_that_points_outside_itself_is_refused() {
        let root = Path::new("/tmp/archive");
        assert!(resolve(root, "stems/a.wav").is_ok());
        assert!(resolve(root, "../escape.wav").is_err());
        assert!(resolve(root, "/etc/passwd").is_err());
        assert!(resolve(root, "./a.wav").is_err());
    }

    #[test]
    fn a_manifest_that_does_not_match_its_audio_is_refused() {
        let scratch = Scratch::new("mismatch");
        let original = decomposition();
        let manifest = write_dir(scratch.path(), &original).unwrap();

        let mut wrong = manifest.clone();
        wrong.samples[0].frames = 99;
        assert!(from_manifest(scratch.path(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("holds 3 frames"));

        let mut wrong = manifest.clone();
        wrong.samples[0].peak = 0.125;
        assert!(from_manifest(scratch.path(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("peaks at"));

        let mut wrong = manifest.clone();
        wrong.samples[0].id = 7;
        assert!(from_manifest(scratch.path(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("has id 7"));

        let mut wrong = manifest.clone();
        wrong.source.frames = 9;
        assert!(from_manifest(scratch.path(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("the source is"));

        let mut wrong = manifest;
        wrong.placements.push(Placement::new(9, 0, 0, Gain::UNIT));
        assert!(from_manifest(scratch.path(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("refers to sample 9"));
    }

    #[test]
    fn a_missing_archive_says_which_file_is_missing() {
        let scratch = Scratch::new("missing");
        let message = read_dir(scratch.path()).unwrap_err().to_string();

        assert!(message.contains(MANIFEST_FILE), "{message}");
    }
}
