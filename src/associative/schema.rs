//! The schema a decomposition is written in.
//!
//! A decomposition is two things: a structure (what was found, where it goes,
//! how loud) and a pile of waveforms. The structure is written as a Links
//! Notation manifest, which is stored in the link network and read back from
//! it without loss; the waveforms are written next to the manifest as audio
//! files, and the manifest names them. Keeping the two apart is what makes the
//! archive both readable and exact: text stays text, and samples stay bits.
//!
//! ```text
//! (decomposition: version 1)
//! (source: (name song) (sample-rate 44100) (format pcm_i16) (channels 2) (frames 220500))
//! (stem: harmonic stems/harmonic.wav)
//! (sample: 1 sample-0001-A3 samples/sample-0001-A3.wav (frames 4096) (peak 0.5) (note 57))
//! (placement: 1 0 0 65536)
//! (note: 0 0 4096 57 100 220.0 0.93)
//! (residual: residual.wav)
//! ```
//!
//! Every text value is percent-encoded, so a name with a space or a bracket in
//! it survives the round trip through Links Notation, where those characters
//! are syntax.

use links_notation::LiNo;

use crate::associative::lino::{self, Document};
use crate::associative::store::{LinkIndex, LinkStore};
use crate::audio::SampleFormat;
use crate::decompose::model::{Decomposition, Gain, Note, Placement, SourceInfo};
use crate::error::{Error, Result};

/// Version tag written into every manifest.
pub const VERSION: &str = "1";

/// File the residual is stored in.
pub const RESIDUAL_FILE: &str = "residual.wav";

/// File the correction is stored in, when there is one.
pub const CORRECTION_FILE: &str = "correction.wav";

/// File name of a stem's audio.
#[must_use]
pub fn stem_file(name: &str) -> String {
    format!("stems/{}.wav", slug(name))
}

/// File name of a bank sample's waveform.
#[must_use]
pub fn sample_file(name: &str) -> String {
    format!("samples/{}.wav", slug(name))
}

/// A named audio file belonging to a decomposition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Name of the stem.
    pub name: String,
    /// File the audio lives in, relative to the manifest.
    pub file: String,
}

/// Everything a manifest records about one bank sample.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleEntry {
    /// Index into the bank.
    pub id: usize,
    /// Name of the sample.
    pub name: String,
    /// File the waveform lives in, relative to the manifest.
    pub file: String,
    /// Number of frames in the waveform.
    pub frames: usize,
    /// Peak amplitude of the waveform.
    pub peak: f64,
    /// Recognised MIDI note, when the sample is pitched.
    pub note: Option<u8>,
    /// Recognised fundamental frequency, when the sample is pitched.
    pub frequency: Option<f64>,
}

/// A decomposition without its audio: what the manifest holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Where the recording came from.
    pub source: SourceInfo,
    /// The stems and the files holding them.
    pub stems: Vec<Entry>,
    /// The bank.
    pub samples: Vec<SampleEntry>,
    /// Where the bank samples are used.
    pub placements: Vec<Placement>,
    /// The recognised notes.
    pub notes: Vec<Note>,
    /// File holding the residual.
    pub residual: String,
    /// File holding the correction, when the source needed one.
    pub correction: Option<String>,
}

impl Manifest {
    /// The manifest of a decomposition, with the conventional file names.
    #[must_use]
    pub fn of(decomposition: &Decomposition) -> Self {
        Self {
            source: decomposition.source.clone(),
            stems: decomposition
                .stems
                .iter()
                .map(|stem| Entry {
                    name: stem.name.clone(),
                    file: stem_file(&stem.name),
                })
                .collect(),
            samples: decomposition
                .samples
                .iter()
                .map(|sample| SampleEntry {
                    id: sample.id,
                    name: sample.name.clone(),
                    file: sample_file(&sample.name),
                    frames: sample.len(),
                    peak: sample.peak,
                    note: sample.note,
                    frequency: sample.frequency,
                })
                .collect(),
            placements: decomposition.placements.clone(),
            notes: decomposition.notes.clone(),
            residual: RESIDUAL_FILE.to_string(),
            correction: decomposition
                .correction
                .as_ref()
                .map(|_| CORRECTION_FILE.to_string()),
        }
    }

