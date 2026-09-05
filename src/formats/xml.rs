//! A small XML writer and reader.
//!
//! Four of the project formats this crate exports are XML: MusicXML,
//! DAWproject, Ableton Live and LMMS, and Ardour makes five. Pulling in a
//! general XML library for them would be more code than writing the part they
//! need, and none of them needs namespaces, doctypes or entity declarations.
//! What they do need is exactness: an exporter that cannot be read back cannot
//! be tested, so this module parses as well as it writes, and every exporter
//! test reads its own output.

use crate::error::{Error, Result};

/// An XML element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    /// Tag name.
    pub name: String,
    /// Attributes, in the order they were added.
    pub attributes: Vec<(String, String)>,
    /// Child nodes.
    pub children: Vec<Node>,
}

/// Anything that can sit inside an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A nested element.
    Element(Element),
    /// Character data.
    Text(String),
}

impl Element {
    /// An empty element.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Adds an attribute.
    #[must_use]
    pub fn attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.push((key.into(), value.into()));
        self
    }

    /// Adds an attribute only when there is a value for it.
    #[must_use]
    pub fn maybe(self, key: impl Into<String>, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(value) => self.attribute(key, value),
            None => self,
        }
    }

    /// Adds a child element.
    #[must_use]
    pub fn child(mut self, child: Self) -> Self {
        self.children.push(Node::Element(child));
        self
    }

    /// Adds several child elements.
    #[must_use]
    pub fn extend(mut self, children: impl IntoIterator<Item = Self>) -> Self {
        self.children
            .extend(children.into_iter().map(Node::Element));
        self
    }

    /// Adds character data.
    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.children.push(Node::Text(text.into()));
        self
    }

    /// A child element with a name and nothing but text in it.
    #[must_use]
    pub fn leaf(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(name).text(text)
    }

    /// The first child element with this name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Self> {
        self.elements().find(|element| element.name == name)
    }

    /// Every child element with this name.
    pub fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.elements().filter(move |element| element.name == name)
    }

    /// Every child element.
    pub fn elements(&self) -> impl Iterator<Item = &Self> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// The value of an attribute.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The character data directly inside this element.
    #[must_use]
    pub fn content(&self) -> String {
        self.children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                Node::Element(_) => None,
            })
            .collect()
    }

    /// Writes the element and everything under it.
    pub fn write(&self, out: &mut String, depth: usize) {
        let padding = "  ".repeat(depth);
        out.push_str(&padding);
        out.push('<');
        out.push_str(&self.name);
        for (key, value) in &self.attributes {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            out.push_str(&escape(value));
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push_str(">\n");
        for child in &self.children {
            match child {
                Node::Element(element) => element.write(out, depth + 1),
                Node::Text(text) => {
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str(&escape(text));
                    out.push('\n');
                }
            }
        }
        out.push_str(&padding);
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(">\n");
    }
}

/// A whole document, declaration included.
#[must_use]
pub fn document(root: &Element) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    root.write(&mut out, 0);
    out
}

/// Escapes the five characters XML reserves.
#[must_use]
pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Parses a document and returns its root element.
pub fn parse(text: &str) -> Result<Element> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        position: 0,
    };
    reader.prologue()?;
    let root = reader.element()?;
    reader.prologue()?;
    if reader.position < reader.bytes.len() {
        return Err(Error::Parse(format!(
            "trailing content after the root element at byte {}",
            reader.position
        )));
    }
    Ok(root)
}

