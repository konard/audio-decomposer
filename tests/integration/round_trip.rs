//! The claim the project rests on: what comes apart goes back together.
//!
//! "Byte for byte" is the bar throughout. Integer PCM has an exact grid, and
//! every stage of the pipeline is written to stay on it, so anything less than
//! an identical file is a bug rather than a rounding difference.

use audio_decomposer::audio::{self, Container, SampleFormat};
use audio_decomposer::decompose::{decompose, DecomposeOptions, StemMode};
use audio_decomposer::formats::session::SessionOptions;
use audio_decomposer::recompose;
use audio_decomposer::{archive, formats::project};

use super::fixtures::{chord_progression, every, repetitive_loop, wide_stereo, Scratch};

/// Analysis settings small enough to run four fixtures per test without
/// turning the suite into something nobody waits for.
fn options(name: &str) -> DecomposeOptions {
    DecomposeOptions {
        name: name.to_string(),
        candidates: 8,
        ..DecomposeOptions::default()
    }
}

#[test]
fn every_fixture_reconstructs_exactly_in_memory() {
    for (name, source) in every(8_000, SampleFormat::PcmI16) {
        let decomposition = decompose(&source, &options(name)).expect(name);
        let report = recompose::verify(&decomposition, &source).expect(name);
        assert!(
            report.is_exact(),
            "{name} is off by {:.3e}",
            report.deviation
        );
        let rebuilt = recompose::reconstruct(&decomposition);
        assert_eq!(rebuilt.channels(), source.channels(), "{name}");
    }
}

#[test]
fn every_fixture_reconstructs_exactly_through_an_archive_on_disk() {
    let scratch = Scratch::new("archive-round-trip");
    for (name, source) in every(8_000, SampleFormat::PcmI16) {
        let decomposition = decompose(&source, &options(name)).expect(name);
        let root = scratch.join(name);
        archive::write_dir(&root, &decomposition).expect(name);

        let report = recompose::verify_archive(&root, &source).expect(name);
        assert!(report.is_exact(), "{name} does not survive the disk");

        // The archive must also read back as the same decomposition, not just
        // as the same audio: the sample bank is the deliverable, not a cache.
        let reread = archive::read_dir(&root).expect(name);
        assert_eq!(reread.samples.len(), decomposition.samples.len(), "{name}");
        assert_eq!(
            reread.placements.len(),
            decomposition.placements.len(),
            "{name}"
        );
        assert_eq!(reread.notes.len(), decomposition.notes.len(), "{name}");
    }
}

#[test]
fn every_sample_format_survives_the_whole_trip_through_a_file() {
    let scratch = Scratch::new("formats");
    for format in [
        SampleFormat::PcmU8,
        SampleFormat::PcmI16,
        SampleFormat::PcmI24,
        SampleFormat::PcmI32,
    ] {
        let label = format.name();
        let source = repetitive_loop(8_000, format, 1);
        let written = scratch.join(&format!("{label}.wav"));
        audio::write_file(&written, &source).expect(label);

        let read = audio::read_file(&written).expect(label);
        assert_eq!(read.format(), format, "{label}");

        let decomposition = decompose(&read, &options(label)).expect(label);
        let root = scratch.join(label);
        archive::write_dir(&root, &decomposition).expect(label);
        let rebuilt = recompose::from_archive(&root).expect(label);

        let back = scratch.join(&format!("{label}-rebuilt.wav"));
        audio::write_file(&back, &rebuilt).expect(label);
        assert_eq!(
            std::fs::read(&written).unwrap(),
            std::fs::read(&back).unwrap(),
            "{label} does not come back as the file it went in as"
        );
    }
}

#[test]
fn both_containers_hold_the_same_recording() {
    let scratch = Scratch::new("containers");
    let source = wide_stereo(8_000, SampleFormat::PcmI16);
    let decomposition = decompose(&source, &options("stereo")).unwrap();
    let root = scratch.join("archive");
    archive::write_dir(&root, &decomposition).unwrap();
    let rebuilt = recompose::from_archive(&root).unwrap();

    for container in Container::ALL {
        let path = scratch.join(&format!("rebuilt.{}", container.extension()));
        audio::write_file(&path, &rebuilt).unwrap();
        let read = audio::read_file(&path).unwrap();
        assert_eq!(read.channels(), source.channels(), "{container:?}");
    }
}

#[test]
fn a_loop_is_stored_as_far_fewer_samples_than_it_plays() {
    // Deduplication is the point of the exercise: eight bars of the same two
    // waveforms must not be stored eight times over.
    let source = repetitive_loop(8_000, SampleFormat::PcmI16, 4);
    let decomposition = decompose(&source, &options("loop")).unwrap();
    let report = recompose::verify(&decomposition, &source).unwrap();

    assert!(report.is_exact());
    assert!(
        report.placements >= 4 * report.samples,
        "{} placements over {} samples is not much reuse",
        report.placements,
        report.samples
    );
    assert!(
        report.reuse() >= 4.0,
        "reuse was only {:.1}x",
        report.reuse()
    );
}

#[test]
fn a_recording_kept_whole_still_reconstructs_exactly() {
    // Without stem separation there is one timeline instead of three, which is
    // a different path through the matcher and the archive.
    let source = chord_progression(8_000, SampleFormat::PcmI16);
    let mut settings = options("whole");
    settings.stems = StemMode::None;
    let decomposition = decompose(&source, &settings).unwrap();

    let report = recompose::verify(&decomposition, &source).unwrap();
    assert!(report.is_exact(), "off by {:.3e}", report.deviation);
}

#[test]
fn chords_are_recognised_as_notes_and_written_into_the_projects() {
    let scratch = Scratch::new("notes");
    let source = chord_progression(8_000, SampleFormat::PcmI16);
    let decomposition = decompose(&source, &options("chords")).unwrap();
    assert!(
        !decomposition.notes.is_empty(),
        "no notes were recognised in a recording that is nothing but notes"
    );

    let root = scratch.join("archive");
    archive::write_dir(&root, &decomposition).unwrap();
    let written = project::export(&root, &SessionOptions::default()).unwrap();
    // At least one per format: LMMS also writes the MIDI file it references.
    assert!(written.len() >= project::Format::ALL.len());
    for path in &written {
        assert!(
            path.exists(),
            "{} was reported but not written",
            path.display()
        );
        assert!(
            std::fs::metadata(path).unwrap().len() > 0,
            "{} is empty",
            path.display()
        );
    }
}