    /// The manifest as a Links Notation document.
    #[must_use]
    pub fn to_document(&self) -> Document {
        let mut document = vec![lino::link(
            "decomposition",
            vec![text("version"), text(VERSION)],
        )];
        document.push(lino::link(
            "source",
            vec![
                pair("name", &encode(&self.source.name)),
                pair("sample-rate", &self.source.sample_rate.to_string()),
                pair("format", self.source.format.name()),
                pair("channels", &self.source.channels.to_string()),
                pair("frames", &self.source.frames.to_string()),
            ],
        ));
        for stem in &self.stems {
            document.push(lino::link(
                "stem",
                vec![text(&encode(&stem.name)), text(&encode(&stem.file))],
            ));
        }
        for sample in &self.samples {
            let mut values = vec![
                text(&sample.id.to_string()),
                text(&encode(&sample.name)),
                text(&encode(&sample.file)),
                pair("frames", &sample.frames.to_string()),
                pair("peak", &number(sample.peak)),
            ];
            if let Some(note) = sample.note {
                values.push(pair("note", &note.to_string()));
            }
            if let Some(frequency) = sample.frequency {
                values.push(pair("frequency", &number(frequency)));
            }
            document.push(lino::link("sample", values));
        }
        for placement in &self.placements {
            document.push(lino::link(
                "placement",
                vec![
                    text(&placement.sample.to_string()),
                    text(&placement.channel.to_string()),
                    text(&placement.start.to_string()),
                    text(&placement.gain.numerator().to_string()),
                ],
            ));
        }
        for note in &self.notes {
            document.push(lino::link(
                "note",
                vec![
                    text(&note.channel.to_string()),
                    text(&note.start.to_string()),
                    text(&note.length.to_string()),
                    text(&note.note.to_string()),
                    text(&note.velocity.to_string()),
                    text(&number(note.frequency)),
                    text(&number(note.confidence)),
                ],
            ));
        }
        document.push(lino::link("residual", vec![text(&encode(&self.residual))]));
        if let Some(correction) = &self.correction {
            document.push(lino::link("correction", vec![text(&encode(correction))]));
        }
        document
    }

    /// Reads a manifest back out of a Links Notation document.
    pub fn from_document(document: &Document) -> Result<Self> {
        let mut version = None;
        let mut source = None;
        let mut stems = Vec::new();
        let mut samples = Vec::new();
        let mut placements = Vec::new();
        let mut notes = Vec::new();
        let mut residual = RESIDUAL_FILE.to_string();
        let mut correction = None;

        for node in document {
            let (id, values) = entry(node)?;
            match id {
                "decomposition" => version = Some(read_version(values)?),
                "source" => source = Some(read_source(values)?),
                "stem" => stems.push(Entry {
                    name: decode(field(values, 0, "a stem name")?)?,
                    file: decode(field(values, 1, "a stem file")?)?,
                }),
                "sample" => samples.push(read_sample(values)?),
                "placement" => placements.push(read_placement(values)?),
                "note" => notes.push(read_note(values)?),
                "residual" => residual = decode(field(values, 0, "the residual file")?)?,
                "correction" => {
                    correction = Some(decode(field(values, 0, "the correction file")?)?);
                }
                other => return Err(Error::Format(format!("unknown manifest entry `{other}`"))),
            }
        }

        match version.as_deref() {
            Some(VERSION) => {}
            Some(other) => {
                return Err(Error::Format(format!(
                    "manifest version `{other}` is not supported, expected `{VERSION}`"
                )))
            }
            None => return Err(Error::Format("manifest has no version".to_string())),
        }
        Ok(Self {
            source: source.ok_or_else(|| Error::Format("manifest has no source".to_string()))?,
            stems,
            samples,
            placements,
            notes,
            residual,
            correction,
        })
    }

    /// The manifest as Links Notation text.
    #[must_use]
    pub fn to_lino(&self) -> String {
        lino::format(&self.to_document())
    }

    /// Reads a manifest from Links Notation text.
    pub fn from_lino(text: &str) -> Result<Self> {
        Self::from_document(&lino::parse(text)?)
    }

    /// Stores the manifest in a link network, returning the document root.
    pub fn store(&self, store: &mut LinkStore) -> LinkIndex {
        lino::store_document(store, &self.to_document())
    }

