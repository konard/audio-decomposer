//! The same round trip, over real public-domain recordings instead of
//! synthesised ones.
//!
//! Synthesised fixtures keep the suite fast, hermetic and free of any
//! copyright question, but they are also kinder to the analysis than a room
//! with a microphone in it. This test runs the whole pipeline over actual
//! music when actual music is available, and does nothing at all when it is
//! not, so the default `cargo test` stays offline and deterministic.
//!
//! Fill a corpus with `cargo run --release --example public_domain_corpus`,
//! which downloads only files Commons states are Public Domain or CC0 and
//! writes a `CREDITS.md` beside them, then point the suite at it:
//!
//! ```text
//! AUDIO_DECOMPOSER_CORPUS=corpus cargo test --test integration public_domain
//! ```

use std::path::{Path, PathBuf};

use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::{audio, recompose, Audio, Container};

/// Environment variable naming a directory of public-domain recordings.
const CORPUS: &str = "AUDIO_DECOMPOSER_CORPUS";

/// Seconds of each recording that are analysed.
///
/// Matching pursuit grows with the number of events, so a whole movement would
/// turn one test into a coffee break. The opening is enough to exercise every
/// stage against real timbre, real reverb and real noise.
const SECONDS: f64 = 8.0;

/// Every readable recording in the corpus, or nothing when there is no corpus.
fn corpus() -> Vec<PathBuf> {
    let Some(directory) = std::env::var_os(CORPUS) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        panic!(
            "{CORPUS} points at {}, which cannot be read",
            PathBuf::from(directory).display()
        );
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| Container::of_path(path).is_some())
        .collect();
    // Read in a stated order so a failure names the same file every run.
    paths.sort();
    assert!(
        !paths.is_empty(),
        "{CORPUS} points at a directory with no WAV, AIFF or FLAC files in it"
    );
    paths
}

/// The opening of a recording, at whatever rate and depth it was published in.
fn opening(path: &Path) -> Audio {
    let name = path.display();
    let whole = audio::read_file(path).unwrap_or_else(|error| panic!("{name}: {error}"));
    let wanted = (SECONDS * f64::from(whole.sample_rate())) as usize;
    if wanted < whole.frames() {
        whole.segment(0, wanted)
    } else {
        whole
    }
}

#[test]
fn real_recordings_come_apart_and_go_back_together_exactly() {
    let corpus = corpus();
    if corpus.is_empty() {
        eprintln!("{CORPUS} is unset; skipping the public-domain corpus");
        return;
    }

    for path in &corpus {
        let name = path.display();
        let original = opening(path);
        assert!(original.frames() > 0, "{name} decoded to nothing at all");

        let options = DecomposeOptions {
            name: path.file_stem().map_or_else(
                || "recording".to_string(),
                |stem| stem.to_string_lossy().into_owned(),
            ),
            candidates: 8,
            ..DecomposeOptions::default()
        };
        let decomposition =
            decompose(&original, &options).unwrap_or_else(|error| panic!("{name}: {error}"));
        let report = recompose::verify(&decomposition, &original)
            .unwrap_or_else(|error| panic!("{name}: {error}"));

        assert!(
            report.is_exact(),
            "{name} rebuilt with a deviation of {}",
            report.deviation
        );
        assert!(
            report.stems_are_exact(),
            "{name} stems miss the original by {}",
            report.stem_deviation
        );
        assert!(
            report.samples > 0 && report.placements >= report.samples,
            "{name} produced {} sample(s) for {} placement(s)",
            report.samples,
            report.placements
        );
    }
}

#[test]
fn real_recordings_survive_every_container_they_can_be_written_to() {
    let corpus = corpus();
    if corpus.is_empty() {
        eprintln!("{CORPUS} is unset; skipping the public-domain corpus");
        return;
    }

    for path in &corpus {
        let name = path.display();
        let original = opening(path);
        for container in Container::ALL {
            let bytes = audio::encode(&original, container)
                .unwrap_or_else(|error| panic!("{name} as {container:?}: {error}"));
            let read = audio::decode(&bytes)
                .unwrap_or_else(|error| panic!("{name} as {container:?}: {error}"));
            assert_eq!(
                read.channels(),
                original.channels(),
                "{name} changed passing through {container:?}"
            );
            assert_eq!(read.sample_rate(), original.sample_rate());
        }
    }
}
