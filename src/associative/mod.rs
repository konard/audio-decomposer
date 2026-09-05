//! The associative data model: links, their text projection, and the schema
//! the decomposition is written in.
//!
//! - [`store`] the deduplicating doublet network;
//! - [`lino`] Links Notation as the portable projection of that network.

pub mod lino;
pub mod store;

pub use store::{Doublet, LinkIndex, LinkStore, NULL};