    /// Reads a manifest back out of a link network.
    pub fn read(store: &LinkStore, root: LinkIndex) -> Result<Self> {
        Self::from_document(&lino::read_document(store, root)?)
    }
}

/// A value node.
fn text(value: &str) -> LiNo<String> {
    lino::reference(value)
}

/// A `(key value)` node.
fn pair(key: &str, value: &str) -> LiNo<String> {
    lino::property(key, value)
}

/// Formats a number so that reading it back yields the same `f64`.
fn number(value: f64) -> String {
    format!("{value:?}")
}

/// The identifier and values of a top-level link.
fn entry(node: &LiNo<String>) -> Result<(&str, &[LiNo<String>])> {
    match node {
        LiNo::Link {
            id: Some(id),
            values,
        } => Ok((id.as_str(), values.as_slice())),
        other => Err(Error::Format(format!(
            "expected a named manifest entry, found `{other}`"
        ))),
    }
}

/// The nth value of an entry, as text.
fn field<'a>(values: &'a [LiNo<String>], index: usize, what: &str) -> Result<&'a str> {
    match values.get(index) {
        Some(LiNo::Ref(value)) => Ok(value.as_str()),
        Some(other) => Err(Error::Format(format!("expected {what}, found `{other}`"))),
        None => Err(Error::Format(format!("missing {what}"))),
    }
}

/// The value of a `(key value)` node inside an entry.
fn lookup<'a>(values: &'a [LiNo<String>], key: &str) -> Option<&'a str> {
    values.iter().find_map(|node| match node {
        LiNo::Link { id: None, values } => match (values.first(), values.get(1)) {
            (Some(LiNo::Ref(name)), Some(LiNo::Ref(value))) if name == key => Some(value.as_str()),
            _ => None,
        },
        _ => None,
    })
}

/// A required `(key value)` node.
fn required<'a>(values: &'a [LiNo<String>], key: &str) -> Result<&'a str> {
    lookup(values, key).ok_or_else(|| Error::Format(format!("missing `{key}`")))
}

/// Parses an integer field.
fn integer<T: std::str::FromStr>(value: &str, what: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| Error::Format(format!("`{value}` is not a valid {what}")))
}

/// Parses a floating-point field.
fn decimal(value: &str, what: &str) -> Result<f64> {
    value
        .parse()
        .map_err(|_| Error::Format(format!("`{value}` is not a valid {what}")))
}

fn read_version(values: &[LiNo<String>]) -> Result<String> {
    let key = field(values, 0, "the version key")?;
    if key != "version" {
        return Err(Error::Format(format!("expected `version`, found `{key}`")));
    }
    Ok(field(values, 1, "the version")?.to_string())
}

fn read_source(values: &[LiNo<String>]) -> Result<SourceInfo> {
    Ok(SourceInfo {
        name: decode(required(values, "name")?)?,
        sample_rate: integer(required(values, "sample-rate")?, "sample rate")?,
        format: SampleFormat::parse(required(values, "format")?)?,
        channels: integer(required(values, "channels")?, "channel count")?,
        frames: integer(required(values, "frames")?, "frame count")?,
    })
}

fn read_sample(values: &[LiNo<String>]) -> Result<SampleEntry> {
    Ok(SampleEntry {
        id: integer(field(values, 0, "a sample id")?, "sample id")?,
        name: decode(field(values, 1, "a sample name")?)?,
        file: decode(field(values, 2, "a sample file")?)?,
        frames: integer(required(values, "frames")?, "frame count")?,
        peak: decimal(required(values, "peak")?, "peak")?,
        note: lookup(values, "note")
            .map(|value| integer(value, "note number"))
            .transpose()?,
        frequency: lookup(values, "frequency")
            .map(|value| decimal(value, "frequency"))
            .transpose()?,
    })
}

fn read_placement(values: &[LiNo<String>]) -> Result<Placement> {
    Ok(Placement::new(
        integer(field(values, 0, "a placement sample")?, "sample id")?,
        integer(field(values, 1, "a placement channel")?, "channel")?,
        integer(field(values, 2, "a placement start")?, "start frame")?,
        Gain::from_numerator(integer(field(values, 3, "a placement gain")?, "gain")?),
    ))
}

