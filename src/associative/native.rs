//! The upstream Links Platform store as an optional backend.
//!
//! [`LinkStore`] keeps the network in a `Vec`, which is enough for a single
//! decomposition and keeps the default build dependency-free. When a network
//! has to outlive the process — a sample library that grows across many
//! recordings, say — the same doublets can be mirrored into the file-mapped
//! store from the [`doublets`] crate, which is the physical network the
//! associative technology stack is built on.
//!
//! The two directions are exact inverses:
//!
//! * [`NativeStore::mirror`] replays a [`LinkStore`] into the native store in
//!   creation order, so every link keeps its address;
//! * [`NativeStore::snapshot`] replays the native store back into a
//!   [`LinkStore`], again in address order.
//!
//! Symbol names are not part of the doublet network — natively a symbol is just
//! a point — so `mirror` carries them along in memory and `snapshot` restores
//! them. The durable, self-describing projection of names remains Links
//! Notation, written by [`crate::associative::lino`].
//!
//! # Why the mapping is not an archive
//!
//! [`NativeStore::file_mapped`] backs the network with a memory-mapped file,
//! which lets a network larger than RAM spill to disk. It does **not** make the
//! network durable: reopening the same path yields an empty store, because
//! `doublets::mem::resize_mem` grows the mapping with `RawMem::grow_filled`,
//! which overwrites the whole newly mapped region with default link parts and
//! so erases the header the previous run wrote. `experiments/doublets-persistence`
//! demonstrates this against `doublets` 0.5.0. Everything this crate has to
//! keep is therefore written as Links Notation.
//!
//! This module is compiled only with the `doublets-native` feature.

use std::collections::HashMap;
use std::path::Path;

use doublets::mem::{ErasedMem, FileMapped, Global};
use doublets::{unit, Doublets, DoubletsExt};

use crate::associative::store::{Doublet, LinkIndex, LinkStore, NULL};
use crate::error::Result;
use crate::storage_error;

/// The link record the unit store keeps in memory.
type Part = doublets::parts::LinkPart<u64>;

/// Any memory backend, chosen at run time.
type Memory = Box<dyn ErasedMem<Item = Part>>;

/// A doublet network held in the upstream file-mapped store.
pub struct NativeStore {
    inner: unit::Store<u64, Memory>,
    names: HashMap<LinkIndex, String>,
    symbols: HashMap<String, LinkIndex>,
}

impl std::fmt::Debug for NativeStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeStore")
            .field("len", &self.len())
            .field("names", &self.names.len())
            .field("symbols", &self.symbols.len())
            .finish_non_exhaustive()
    }
}

impl NativeStore {
    /// A network held in process memory.
    pub fn in_memory() -> Result<Self> {
        Self::with_memory(Box::new(Global::<Part>::new()))
    }

    /// A network backed by a memory-mapped file, so that a network larger than
    /// RAM can spill to disk.
    ///
    /// The mapping is scratch space for the current run, not an archive: see
    /// the module documentation. The file is created when it does not exist and
    /// any content it already has is overwritten.
    pub fn file_mapped(path: impl AsRef<Path>) -> Result<Self> {
        let mapped = FileMapped::<Part>::from_path(path.as_ref())
            .map_err(|error| storage_error!("cannot map {}: {error}", path.as_ref().display()))?;
        Self::with_memory(Box::new(mapped))
    }

    fn with_memory(memory: Memory) -> Result<Self> {
        let inner = unit::Store::<u64, Memory>::new(memory)
            .map_err(|error| storage_error!("cannot open the link store: {error}"))?;
        Ok(Self {
            inner,
            names: HashMap::new(),
            symbols: HashMap::new(),
        })
    }

