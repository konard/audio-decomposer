//! Signal analysis used by the decomposition pipeline.
//!
//! Everything here is written from first principles and has no dependencies
//! outside the standard library, so an analysis run is reproducible from the
//! source in this repository alone:
//!
//! - [`fft`] complex arithmetic, a radix-2 transform, and FFT cross-correlation;
//! - [`window`] analysis windows and the overlap-add condition they must meet;
//! - [`stft`] an exactly invertible short-time Fourier transform;
//! - [`hpss`] harmonic/percussive separation by median filtering;
//! - [`onset`] spectral-flux onset detection, which cuts the signal into notes;
//! - [`pitch`] monophonic YIN pitch tracking and note-number conversions;
//! - [`polyphonic`] multi-pitch estimation by harmonic-sum salience; and
//! - [`envelope`] amplitude envelopes and the velocities derived from them.

pub mod envelope;
pub mod fft;
pub mod hpss;
pub mod onset;
pub mod pitch;
pub mod polyphonic;
pub mod stft;
pub mod window;

pub use fft::{Complex, Direction};
pub use stft::{Spectrogram, StftOptions};
pub use window::WindowKind;