fn read_note(values: &[LiNo<String>]) -> Result<Note> {
    Ok(Note {
        channel: integer(field(values, 0, "a note channel")?, "channel")?,
        start: integer(field(values, 1, "a note start")?, "start frame")?,
        length: integer(field(values, 2, "a note length")?, "length")?,
        note: integer(field(values, 3, "a note number")?, "note number")?,
        velocity: integer(field(values, 4, "a note velocity")?, "velocity")?,
        frequency: decimal(field(values, 5, "a note frequency")?, "frequency")?,
        confidence: decimal(field(values, 6, "a note confidence")?, "confidence")?,
    })
}

/// Whether a byte may appear in a manifest value unencoded.
const fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b'#')
}

/// Percent-encodes a value so that Links Notation syntax cannot appear in it.
#[must_use]
pub fn encode(value: &str) -> String {
    if value.is_empty() {
        return "%".to_string();
    }
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_plain(byte) {
            encoded.push(char::from(byte));
        } else {
            const HEX: [u8; 16] = *b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0F) as usize]));
        }
    }
    encoded
}

/// Decodes a value written by [`encode`].
pub fn decode(value: &str) -> Result<String> {
    if value == "%" {
        return Ok(String::new());
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = value
                .get(index + 1..index + 3)
                .ok_or_else(|| Error::Format(format!("truncated escape in `{value}`")))?;
            let byte = u8::from_str_radix(digits, 16)
                .map_err(|_| Error::Format(format!("invalid escape `%{digits}` in `{value}`")))?;
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|error| Error::Format(format!("{error} in `{value}`")))
}