    /// Number of links in the network.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.count() as usize
    }

    /// Whether the network holds no links.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Creates a link pointing at itself.
    pub fn create_point(&mut self) -> Result<LinkIndex> {
        let index = self
            .inner
            .create_point()
            .map_err(|error| storage_error!("cannot create a point: {error}"))?;
        Ok(index as LinkIndex)
    }

    /// Creates a doublet, returning the existing link when an identical one is
    /// already stored — the same deduplicating contract as [`LinkStore::create`].
    pub fn create(&mut self, source: LinkIndex, target: LinkIndex) -> Result<LinkIndex> {
        if let Some(existing) = self.find(source, target) {
            return Ok(existing);
        }
        let index = self
            .inner
            .create_link(source as u64, target as u64)
            .map_err(|error| storage_error!("cannot create ({source} {target}): {error}"))?;
        Ok(index as LinkIndex)
    }

    /// The address of an existing doublet, if it is stored.
    #[must_use]
    pub fn find(&self, source: LinkIndex, target: LinkIndex) -> Option<LinkIndex> {
        self.inner
            .search(source as u64, target as u64)
            .map(|index| index as LinkIndex)
    }

    /// Interns a named point, returning the same address for the same name.
    pub fn symbol(&mut self, name: &str) -> Result<LinkIndex> {
        if let Some(existing) = self.symbols.get(name) {
            return Ok(*existing);
        }
        let index = self.create_point()?;
        self.symbols.insert(name.to_string(), index);
        self.names.insert(index, name.to_string());
        Ok(index)
    }

    /// The name of a symbol, if the link is one.
    #[must_use]
    pub fn name(&self, index: LinkIndex) -> Option<&str> {
        self.names.get(&index).map(String::as_str)
    }

    /// The doublet stored at an address.
    #[must_use]
    pub fn get(&self, index: LinkIndex) -> Option<Doublet> {
        if index == NULL {
            return None;
        }
        self.inner
            .get_link(index as u64)
            .map(|link| Doublet::new(link.source as LinkIndex, link.target as LinkIndex))
    }

    /// Every link in the network as `(address, doublet)` pairs, in address
    /// order.
    #[must_use]
    pub fn links(&self) -> Vec<(LinkIndex, Doublet)> {
        let mut links: Vec<_> = self
            .inner
            .iter()
            .map(|link| {
                (
                    link.index as LinkIndex,
                    Doublet::new(link.source as LinkIndex, link.target as LinkIndex),
                )
            })
            .collect();
        links.sort_by_key(|(index, _)| *index);
        links
    }

    /// Replays a [`LinkStore`] into the native network.
    ///
    /// The network must be empty, so that replaying in creation order gives
    /// every link the address it had in the source network.
    pub fn mirror(&mut self, store: &LinkStore) -> Result<()> {
        if !self.is_empty() {
            return Err(storage_error!(
                "cannot mirror into a store that already holds {} links",
                self.len()
            ));
        }
        for (index, doublet) in store.iter() {
            let written = if doublet.is_point(index) {
                self.create_point()?
            } else {
                self.create(doublet.source, doublet.target)?
            };
            if written != index {
                return Err(storage_error!(
                    "link {index} was mirrored as {written}, so addresses would shift"
                ));
            }
            if let Some(name) = store.name(index) {
                self.names.insert(index, name.to_string());
                self.symbols.insert(name.to_string(), index);
            }
        }
        Ok(())
    }

    /// Replays the native network back into a [`LinkStore`].
    pub fn snapshot(&self) -> Result<LinkStore> {
        let mut store = LinkStore::new();
        for (index, doublet) in self.links() {
            let written = if doublet.is_point(index) {
                match self.name(index) {
                    Some(name) => store.symbol(name),
                    None => store.create_point(),
                }
            } else {
                store.create(doublet.source, doublet.target)
            };
            if written != index {
                return Err(storage_error!(
                    "link {index} was restored as {written}, so addresses would shift"
                ));
            }
        }
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_store() -> LinkStore {
        let mut store = LinkStore::new();
        let note = store.symbol("note");
        let pitch = store.symbol("pitch");
        let value = store.create_point();
        let first = store.create(note, store.find_symbol("pitch").unwrap());
        let second = store.create(first, value);
        assert_eq!(store.create(note, pitch), first);
        assert_eq!(store.len(), 5);
        assert_eq!(second, 5);
        store
    }

    #[test]
    fn an_in_memory_network_creates_and_deduplicates_links() {
        let mut store = NativeStore::in_memory().unwrap();

        assert!(store.is_empty());

        let left = store.create_point().unwrap();
        let right = store.create_point().unwrap();
        let pair = store.create(left, right).unwrap();

        assert_eq!(store.create(left, right).unwrap(), pair);
        assert_eq!(store.len(), 3);
        assert_eq!(store.get(pair), Some(Doublet::new(left, right)));
        assert_eq!(store.find(left, right), Some(pair));
        assert_eq!(store.find(right, left), None);
        assert_eq!(store.get(NULL), None);
    }

    #[test]
    fn symbols_are_interned_once() {
        let mut store = NativeStore::in_memory().unwrap();

        let first = store.symbol("note").unwrap();

        assert_eq!(store.symbol("note").unwrap(), first);
        assert_eq!(store.name(first), Some("note"));
        assert_eq!(store.name(first + 1), None);
    }

    #[test]
    fn mirroring_preserves_every_address_and_name() {
        let source = sample_store();
        let mut native = NativeStore::in_memory().unwrap();

        native.mirror(&source).unwrap();

        assert_eq!(native.len(), source.len());
        for (index, doublet) in source.iter() {
            assert_eq!(native.get(index), Some(doublet), "link {index}");
            assert_eq!(native.name(index), source.name(index), "name of {index}");
        }

        let restored = native.snapshot().unwrap();

        assert_eq!(restored.to_string(), source.to_string());
        assert_eq!(restored.find_symbol("pitch"), source.find_symbol("pitch"));
    }

    #[test]
    fn mirroring_twice_is_refused() {
        let source = sample_store();
        let mut native = NativeStore::in_memory().unwrap();

        native.mirror(&source).unwrap();

        assert!(native.mirror(&source).is_err());
    }

    #[test]
    fn a_file_mapped_network_holds_the_same_links_as_an_in_memory_one() {
        let directory = std::env::temp_dir().join(format!(
            "audio-decomposer-native-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("links.doublets");

        let source = sample_store();
        let mut native = NativeStore::file_mapped(&path).unwrap();

        native.mirror(&source).unwrap();

        assert_eq!(native.len(), source.len());
        for (index, doublet) in source.iter() {
            assert_eq!(native.get(index), Some(doublet), "link {index}");
        }
        assert_eq!(native.snapshot().unwrap().to_string(), source.to_string());
        assert!(path.exists());

        drop(native);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
