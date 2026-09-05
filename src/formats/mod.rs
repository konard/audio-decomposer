//! Exporters and importers for the formats music software reads.
//!
//! - [`midi`] standard MIDI files, the common denominator of every DAW.
//! - [`xml`] the small XML writer and reader the project formats share.

pub mod midi;
pub mod xml;