/// A file-name-safe form of a name, used for the audio files.
#[must_use]
pub fn slug(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if slug.is_empty() {
        slug.push('_');
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Audio;
    use crate::decompose::model::{Sample, Stem};

    fn manifest() -> Manifest {
        Manifest {
            source: SourceInfo {
                name: "prelude in c".to_string(),
                sample_rate: 44_100,
                format: SampleFormat::PcmI16,
                channels: 2,
                frames: 220_500,
            },
            stems: vec![
                Entry {
                    name: "harmonic".to_string(),
                    file: "stems/harmonic.wav".to_string(),
                },
                Entry {
                    name: "percussive".to_string(),
                    file: "stems/percussive.wav".to_string(),
                },
            ],
            samples: vec![
                SampleEntry {
                    id: 0,
                    name: "sample-0001".to_string(),
                    file: "samples/sample-0001.wav".to_string(),
                    frames: 4096,
                    peak: 0.6,
                    note: Some(57),
                    frequency: Some(220.0),
                },
                SampleEntry {
                    id: 1,
                    name: "sample-0002".to_string(),
                    file: "samples/sample-0002.wav".to_string(),
                    frames: 1024,
                    peak: 0.125,
                    note: None,
                    frequency: None,
                },
            ],
            placements: vec![
                Placement::new(0, 0, 0, Gain::UNIT),
                Placement::new(1, 1, -12, Gain::from_numerator(32_768)),
            ],
            notes: vec![Note {
                channel: 0,
                start: 0,
                length: 4096,
                note: 57,
                velocity: 100,
                frequency: 220.0,
                confidence: 0.93,
            }],
            residual: RESIDUAL_FILE.to_string(),
            correction: Some(CORRECTION_FILE.to_string()),
        }
    }

    #[test]
    fn a_manifest_survives_the_round_trip_through_links_notation() {
        let original = manifest();
        let text = original.to_lino();
        assert_eq!(Manifest::from_lino(&text).unwrap(), original);
        assert_eq!(Manifest::from_lino(&text).unwrap().to_lino(), text);
    }

    #[test]
    fn a_manifest_survives_the_round_trip_through_the_link_network() {
        let original = manifest();
        let mut store = LinkStore::default();
        let root = original.store(&mut store);
        assert_eq!(Manifest::read(&store, root).unwrap(), original);
    }

    #[test]
    fn a_manifest_without_optional_parts_round_trips() {
        let mut original = manifest();
        original.correction = None;
        original.stems.clear();
        original.notes.clear();
        original.samples[0].note = None;
        original.samples[0].frequency = None;

        let text = original.to_lino();
        assert!(!text.contains("correction"));
        assert!(!text.contains("(note"));
        assert_eq!(Manifest::from_lino(&text).unwrap(), original);
    }

    #[test]
    fn the_manifest_of_a_decomposition_names_every_file() {
        let audio = Audio::silence(8000, SampleFormat::PcmI16, 1, 16);
        let decomposition = Decomposition {
            source: SourceInfo::of("my song", &audio),
            stems: vec![Stem {
                name: "one two".to_string(),
                audio: audio.clone(),
            }],
            samples: vec![Sample::new(0, "a/b".to_string(), vec![0.5, -0.5])],
            placements: vec![Placement::new(0, 0, 4, Gain::UNIT)],
            notes: Vec::new(),
            residual: audio.clone(),
            correction: Some(audio),
        };

        let manifest = Manifest::of(&decomposition);
        assert_eq!(manifest.source.name, "my song");
        assert_eq!(manifest.stems[0].file, "stems/one_two.wav");
        assert_eq!(manifest.samples[0].file, "samples/a_b.wav");
        assert_eq!(manifest.samples[0].frames, 2);
        assert_eq!(manifest.samples[0].peak, 0.5);
        assert_eq!(manifest.residual, RESIDUAL_FILE);
        assert_eq!(manifest.correction.as_deref(), Some(CORRECTION_FILE));
        assert_eq!(Manifest::from_lino(&manifest.to_lino()).unwrap(), manifest);
    }

    #[test]
    fn names_that_collide_with_the_notation_are_encoded() {
        let awkward = "a (b) c: d\t\u{e9}%";
        assert_eq!(encode(awkward), "a%20%28b%29%20c%3A%20d%09%C3%A9%25");
        assert_eq!(decode(&encode(awkward)).unwrap(), awkward);
        assert_eq!(encode(""), "%");
        assert_eq!(decode("%").unwrap(), "");
        assert_eq!(encode("stems/a-b_c.wav"), "stems/a-b_c.wav");

        let mut original = manifest();
        original.source.name = awkward.to_string();
        original.stems[0].name = "left right".to_string();
        assert_eq!(Manifest::from_lino(&original.to_lino()).unwrap(), original);
    }

    #[test]
    fn awkward_decodes_are_reported_rather_than_guessed() {
        assert!(decode("%2").is_err());
        assert!(decode("%zz").is_err());
        assert!(decode("%FF").is_err());
        assert_eq!(slug(""), "_");
        assert_eq!(slug("Kick 01!"), "Kick_01_");
    }

    #[test]
    fn exact_values_are_preserved_to_the_last_bit() {
        let mut original = manifest();
        original.samples[0].peak = 0.1 + 0.2;
        original.samples[0].frequency = Some(std::f64::consts::PI);
        original.notes[0].confidence = 1.0 / 3.0;

        let restored = Manifest::from_lino(&original.to_lino()).unwrap();
        assert_eq!(
            restored.samples[0].peak.to_bits(),
            (0.1_f64 + 0.2).to_bits()
        );
        assert_eq!(restored, original);
    }

    #[test]
    fn broken_manifests_are_rejected_with_a_reason() {
        let complain = |text: &str| Manifest::from_lino(text).unwrap_err().to_string();

        assert!(complain("(residual: r.wav)").contains("no version"));
        assert!(complain("(decomposition: version 2)").contains("not supported"));
        assert!(complain("(decomposition: version 1)").contains("no source"));
        assert!(complain("(decomposition: revision 1)").contains("expected `version`"));
        assert!(complain("(mystery: 1)").contains("unknown manifest entry"));
        assert!(
            complain("(decomposition: version 1)\n(source: (name a) (sample-rate 44100))")
                .contains("missing `format`")
        );
        assert!(complain(
            "(decomposition: version 1)\n(source: (name a) (sample-rate x) (format pcm_i16) (channels 1) (frames 1))"
        )
        .contains("not a valid sample rate"));
        assert!(
            complain("(decomposition: version 1)\n(stem: only)").contains("missing a stem file")
        );
        assert!(complain("(decomposition: version 1)\n(placement: 0 0 0 x)")
            .contains("not a valid gain"));
        assert!(
            complain("(decomposition: version 1)\n(note: 0 0 0 0 0 0.0 x)")
                .contains("not a valid confidence")
        );
        assert!(
            complain("(decomposition: version 1)\n(sample: 0 a b (frames 1))")
                .contains("missing `peak`")
        );
    }
}
