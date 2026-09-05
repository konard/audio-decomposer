//! Decompose audio into deduplicated samples, stems, notes and projects.
//!
//! The pipeline is deliberately reversible at every level:
//!
//! 1. [`audio`] decodes a container into planar `f64` channels on an exact
//!    integer grid.
//! 2. [`dsp`] provides the analysis primitives (FFT, STFT, windows, onsets,
//!    pitch detection, harmonic/percussive separation).
//! 3. `decompose` splits a recording into stems, cuts the stems into events,
//!    deduplicates those events into a sample bank by subtracting the best
//!    aligned and scaled match, and keeps an exact residual.
//! 4. [`associative`] stores the whole result as a doublet link network that is
//!    serialised to Links Notation.
//! 5. `formats` exports the result as MIDI, MusicXML, DAWproject, Ableton
//!    Live, FL Studio, REAPER, Ardour, LMMS and SFZ projects plus WAV/AIFF
//!    stems and samples.
//! 6. `recompose` rebuilds the audio, sample-exactly for integer PCM input.
pub mod associative;
pub mod audio;
pub mod decompose;
pub mod dsp;
pub mod error;

pub use audio::{Audio, SampleFormat};
pub use error::{Error, Result};
