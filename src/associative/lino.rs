//! Links Notation as the portable text projection of the network.
//!
//! A decomposition is stored twice: as links in a [`LinkStore`], which is what
//! the deduplication works on, and as a `.lino` document, which is what a
//! human reads and what other tools in the associative stack can load. This
//! module is the bridge, and it is exactly reversible: parsing a document into
//! the network and projecting it back yields the same document.
//!
//! Every node is tagged, so the projection never has to guess whether a link
//! is a reference, a list, or a nested link:
//!
//! ```text
//! reference v  ->  (ref  . symbol)
//! list         ->  (cons . (item . rest))  ending in the `nil` marker
//! link id vs   ->  (link . (id-or-nil . list))
//! ```
//!
//! Because [`LinkStore::create`] deduplicates, two identical subtrees anywhere
//! in the document end up as the same link.

use links_notation::{format_links, parse_lino_to_links, LiNo};

use crate::associative::store::{LinkIndex, LinkStore, NULL};
use crate::error::{Error, Result};
use crate::parse_error;

/// A parsed Links Notation document: a flat list of top-level links.
pub type Document = Vec<LiNo<String>>;

/// Marker symbols that tag every node stored by this module.
#[derive(Clone, Copy, Debug)]
pub struct Markers {
    /// Tags a reference node.
    pub reference: LinkIndex,
    /// Tags a list cell.
    pub cons: LinkIndex,
    /// Terminates a list.
    pub nil: LinkIndex,
    /// Tags a link node.
    pub link: LinkIndex,
    /// Tags the document root.
    pub document: LinkIndex,
}

impl Markers {
    /// Interns the markers in a store, reusing them if they already exist.
    pub fn intern(store: &mut LinkStore) -> Self {
        Self {
            reference: store.symbol("lino:ref"),
            cons: store.symbol("lino:cons"),
            nil: store.symbol("lino:nil"),
            link: store.symbol("lino:link"),
            document: store.symbol("lino:document"),
        }
    }
}

/// Parses a Links Notation document.
///
/// # Errors
///
/// Returns [`Error::Parse`] when the document is not valid Links Notation.
pub fn parse(document: &str) -> Result<Document> {
    parse_lino_to_links(document).map_err(|error| parse_error!("{error}"))
}

