//! The associative data model: links, their text projection, and the schema
//! the decomposition is written in.
//!
//! - [`store`] the deduplicating doublet network;
//! - [`lino`] Links Notation as the portable projection of that network;
//! - [`schema`] the manifest a decomposition is written as;
//! - `native` the optional file-mapped upstream store (`doublets-native`).

pub mod lino;
#[cfg(feature = "doublets-native")]
pub mod native;
pub mod schema;
pub mod store;

pub use store::{Doublet, LinkIndex, LinkStore, NULL};
