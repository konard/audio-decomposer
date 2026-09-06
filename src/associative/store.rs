//! An associative link network: the universal data model this crate stores
//! everything in.
//!
//! Following the associative technology stack, every fact is a *doublet* — a
//! link with a source and a target, both of which are themselves links. Two
//! properties make the model worth using here:
//!
//! * **Creation deduplicates.** Creating a doublet that already exists returns
//!   the existing link, so identical structures are stored once no matter how
//!   often they occur. Repeated audio content therefore collapses in the
//!   network exactly the way repeated notes collapse in a score.
//! * **Everything is addressable.** A sample, a placement, a note, and the
//!   whole composition are all link indices, so any of them can be a value of
//!   any other fact.
//!
//! [`LinkStore`] is the in-repository network used by default. It is a plain
//! `Vec` of doublets with a hash index, which keeps the crate dependency-free
//! for anyone who does not want the file-mapped upstream store; the optional
//! `doublets-native` feature mirrors the same network into `doublets`.

use std::collections::HashMap;
use std::fmt;

/// Address of a link. Index `0` is the null link and is never stored.
pub type LinkIndex = usize;

/// The null link: "no link".
pub const NULL: LinkIndex = 0;

/// A link with a source and a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Doublet {
    /// The link this one points from.
    pub source: LinkIndex,
    /// The link this one points to.
    pub target: LinkIndex,
}

impl Doublet {
    /// A doublet from its parts.
    #[must_use]
    pub const fn new(source: LinkIndex, target: LinkIndex) -> Self {
        Self { source, target }
    }

    /// Whether the doublet points at itself in both directions.
    #[must_use]
    pub const fn is_point(&self, index: LinkIndex) -> bool {
        self.source == index && self.target == index
    }
}

/// An associative network of doublets with interned symbols.
#[derive(Clone, Debug, Default)]
pub struct LinkStore {
    doublets: Vec<Doublet>,
    lookup: HashMap<Doublet, LinkIndex>,
    names: HashMap<LinkIndex, String>,
    symbols: HashMap<String, LinkIndex>,
}

impl LinkStore {
    /// An empty network.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of links in the network.
    #[must_use]
    pub fn len(&self) -> usize {
        self.doublets.len()
    }

    /// Whether the network holds no links.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.doublets.is_empty()
    }

    /// Creates a link pointing at itself, which is how values and markers are
    /// represented. Every call returns a new address.
    pub fn create_point(&mut self) -> LinkIndex {
        let index = self.doublets.len() + 1;
        let doublet = Doublet::new(index, index);
        self.doublets.push(doublet);
        self.lookup.insert(doublet, index);
        index
    }

    /// Creates a doublet, returning the existing link when an identical one is
    /// already stored.
    ///
    /// This is the deduplicating primitive the whole model rests on.
    pub fn create(&mut self, source: LinkIndex, target: LinkIndex) -> LinkIndex {
        let doublet = Doublet::new(source, target);
        if let Some(existing) = self.lookup.get(&doublet) {
            return *existing;
        }
        let index = self.doublets.len() + 1;
        self.doublets.push(doublet);
        self.lookup.insert(doublet, index);
        index
    }

    /// The address of an existing doublet, if it is stored.
    #[must_use]
    pub fn find(&self, source: LinkIndex, target: LinkIndex) -> Option<LinkIndex> {
        self.lookup.get(&Doublet::new(source, target)).copied()
    }

    /// Interns a named point, returning the same address for the same name.
    pub fn symbol(&mut self, name: &str) -> LinkIndex {
        if let Some(existing) = self.symbols.get(name) {
            return *existing;
        }
        let index = self.create_point();
        self.symbols.insert(name.to_string(), index);
        self.names.insert(index, name.to_string());
        index
    }

    /// The address of a symbol that has already been interned.
    #[must_use]
    pub fn find_symbol(&self, name: &str) -> Option<LinkIndex> {
        self.symbols.get(name).copied()
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
        self.doublets.get(index - 1).copied()
    }

    /// Source of the link at an address.
    #[must_use]
    pub fn source(&self, index: LinkIndex) -> LinkIndex {
        self.get(index).map_or(NULL, |doublet| doublet.source)
    }

    /// Target of the link at an address.
    #[must_use]
    pub fn target(&self, index: LinkIndex) -> LinkIndex {
        self.get(index).map_or(NULL, |doublet| doublet.target)
    }

    /// Whether the link at an address points at itself.
    #[must_use]
    pub fn is_point(&self, index: LinkIndex) -> bool {
        self.get(index)
            .is_some_and(|doublet| doublet.is_point(index))
    }

    /// Every address in the network, in creation order.
    pub fn indices(&self) -> impl Iterator<Item = LinkIndex> + '_ {
        1..=self.doublets.len()
    }

    /// Every link in the network as `(address, doublet)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (LinkIndex, Doublet)> + '_ {
        self.doublets
            .iter()
            .enumerate()
            .map(|(offset, doublet)| (offset + 1, *doublet))
    }

    /// All links whose source is `source`.
    #[must_use]
    pub fn by_source(&self, source: LinkIndex) -> Vec<LinkIndex> {
        self.iter()
            .filter(|(index, doublet)| doublet.source == source && !doublet.is_point(*index))
            .map(|(index, _)| index)
            .collect()
    }

    /// All links whose target is `target`.
    #[must_use]
    pub fn by_target(&self, target: LinkIndex) -> Vec<LinkIndex> {
        self.iter()
            .filter(|(index, doublet)| doublet.target == target && !doublet.is_point(*index))
            .map(|(index, _)| index)
            .collect()
    }

    /// Folds a sequence of links into left-associated doublets.
    ///
    /// An empty sequence yields [`NULL`] and a single element yields itself,
    /// so the encoding is total.
    pub fn sequence(&mut self, elements: &[LinkIndex]) -> LinkIndex {
        let mut iterator = elements.iter().copied();
        let Some(first) = iterator.next() else {
            return NULL;
        };
        iterator.fold(first, |accumulated, element| {
            self.create(accumulated, element)
        })
    }

    /// A stable, human-readable rendering of one link, used by diagnostics.
    #[must_use]
    pub fn describe(&self, index: LinkIndex) -> String {
        if index == NULL {
            return "null".to_string();
        }
        if let Some(name) = self.name(index) {
            return name.to_string();
        }
        match self.get(index) {
            None => format!("missing({index})"),
            Some(doublet) if doublet.is_point(index) => format!("point({index})"),
            Some(doublet) => format!(
                "{index}: ({} {})",
                self.describe(doublet.source),
                self.describe(doublet.target)
            ),
        }
    }
}