/// Formats a document back to Links Notation, one link per line.
#[must_use]
pub fn format(document: &Document) -> String {
    let mut text = format_links(document);
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Builds a link node from an identifier and its values.
#[must_use]
pub fn link(id: &str, values: Vec<LiNo<String>>) -> LiNo<String> {
    LiNo::Link {
        id: Some(id.to_string()),
        values,
    }
}

/// Builds a reference node.
#[must_use]
pub fn reference(value: impl Into<String>) -> LiNo<String> {
    LiNo::Ref(value.into())
}

/// Builds an anonymous `(key value)` property node.
#[must_use]
pub fn property(key: &str, value: impl Into<String>) -> LiNo<String> {
    LiNo::Link {
        id: None,
        values: vec![reference(key), reference(value)],
    }
}

/// Stores a whole document in the network, returning the root link.
pub fn store_document(store: &mut LinkStore, document: &Document) -> LinkIndex {
    let markers = Markers::intern(store);
    let items: Vec<LinkIndex> = document
        .iter()
        .map(|node| store_node(store, markers, node))
        .collect();
    let list = store_list(store, markers, &items);
    store.create(markers.document, list)
}

/// Projects a document root back out of the network.
///
/// # Errors
///
/// Returns [`Error::Format`] when the link is not a document written by
/// [`store_document`].
pub fn read_document(store: &LinkStore, root: LinkIndex) -> Result<Document> {
    let markers = expect_markers(store)?;
    let doublet = store
        .get(root)
        .ok_or_else(|| Error::Format(format!("link {root} is not in the store")))?;
    if doublet.source != markers.document {
        return Err(Error::Format(format!(
            "link {root} is not a Links Notation document"
        )));
    }
    read_list(store, markers, doublet.target)?
        .into_iter()
        .map(|item| read_node(store, markers, item))
        .collect()
}

fn expect_markers(store: &LinkStore) -> Result<Markers> {
    let lookup = |name: &str| {
        store
            .find_symbol(name)
            .ok_or_else(|| Error::Format(format!("store has no `{name}` marker")))
    };
    Ok(Markers {
        reference: lookup("lino:ref")?,
        cons: lookup("lino:cons")?,
        nil: lookup("lino:nil")?,
        link: lookup("lino:link")?,
        document: lookup("lino:document")?,
    })
}

fn store_node(store: &mut LinkStore, markers: Markers, node: &LiNo<String>) -> LinkIndex {
    match node {
        LiNo::Ref(value) => {
            let symbol = store.symbol(value);
            store.create(markers.reference, symbol)
        }
        LiNo::Link { id, values } => {
            let identifier = id.as_ref().map_or(markers.nil, |name| {
                let symbol = store.symbol(name);
                store.create(markers.reference, symbol)
            });
            let items: Vec<LinkIndex> = values
                .iter()
                .map(|value| store_node(store, markers, value))
                .collect();
            let list = store_list(store, markers, &items);
            let body = store.create(identifier, list);
            store.create(markers.link, body)
        }
    }
}

fn store_list(store: &mut LinkStore, markers: Markers, items: &[LinkIndex]) -> LinkIndex {
    items.iter().rev().fold(markers.nil, |rest, item| {
        let cell = store.create(*item, rest);
        store.create(markers.cons, cell)
    })
}

fn read_list(store: &LinkStore, markers: Markers, mut list: LinkIndex) -> Result<Vec<LinkIndex>> {
    let mut items = Vec::new();
    while list != markers.nil {
        let cell = store
            .get(list)
            .ok_or_else(|| Error::Format(format!("link {list} is not in the store")))?;
        if cell.source != markers.cons {
            return Err(Error::Format(format!("link {list} is not a list cell")));
        }
        let pair = store
            .get(cell.target)
            .ok_or_else(|| Error::Format(format!("link {} is not in the store", cell.target)))?;
        items.push(pair.source);
        list = pair.target;
        if list == NULL {
            return Err(Error::Format("list is not terminated".to_string()));
        }
    }
    Ok(items)
}

fn read_node(store: &LinkStore, markers: Markers, index: LinkIndex) -> Result<LiNo<String>> {
    let node = store
        .get(index)
        .ok_or_else(|| Error::Format(format!("link {index} is not in the store")))?;
    if node.source == markers.reference {
        let name = store
            .name(node.target)
            .ok_or_else(|| Error::Format(format!("link {} is not a symbol", node.target)))?;
        return Ok(LiNo::Ref(name.to_string()));
    }
    if node.source != markers.link {
        return Err(Error::Format(format!(
            "link {index} is neither a reference nor a link node"
        )));
    }
    let body = store
        .get(node.target)
        .ok_or_else(|| Error::Format(format!("link {} is not in the store", node.target)))?;
    let id = if body.source == markers.nil {
        None
    } else {
        match read_node(store, markers, body.source)? {
            LiNo::Ref(name) => Some(name),
            LiNo::Link { .. } => {
                return Err(Error::Format(format!(
                    "link {index} has a compound identifier"
                )))
            }
        }
    };
    let values = read_list(store, markers, body.target)?
        .into_iter()
        .map(|item| read_node(store, markers, item))
        .collect::<Result<Vec<_>>>()?;
    Ok(LiNo::Link { id, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DOCUMENT: &str = "\
(decomposition: version 1)
(source: (sample-rate 44100) (channels 2) (format i16))
(sample: s1 (frames 1024) (peak 0.5))
(sample: s2 (frames 1024) (peak 0.5))
(placement: s1 0 0 1)
(placement: s2 44100 0 1)
";

    #[test]
    fn documents_round_trip_through_text() {
        let document = parse(SAMPLE_DOCUMENT).unwrap();

        assert_eq!(document.len(), 6);
        assert_eq!(format(&document), SAMPLE_DOCUMENT);
        assert_eq!(parse("").unwrap(), Vec::new());
        assert_eq!(format(&Vec::new()), "");
        assert!(parse("(unclosed").is_err());
    }

    #[test]
    fn documents_round_trip_through_the_network() {
        let document = parse(SAMPLE_DOCUMENT).unwrap();
        let mut store = LinkStore::new();
        let root = store_document(&mut store, &document);
        let restored = read_document(&store, root).unwrap();

        assert_eq!(restored, document);
        assert_eq!(format(&restored), SAMPLE_DOCUMENT);
    }

    #[test]
    fn identical_subtrees_are_stored_once() {
        let mut store = LinkStore::new();
        let single = parse("(sample: s1 (frames 1024) (peak 0.5))").unwrap();
        store_document(&mut store, &single);
        let after_first = store.len();

        let mut repeated = LinkStore::new();
        let twice =
            parse("(sample: s1 (frames 1024) (peak 0.5))\n(sample: s1 (frames 1024) (peak 0.5))")
                .unwrap();
        store_document(&mut repeated, &twice);

        // The second copy adds only the list cell that holds it.
        assert!(
            repeated.len() <= after_first + 2,
            "duplicate stored {} links against {after_first}",
            repeated.len()
        );
    }

    #[test]
    fn builders_produce_the_expected_notation() {
        let document = vec![
            link("sample", vec![reference("s1"), property("frames", "1024")]),
            link("note", vec![reference("60"), reference("0.5")]),
        ];

        assert_eq!(
            format(&document),
            "(sample: s1 (frames 1024))\n(note: 60 0.5)\n"
        );
        assert_eq!(parse(&format(&document)).unwrap(), document);
    }

    #[test]
    fn anonymous_and_nested_links_survive_the_network() {
        let document = parse("((a b) (c: (d: e)))").unwrap();
        let mut store = LinkStore::new();
        let root = store_document(&mut store, &document);

        assert_eq!(read_document(&store, root).unwrap(), document);
    }

    #[test]
    fn malformed_roots_are_reported() {
        let mut store = LinkStore::new();
        let document = parse("(a: b)").unwrap();
        let root = store_document(&mut store, &document);

        assert!(read_document(&store, root + 1000).is_err());
        let stray = store.symbol("stray");
        assert!(read_document(&store, stray).is_err());
        assert!(read_document(&LinkStore::new(), root).is_err());
    }
}
