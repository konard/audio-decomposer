//! Exporters and importers for the formats music software reads.
//!
//! - [`session`] the neutral session model every project exporter reads.
//! - [`midi`] standard MIDI files, the common denominator of every DAW.
//! - [`musicxml`] the notation the score editors read.
//! - [`dawproject`] the open interchange format Bitwig and Studio One read.
//! - [`ableton`] the gzipped sets Ableton Live reads.
//! - [`flstudio`] the binary projects FL Studio reads.
//! - [`ardour`] the XML sessions Ardour reads.
//! - [`reaper`] the plain-text projects REAPER reads.
//! - [`lmms`] the open projects LMMS reads.
//! - [`xml`] the small XML writer and reader the project formats share.
//! - [`zip`] the store-only container DAWproject files are packed into.
//! - [`gzip`] the wrapper Ableton Live sets are compressed with.
//! - [`project`] writes every one of them beside the audio they point at.
//! - [`sfz`] the sample bank as an instrument every sampler can load.

pub mod ableton;
pub mod ardour;
pub mod dawproject;
pub mod flstudio;
pub mod gzip;
pub mod lmms;
pub mod midi;
pub mod musicxml;
pub mod project;
pub mod reaper;
pub mod session;
pub mod sfz;
pub mod xml;
pub mod zip;
