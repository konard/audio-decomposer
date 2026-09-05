//! Exporters and importers for the formats music software reads.
//!
//! - [`midi`] standard MIDI files, the common denominator of every DAW.
//! - [`xml`] the small XML writer and reader the project formats share.
//! - [`zip`] the store-only container DAWproject files are packed into.
//! - [`gzip`] the wrapper Ableton Live sets are compressed with.

pub mod gzip;
pub mod midi;
pub mod xml;
pub mod zip;