/// A cursor over the document text.
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    /// Skips whitespace, declarations, comments and processing instructions.
    fn prologue(&mut self) -> Result<()> {
        loop {
            self.whitespace();
            if self.starts_with(b"<?") {
                self.until(b"?>")?;
            } else if self.starts_with(b"<!--") {
                self.until(b"-->")?;
            } else if self.starts_with(b"<!") {
                self.until(b">")?;
            } else {
                return Ok(());
            }
        }
    }

    /// Whether the remaining text starts with a marker.
    fn starts_with(&self, marker: &[u8]) -> bool {
        self.bytes[self.position..].starts_with(marker)
    }

    /// Skips past the next occurrence of a marker.
    fn until(&mut self, marker: &[u8]) -> Result<()> {
        let rest = &self.bytes[self.position..];
        let offset = rest
            .windows(marker.len())
            .position(|window| window == marker)
            .ok_or_else(|| {
                Error::Parse(format!(
                    "unterminated `{}` at byte {}",
                    String::from_utf8_lossy(marker),
                    self.position
                ))
            })?;
        self.position += offset + marker.len();
        Ok(())
    }

    /// Skips whitespace.
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    /// Parses one element, including its children.
    fn element(&mut self) -> Result<Element> {
        if !self.starts_with(b"<") {
            return Err(Error::Parse(format!(
                "expected an element at byte {}",
                self.position
            )));
        }
        self.position += 1;
        let name = self.name()?;
        let mut element = Element::new(name);
        loop {
            self.whitespace();
            if self.starts_with(b"/>") {
                self.position += 2;
                return Ok(element);
            }
            if self.starts_with(b">") {
                self.position += 1;
                break;
            }
            let key = self.name()?;
            self.whitespace();
            if !self.starts_with(b"=") {
                return Err(Error::Parse(format!(
                    "attribute `{key}` has no value at byte {}",
                    self.position
                )));
            }
            self.position += 1;
            self.whitespace();
            let value = self.quoted()?;
            element.attributes.push((key, value));
        }
        self.content(&mut element)?;
        Ok(element)
    }

    /// Parses the children of an element up to its closing tag.
    fn content(&mut self, element: &mut Element) -> Result<()> {
        loop {
            if self.position >= self.bytes.len() {
                return Err(Error::Parse(format!("`{}` is never closed", element.name)));
            }
            if self.starts_with(b"</") {
                self.position += 2;
                let name = self.name()?;
                if name != element.name {
                    return Err(Error::Parse(format!(
                        "`{}` is closed by `{name}`",
                        element.name
                    )));
                }
                self.whitespace();
                if !self.starts_with(b">") {
                    return Err(Error::Parse(format!(
                        "the closing tag of `{name}` does not end at byte {}",
                        self.position
                    )));
                }
                self.position += 1;
                return Ok(());
            }
            if self.starts_with(b"<!--") {
                self.until(b"-->")?;
                continue;
            }
            if self.starts_with(b"<![CDATA[") {
                let start = self.position + 9;
                self.until(b"]]>")?;
                let text = &self.bytes[start..self.position - 3];
                element
                    .children
                    .push(Node::Text(String::from_utf8_lossy(text).into_owned()));
                continue;
            }
            if self.starts_with(b"<?") {
                self.until(b"?>")?;
                continue;
            }
            if self.starts_with(b"<") {
                let child = self.element()?;
                element.children.push(Node::Element(child));
                continue;
            }
            let start = self.position;
            while self.position < self.bytes.len() && self.bytes[self.position] != b'<' {
                self.position += 1;
            }
            let raw = std::str::from_utf8(&self.bytes[start..self.position])
                .map_err(|error| Error::Parse(error.to_string()))?;
            if !raw.trim().is_empty() {
                element.children.push(Node::Text(unescape(raw.trim())?));
            }
        }
    }

    /// Reads a tag or attribute name.
    fn name(&mut self) -> Result<String> {
        let start = self.position;
        while self.position < self.bytes.len() {
            let byte = self.bytes[self.position];
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':') {
                self.position += 1;
            } else {
                break;
            }
        }
        if start == self.position {
            return Err(Error::Parse(format!("expected a name at byte {start}")));
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned())
    }

    /// Reads a quoted attribute value.
    fn quoted(&mut self) -> Result<String> {
        let quote = *self
            .bytes
            .get(self.position)
            .filter(|byte| matches!(byte, b'"' | b'\''))
            .ok_or_else(|| Error::Parse(format!("expected a quote at byte {}", self.position)))?;
        self.position += 1;
        let start = self.position;
        while self.position < self.bytes.len() && self.bytes[self.position] != quote {
            self.position += 1;
        }
        if self.position >= self.bytes.len() {
            return Err(Error::Parse(format!(
                "unterminated attribute value at byte {start}"
            )));
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|error| Error::Parse(error.to_string()))?;
        self.position += 1;
        unescape(raw)
    }
}