impl fmt::Display for LinkStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, doublet) in self.iter() {
            writeln!(formatter, "{index}: {} {}", doublet.source, doublet.target)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_store_is_empty() {
        let store = LinkStore::new();

        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.get(NULL), None);
        assert_eq!(store.get(1), None);
        assert_eq!(store.source(7), NULL);
        assert_eq!(store.target(7), NULL);
        assert_eq!(store.describe(NULL), "null");
        assert_eq!(store.describe(3), "missing(3)");
        assert!(!store.is_point(1));
    }

    #[test]
    fn points_are_distinct_and_self_referencing() {
        let mut store = LinkStore::new();
        let first = store.create_point();
        let second = store.create_point();

        assert_ne!(first, second);
        assert!(store.is_point(first));
        assert!(store.is_point(second));
        assert_eq!(store.get(first), Some(Doublet::new(first, first)));
        assert_eq!(store.describe(first), format!("point({first})"));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn creating_the_same_doublet_twice_returns_the_same_link() {
        let mut store = LinkStore::new();
        let left = store.symbol("left");
        let right = store.symbol("right");

        let first = store.create(left, right);
        let second = store.create(left, right);

        assert_eq!(first, second, "identical doublets must deduplicate");
        assert_eq!(store.len(), 3, "only one doublet on top of two symbols");
        assert_eq!(store.find(left, right), Some(first));
        assert_eq!(store.find(right, left), None);
        assert_eq!(store.source(first), left);
        assert_eq!(store.target(first), right);
    }

    #[test]
    fn symbols_are_interned_by_name() {
        let mut store = LinkStore::new();
        let first = store.symbol("sample");
        let second = store.symbol("sample");
        let other = store.symbol("placement");

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_eq!(store.name(first), Some("sample"));
        assert_eq!(store.name(999), None);
        assert_eq!(store.find_symbol("sample"), Some(first));
        assert_eq!(store.find_symbol("missing"), None);
        assert_eq!(store.describe(first), "sample");
    }

    #[test]
    fn sequences_fold_left_and_share_their_prefixes() {
        let mut store = LinkStore::new();
        let a = store.symbol("a");
        let b = store.symbol("b");
        let c = store.symbol("c");

        let first = store.sequence(&[a, b, c]);
        let before = store.len();
        let second = store.sequence(&[a, b, c]);
        let shared_prefix = store.sequence(&[a, b]);

        assert_eq!(first, second);
        assert_eq!(store.len(), before, "re-encoding stores nothing new");
        assert_eq!(store.source(first), shared_prefix);
        assert_eq!(store.target(first), c);
        assert_eq!(store.sequence(&[]), NULL);
        assert_eq!(store.sequence(&[a]), a);
    }

    #[test]
    fn links_can_be_searched_by_source_and_target() {
        let mut store = LinkStore::new();
        let subject = store.symbol("subject");
        let first = store.symbol("first");
        let second = store.symbol("second");
        let left = store.create(subject, first);
        let right = store.create(subject, second);

        assert_eq!(store.by_source(subject), vec![left, right]);
        assert_eq!(store.by_target(first), vec![left]);
        assert_eq!(store.by_target(subject), Vec::<LinkIndex>::new());
        assert_eq!(store.indices().count(), store.len());
        assert_eq!(store.iter().count(), store.len());
    }

    #[test]
    fn the_store_renders_as_a_link_table() {
        let mut store = LinkStore::new();
        let a = store.symbol("a");
        let b = store.symbol("b");
        let pair = store.create(a, b);

        assert_eq!(store.to_string(), "1: 1 1\n2: 2 2\n3: 1 2\n");
        assert_eq!(store.describe(pair), "3: (a b)");
    }
}
