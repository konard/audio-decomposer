//! Decompose audio into deduplicated samples, stems, notes and projects.
//!
//! The pipeline is deliberately reversible at every level:
//!
//! 1. [`audio`] decodes a container into planar `f64` channels on an exact
//!    integer grid.
//! 2. [`dsp`] provides the analysis primitives (FFT, STFT, windows, onsets,
//!    pitch detection, harmonic/percussive separation).
//! 3. [`decompose`] splits a recording into stems, cuts the stems into events,
//!    deduplicates those events into a sample bank by subtracting the best
//!    aligned and scaled match, and keeps an exact residual.
//! 4. [`associative`] stores the whole result as a doublet link network that is
//!    serialised to Links Notation.
//! 5. [`archive`] writes the whole decomposition to a directory and reads it
//!    back unchanged.
//! 6. [`formats`] exports the result as MIDI, MusicXML, DAWproject, Ableton
//!    Live, FL Studio, REAPER, Ardour, LMMS and SFZ projects plus WAV/AIFF
//!    stems and samples.
//! 7. [`recompose`] rebuilds the audio, sample-exactly for integer PCM input.
//!
//! # At a glance
//!
//! Take a recording apart and put it back together:
//!
//! ```no_run
//! use audio_decomposer::decompose::{decompose, DecomposeOptions};
//! use audio_decomposer::{audio, recompose};
//!
//! let original = audio::read_file("song.wav")?;
//! let decomposition = decompose(&original, &DecomposeOptions::default())?;
//! let rebuilt = recompose::reconstruct(&decomposition);
//!
//! // For integer PCM input the reconstruction is exact, not approximate.
//! assert_eq!(original.channels(), rebuilt.channels());
//! # Ok::<(), audio_decomposer::Error>(())
//! ```
//!
//! Keep it on disk and read it back unchanged:
//!
//! ```no_run
//! use audio_decomposer::archive;
//! # use audio_decomposer::decompose::{decompose, DecomposeOptions};
//! # use audio_decomposer::audio;
//! # let original = audio::read_file("song.wav")?;
//! # let decomposition = decompose(&original, &DecomposeOptions::default())?;
//! archive::write_dir("song-decomposition", &decomposition)?;
//! let same = archive::read_dir("song-decomposition")?;
//! assert_eq!(same.samples.len(), decomposition.samples.len());
//! # Ok::<(), audio_decomposer::Error>(())
//! ```
//!
//! Write it out as the projects music software already reads:
//!
//! ```no_run
//! use audio_decomposer::formats::project;
//! use audio_decomposer::formats::session::SessionOptions;
//!
//! let written = project::export("song-decomposition", &SessionOptions::default())?;
//! for path in &written {
//!     println!("{}", path.display());
//! }
//! # Ok::<(), audio_decomposer::Error>(())
//! ```
pub mod archive;
pub mod associative;
pub mod audio;
pub mod decompose;
pub mod dsp;
pub mod error;
pub mod formats;
pub mod recompose;

pub use audio::{Audio, Container, SampleFormat};
pub use error::{Error, Result};