/// Turns the entities [`escape`] writes, and numeric references, back into text.
pub fn unescape(value: &str) -> Result<String> {
    if !value.contains('&') {
        return Ok(value.to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let end = tail
            .find(';')
            .ok_or_else(|| Error::Parse(format!("unterminated entity in `{value}`")))?;
        let entity = &tail[1..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            numeric if numeric.starts_with('#') => out.push(numeric_entity(numeric, value)?),
            other => return Err(Error::Parse(format!("unknown entity `&{other};`"))),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Decodes `&#38;` and `&#x26;`.
fn numeric_entity(entity: &str, context: &str) -> Result<char> {
    let (digits, radix) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
        .map_or_else(|| (&entity[1..], 10), |hex| (hex, 16));
    let code = u32::from_str_radix(digits, radix)
        .map_err(|_| Error::Parse(format!("invalid entity `&{entity};` in `{context}`")))?;
    char::from_u32(code)
        .ok_or_else(|| Error::Parse(format!("invalid code point `&{entity};` in `{context}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_survives_the_round_trip() {
        let root = Element::new("session")
            .attribute("name", "a <song> & \"more\"")
            .attribute("rate", "44100")
            .child(
                Element::new("tracks").extend([
                    Element::new("track").attribute("id", "0"),
                    Element::new("track")
                        .attribute("id", "1")
                        .child(Element::leaf("name", "drums")),
                ]),
            )
            .child(Element::leaf("comment", "written by a test"));

        let text = document(&root);
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(text.contains("a &lt;song&gt; &amp; &quot;more&quot;"));

        let parsed = parse(&text).unwrap();
        assert_eq!(parsed, root);
    }

    #[test]
    fn elements_can_be_looked_up() {
        let root =
            parse("<a><b id=\"1\"/><b id=\"2\"><c>text</c></b><!-- note --><d/></a>").unwrap();

        assert_eq!(root.find("b").unwrap().get("id"), Some("1"));
        assert_eq!(root.find_all("b").count(), 2);
        assert_eq!(root.elements().count(), 3);
        assert_eq!(
            root.find_all("b")
                .nth(1)
                .unwrap()
                .find("c")
                .unwrap()
                .content(),
            "text"
        );
        assert!(root.find("z").is_none());
        assert!(root.get("missing").is_none());
    }

    #[test]
    fn optional_attributes_are_only_written_when_present() {
        let element = Element::new("note")
            .maybe("pitch", Some("60"))
            .maybe("velocity", None::<String>);

        assert_eq!(element.attributes.len(), 1);
        assert_eq!(element.get("pitch"), Some("60"));
    }

    #[test]
    fn declarations_comments_and_cdata_are_understood() {
        let text = "<?xml version=\"1.0\"?>\n<!DOCTYPE score>\n<!-- a comment -->\n\
             <score><text><![CDATA[a & b]]></text></score>\n<!-- trailing -->";

        let root = parse(text).unwrap();

        assert_eq!(root.name, "score");
        assert_eq!(root.find("text").unwrap().content(), "a & b");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(unescape("&amp;&lt;&gt;&quot;&apos;").unwrap(), "&<>\"'");
        assert_eq!(unescape("&#65;&#x42;&#X43;").unwrap(), "ABC");
        assert_eq!(unescape("plain").unwrap(), "plain");
        assert!(unescape("&nbsp;").is_err());
        assert!(unescape("&amp").is_err());
        assert!(unescape("&#zz;").is_err());
        assert!(unescape("&#xD800;").is_err());
    }

    #[test]
    fn broken_documents_are_reported_with_a_position() {
        let complain = |text: &str| parse(text).unwrap_err().to_string();

        assert!(complain("").contains("expected an element"));
        assert!(complain("<a>").contains("never closed"));
        assert!(complain("<a></b>").contains("closed by"));
        assert!(complain("<a attr></a>").contains("no value"));
        assert!(complain("<a attr=x></a>").contains("expected a quote"));
        assert!(complain("<a attr=\"x></a>").contains("unterminated attribute"));
        assert!(complain("<a/><b/>").contains("trailing content"));
        assert!(complain("<!-- open <a/>").contains("unterminated"));
        assert!(complain("<a></a >x").contains("trailing content"));
        assert!(complain("<>").contains("expected a name"));
    }

    #[test]
    fn single_quotes_and_odd_names_are_accepted() {
        let root = parse("<ns:tag data-value='1' xml.thing='2'/>").unwrap();

        assert_eq!(root.name, "ns:tag");
        assert_eq!(root.get("data-value"), Some("1"));
        assert_eq!(root.get("xml.thing"), Some("2"));
    }
}
