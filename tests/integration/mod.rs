//! End-to-end tests: the library and the command line over whole recordings.
//!
//! Every recording these tests use is generated here, from sine waves and
//! noise, so nothing copyrighted is ever committed or downloaded. The point of
//! the suite is the claim the project is built on: a decomposition of integer
//! PCM audio composes back to the file it came from, byte for byte.

#[path = "fixtures.rs"]
mod fixtures;

#[path = "round_trip.rs"]
mod round_trip;

#[path = "projects.rs"]
mod projects;

#[path = "command_line.rs"]
mod command_line;
