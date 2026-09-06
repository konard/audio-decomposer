//! What a DAW would find in the exported projects.
//!
//! Writing a file is easy; writing one that opens is the part worth testing.
//! Every format this crate can read is read back and checked against the
//! decomposition it was written from, and the ones it only writes are checked
//! for the markers their readers look for.

use std::fs;

use audio_decomposer::audio::SampleFormat;
use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::formats::session::{SessionOptions, TrackKind};
use audio_decomposer::formats::{midi, musicxml, project, sfz};
use audio_decomposer::{archive, formats::project::Format};

use super::fixtures::{chord_progression, Scratch};

/// A decomposition of the chord fixture, written to disk with every project.
struct Exported {
    scratch: Scratch,
    notes: usize,
}

impl Exported {
    fn new(label: &str) -> Self {
        let scratch = Scratch::new(label);
        let source = chord_progression(8_000, SampleFormat::PcmI16);
        let decomposition = decompose(
            &source,
            &DecomposeOptions {
                name: "chords".to_string(),
                candidates: 8,
                ..DecomposeOptions::default()
            },
        )
        .unwrap();
        let notes = decomposition.notes.len();
        archive::write_dir(scratch.path(), &decomposition).unwrap();
        project::export(scratch.path(), &SessionOptions::default()).unwrap();
        Self { scratch, notes }
    }

    fn file(&self, extension: &str) -> std::path::PathBuf {
        self.scratch.join(&format!("chords.{extension}"))
    }

    fn text(&self, extension: &str) -> String {
        fs::read_to_string(self.file(extension))
            .unwrap_or_else(|error| panic!("chords.{extension}: {error}"))
    }
}

#[test]
fn every_session_format_reads_back_as_the_session_it_was_written_from() {
    let exported = Exported::new("projects-read-back");
    for format in Format::ALL {
        let path = exported.file(format.extension());
        let Ok(session) = project::read(&path) else {
            continue; // MIDI, MusicXML and SFZ hold no session of their own.
        };
        if format == Format::Lmms {
            // An LMMS project holds no sample rate: the rate is a setting of
            // the program, not of the piece. The reader says so by defaulting.
            assert_eq!(session.sample_rate, 48_000);
        } else {
            assert_eq!(session.sample_rate, 8_000, "{format:?}");
        }
        assert!(session.tempo > 0.0, "{format:?}");
        assert!(
            session
                .tracks
                .iter()
                .any(|track| !track.clips.is_empty() || !track.notes.is_empty()),
            "{format:?} came back with nothing on any track"
        );
    }
}

#[test]
fn the_midi_file_holds_the_notes_that_were_recognised() {
    let exported = Exported::new("projects-midi");
    let file = midi::read_file(exported.file("mid")).unwrap();
    let notes = midi::to_notes(&file, 8_000);
    assert_eq!(notes.len(), exported.notes);
    assert!(notes.iter().all(|note| note.length > 0));
}

#[test]
fn the_score_holds_the_notes_that_were_recognised() {
    let exported = Exported::new("projects-score");
    let notes = musicxml::parse(&exported.text("musicxml"), 8_000, None).unwrap();
    assert_eq!(notes.len(), exported.notes);
}

#[test]
fn the_sfz_instrument_points_at_samples_that_exist() {
    let exported = Exported::new("projects-sfz");
    let regions = sfz::read_file(exported.file("sfz")).unwrap();
    assert!(!regions.is_empty(), "the bank produced no playable regions");
    for region in &regions {
        let path = exported.scratch.join(&region.sample);
        assert!(path.exists(), "{} is missing", region.sample);
        assert!(region.low_key <= region.key_center && region.key_center <= region.high_key);
    }
}

#[test]
fn every_clip_a_project_names_is_a_file_that_exists() {
    let exported = Exported::new("projects-clips");
    for format in Format::ALL {
        let Ok(session) = project::read(exported.file(format.extension())) else {
            continue;
        };
        for track in &session.tracks {
            for clip in &track.clips {
                let path = exported.scratch.join(&clip.file);
                assert!(
                    path.exists(),
                    "{format:?} points {} at {}, which is not there",
                    track.name,
                    clip.file
                );
            }
        }
    }
}

#[test]
fn the_recognised_notes_travel_on_an_instrument_track() {
    let exported = Exported::new("projects-instrument");
    let mut seen = 0;
    for format in Format::ALL {
        let Ok(session) = project::read(exported.file(format.extension())) else {
            continue;
        };
        let notes: usize = session
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Instrument)
            .map(|track| track.notes.len())
            .sum();
        assert_eq!(notes, exported.notes, "{format:?}");
        seen += 1;
    }
    assert!(seen >= 5, "only {seen} formats read back as sessions");
}
