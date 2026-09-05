//! REAPER projects, the plain-text `.rpp` format.
//!
//! An `.rpp` file is a tree of tokens: a node opens with `<NAME` and closes
//! with `>`, and everything between is either a line of values or another
//! node. That makes it the easiest project format to write and, more
//! importantly, the easiest to read back, so a test can open what it wrote.
//!
//! Audio clips become items pointing at the files the archive holds. Notes
//! become an item with an inline MIDI source, written the way REAPER writes
//! them: `HASDATA` says how many ticks a quarter note has, and every `E` line
//! is a MIDI message a number of ticks after the one before it.

use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::session::{Clip, Session, Track, TrackKind};

/// Ticks per quarter note in the MIDI items that are written.
pub const TICKS: u32 = 960;

/// One node of a project file.
///
/// A line such as `TEMPO 120 4 4` and a block such as `<TRACK … >` are the
/// same thing with and without children; [`Node::block`] says which was
/// written, so a file read and written again keeps its shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Name, the first token of the line.
    pub name: String,
    /// The tokens after the name.
    pub values: Vec<String>,
    /// Nodes inside this one.
    pub children: Vec<Self>,
    /// Whether this was written as `<NAME … >`.
    pub block: bool,
}

impl Node {
    /// A line of values.
    #[must_use]
    pub fn line(name: impl Into<String>, values: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.into(),
            values: values.into_iter().collect(),
            children: Vec::new(),
            block: false,
        }
    }

    /// A block that holds other nodes.
    #[must_use]
    pub fn block(name: impl Into<String>, values: impl IntoIterator<Item = String>) -> Self {
        Self {
            block: true,
            ..Self::line(name, values)
        }
    }

    /// Adds a child.
    #[must_use]
    pub fn child(mut self, child: Self) -> Self {
        self.children.push(child);
        self
    }

    /// The first child with a name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Self> {
        self.children.iter().find(|child| child.name == name)
    }

    /// Every child with a name.
    pub fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The value at a position of a child line.
    #[must_use]
    pub fn value(&self, name: &str, index: usize) -> Option<&str> {
        self.find(name)
            .and_then(|child| child.values.get(index))
            .map(String::as_str)
    }

    /// Writes the node and everything under it.
    pub fn write(&self, out: &mut String, depth: usize) {
        let padding = "  ".repeat(depth);
        out.push_str(&padding);
        if self.block {
            out.push('<');
        }
        out.push_str(&self.name);
        for value in &self.values {
            out.push(' ');
            out.push_str(&quote(value));
        }
        out.push('\n');
        if !self.block {
            return;
        }
        for child in &self.children {
            child.write(out, depth + 1);
        }
        out.push_str(&padding);
        out.push_str(">\n");
    }
}

/// Quotes a value the way the format does, when it has to.
fn quote(value: &str) -> String {
    if !value.is_empty() && !value.contains([' ', '"', '\'', '`']) {
        return value.to_string();
    }
    // The format has no escapes: it picks a quote the value does not contain.
    for quote in ['"', '\'', '`'] {
        if !value.contains(quote) {
            return format!("{quote}{value}{quote}");
        }
    }
    format!("\"{}\"", value.replace('"', "'"))
}

/// Writes a session as a REAPER project.
#[must_use]
pub fn write(session: &Session) -> String {
    let mut out = String::new();
    project(session).write(&mut out, 0);
    out
}

/// Writes a session as a REAPER project file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, write(session))?;
    Ok(())
}

/// Builds the project tree.
#[must_use]
pub fn project(session: &Session) -> Node {
    let mut root = Node::block(
        "REAPER_PROJECT",
        [
            "0.1".to_string(),
            format!("7.0/audio-decomposer-{}", env!("CARGO_PKG_VERSION")),
            "0".to_string(),
        ],
    )
    .child(Node::line("RIPPLE", ["0".to_string()]))
    .child(Node::line("SAMPLERATE", [session.sample_rate.to_string()]))
    .child(Node::line(
        "TEMPO",
        [
            format!("{:.6}", session.tempo),
            "4".to_string(),
            "4".to_string(),
        ],
    ))
    .child(Node::line("TITLE", [session.name.clone()]));
    for track in &session.tracks {
        root = root.child(track_node(track, session));
    }
    root
}

/// One track and its items.
fn track_node(track: &Track, session: &Session) -> Node {
    let mut node = Node::block("TRACK", [])
        .child(Node::line("NAME", [track.name.clone()]))
        .child(Node::line(
            "VOLPAN",
            ["1".to_string(), "0".to_string(), "-1".to_string()],
        ))
        .child(Node::line(
            "MUTESOLO",
            ["0".to_string(), "0".to_string(), "0".to_string()],
        ));
    for clip in &track.clips {
        node = node.child(audio_item(clip, session));
    }
    if !track.notes.is_empty() {
        node = node.child(note_item(&track.notes, session));
    }
    node
}

/// One audio item.
fn audio_item(clip: &Clip, session: &Session) -> Node {
    Node::block("ITEM", [])
        .child(Node::line(
            "POSITION",
            [format!("{:.9}", session.seconds(clip.start))],
        ))
        .child(Node::line(
            "LENGTH",
            [format!("{:.9}", session.seconds(clip.length))],
        ))
        .child(Node::line(
            "SOFFS",
            [format!("{:.9}", session.seconds(clip.offset))],
        ))
        .child(Node::line(
            "VOLPAN",
            [
                format!("{:.9}", clip.gain),
                "0".to_string(),
                "1".to_string(),
                "-1".to_string(),
            ],
        ))
        .child(Node::line("NAME", [clip.name.clone()]))
        .child(
            Node::block("SOURCE", ["WAVE".to_string()])
                .child(Node::line("FILE", [clip.file.clone()])),
        )
}

/// The notes of a track, as one item with an inline MIDI source.
fn note_item(notes: &[Note], session: &Session) -> Node {
    let start = notes.iter().map(|note| note.start).min().unwrap_or(0);
    let end = notes.iter().map(Note::end).max().unwrap_or(0);
    let mut source = Node::block("SOURCE", ["MIDI".to_string()]).child(Node::line(
        "HASDATA",
        ["1".to_string(), TICKS.to_string(), "QN".to_string()],
    ));

    let mut events: Vec<(u64, u8, u8, u8)> = Vec::new();
    for note in notes {
        let from = ticks(session, note.start - start.min(note.start));
        let to = ticks(session, note.start + note.length - start.min(note.start));
        events.push((
            from,
            0x90 | (note.channel as u8 & 0x0f),
            note.note,
            note.velocity.max(1),
        ));
        events.push((to, 0x80 | (note.channel as u8 & 0x0f), note.note, 0));
    }
    // Note offs come before note ons at the same tick, so a repeated note
    // stops before it starts again.
    events.sort_by_key(|(at, status, note, _)| (*at, *status & 0xf0, *note));

    let mut previous = 0;
    for (at, status, data1, data2) in events {
        source = source.child(Node::line(
            "E",
            [
                (at - previous).to_string(),
                format!("{status:02x}"),
                format!("{data1:02x}"),
                format!("{data2:02x}"),
            ],
        ));
        previous = at;
    }

    Node::block("ITEM", [])
        .child(Node::line(
            "POSITION",
            [format!("{:.9}", session.seconds(start))],
        ))
        .child(Node::line(
            "LENGTH",
            [format!("{:.9}", session.seconds(end - start))],
        ))
        .child(Node::line("NAME", ["notes".to_string()]))
        .child(source)
}

/// Ticks a number of frames lasts.
fn ticks(session: &Session, frames: usize) -> u64 {
    let beats = session.beats(frames);
    (beats * f64::from(TICKS)).round().max(0.0) as u64
}

/// Reads a project file back into a session.
pub fn read_file(path: impl AsRef<Path>) -> Result<Session> {
    parse(&fs::read_to_string(path)?)
}

/// Reads a project back into a session.
pub fn parse(text: &str) -> Result<Session> {
    let root = tree(text)?;
    if root.name != "REAPER_PROJECT" {
        return Err(Error::Format(format!(
            "`{}` is not a REAPER project",
            root.name
        )));
    }
    let sample_rate = root
        .value("SAMPLERATE", 0)
        .ok_or_else(|| Error::Format("the project has no sample rate".to_string()))?
        .parse()
        .map_err(|_| Error::Parse("the sample rate is not a number".to_string()))?;
    let tempo = root
        .value("TEMPO", 0)
        .map_or(Ok(crate::formats::session::DEFAULT_TEMPO), |value| {
            number(value, "tempo")
        })?;

    let mut session = Session {
        name: root.value("TITLE", 0).unwrap_or_default().to_string(),
        sample_rate,
        tempo,
        frames: 0,
        channels: 1,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };

    for node in root.find_all("TRACK") {
        let mut track = Track {
            name: node.value("NAME", 0).unwrap_or_default().to_string(),
            kind: TrackKind::Audio,
            clips: Vec::new(),
            notes: Vec::new(),
        };
        for item in node.find_all("ITEM") {
            read_item(item, &session, &mut track)?;
        }
        if !track.notes.is_empty() {
            track.kind = TrackKind::Instrument;
        }
        session.tracks.push(track);
    }
    session.frames = session.end();
    Ok(session)
}

/// Reads one item onto a track.
fn read_item(item: &Node, session: &Session, track: &mut Track) -> Result<()> {
    let position = frames_of(
        session,
        number(item.value("POSITION", 0).unwrap_or("0"), "position")?,
    );
    let length = frames_of(
        session,
        number(item.value("LENGTH", 0).unwrap_or("0"), "length")?,
    );
    let source = item
        .find("SOURCE")
        .ok_or_else(|| Error::Format("an item has no source".to_string()))?;
    if source.values.first().map(String::as_str) == Some("MIDI") {
        return read_midi(source, session, position, track);
    }
    let file = source
        .value("FILE", 0)
        .ok_or_else(|| Error::Format("an audio item has no file".to_string()))?;
    track.clips.push(Clip {
        name: item.value("NAME", 0).unwrap_or_default().to_string(),
        file: file.to_string(),
        start: position,
        length,
        offset: frames_of(
            session,
            number(item.value("SOFFS", 0).unwrap_or("0"), "offset")?,
        ),
        gain: number(item.value("VOLPAN", 0).unwrap_or("1"), "gain")?,
        channel: 0,
    });
    Ok(())
}

/// Reads an inline MIDI source onto a track.
fn read_midi(source: &Node, session: &Session, position: usize, track: &mut Track) -> Result<()> {
    let division: f64 = source
        .value("HASDATA", 1)
        .unwrap_or("960")
        .parse()
        .map_err(|_| Error::Parse("the MIDI division is not a number".to_string()))?;
    if division <= 0.0 {
        return Err(Error::Format("the MIDI source has no division".to_string()));
    }
    let mut at = 0u64;
    let mut open: Vec<(u8, u8, u64, u8)> = Vec::new();
    for event in source.find_all("E") {
        let delta: u64 = event
            .values
            .first()
            .map_or("0", String::as_str)
            .parse()
            .map_err(|_| Error::Parse("a MIDI event has no delta".to_string()))?;
        at += delta;
        let byte = |index: usize| -> Result<u8> {
            u8::from_str_radix(event.values.get(index).map_or("0", String::as_str), 16)
                .map_err(|_| Error::Parse("a MIDI event has a byte that is not hex".to_string()))
        };
        let (status, data1, data2) = (byte(1)?, byte(2)?, byte(3)?);
        let channel = usize::from(status & 0x0f);
        match status & 0xf0 {
            0x90 if data2 > 0 => open.push((status & 0x0f, data1, at, data2)),
            0x80 | 0x90 => {
                if let Some(index) = open
                    .iter()
                    .position(|(chan, note, _, _)| *chan == status & 0x0f && *note == data1)
                {
                    let (_, note, from, velocity) = open.remove(index);
                    let seconds = |ticks: u64| ticks as f64 / division * 60.0 / session.tempo;
                    let start = position + frames_of(session, seconds(from));
                    track.notes.push(Note {
                        channel,
                        start,
                        length: position + frames_of(session, seconds(at)) - start,
                        note,
                        velocity,
                        frequency: 440.0 * ((f64::from(note) - 69.0) / 12.0).exp2(),
                        confidence: 1.0,
                    });
                }
            }
            _ => {}
        }
    }
    track.notes.sort_by_key(|note| (note.start, note.note));
    Ok(())
}

/// Frames a number of seconds lasts.
fn frames_of(session: &Session, seconds: f64) -> usize {
    let frames = (seconds * f64::from(session.sample_rate)).round();
    if frames <= 0.0 {
        0
    } else {
        frames as usize
    }
}

/// Parses a number that the format writes as text.
fn number(text: &str, what: &str) -> Result<f64> {
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("`{text}` is not a {what}")))
}

/// Reads a project file into a tree.
pub fn tree(text: &str) -> Result<Node> {
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == ">" {
            let node = stack
                .pop()
                .ok_or_else(|| Error::Format(format!("line {} closes nothing", index + 1)))?;
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
            continue;
        }
        let block = line.starts_with('<');
        let mut values = tokens(line.trim_start_matches('<'));
        if values.is_empty() {
            continue;
        }
        let name = values.remove(0);
        let node = Node {
            name,
            values,
            children: Vec::new(),
            block,
        };
        if block {
            stack.push(node);
        } else if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else {
            return Err(Error::Format(format!(
                "line {} sits outside any node",
                index + 1
            )));
        }
    }
    if !stack.is_empty() {
        return Err(Error::Format("the project ends inside a node".to_string()));
    }
    root.ok_or_else(|| Error::Format("the project is empty".to_string()))
}

/// Splits a line into tokens, honouring the three quote characters.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_whitespace() {
            continue;
        }
        let mut token = String::new();
        if matches!(character, '"' | '\'' | '`') {
            for next in chars.by_ref() {
                if next == character {
                    break;
                }
                token.push(next);
            }
        } else {
            token.push(character);
            while let Some(next) = chars.peek() {
                if next.is_whitespace() {
                    break;
                }
                token.push(*next);
                chars.next();
            }
        }
        out.push(token);
    }
    out
}

/// Renders a tree back to text, which is how a read project is written again.
#[must_use]
pub fn render(root: &Node) -> String {
    let mut out = String::new();
    root.write(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::session::Instrument;

    /// One beat at 120 bpm and 48 kHz.
    const BEAT: usize = 24_000;

    fn clip(name: &str, file: &str, start: usize, length: usize, gain: f64) -> Clip {
        Clip {
            name: name.to_string(),
            file: file.to_string(),
            start,
            length,
            offset: 0,
            gain,
            channel: 0,
        }
    }

    fn note(start: usize, length: usize, pitch: u8, velocity: u8) -> Note {
        Note {
            channel: 0,
            start,
            length,
            note: pitch,
            velocity,
            frequency: 440.0 * ((f64::from(pitch) - 69.0) / 12.0).exp2(),
            confidence: 1.0,
        }
    }

    fn session() -> Session {
        Session {
            name: "a song".to_string(),
            sample_rate: 48_000,
            tempo: 120.0,
            frames: 8 * BEAT,
            channels: 2,
            tracks: vec![
                Track {
                    name: "kick".to_string(),
                    kind: TrackKind::Audio,
                    clips: vec![
                        clip("kick", "samples/kick.wav", 0, BEAT, 1.0),
                        clip("kick", "samples/kick.wav", 4 * BEAT, BEAT, 0.5),
                    ],
                    notes: Vec::new(),
                },
                Track {
                    name: "notes".to_string(),
                    kind: TrackKind::Instrument,
                    clips: Vec::new(),
                    notes: vec![note(0, BEAT, 60, 100), note(2 * BEAT, BEAT, 67, 80)],
                },
            ],
            instruments: vec![Instrument {
                id: 0,
                name: "kick".to_string(),
                file: "samples/kick.wav".to_string(),
                note: Some(36),
                frames: BEAT,
                peak: 0.9,
            }],
        }
    }

    #[test]
    fn a_session_survives_the_round_trip() {
        let session = session();

        let read = parse(&write(&session)).unwrap();

        assert_eq!(read.name, "a song");
        assert_eq!(read.sample_rate, 48_000);
        assert!((read.tempo - 120.0).abs() < 1e-9);
        assert_eq!(read.tracks.len(), 2);
        for (read, expected) in read.tracks.iter().zip(&session.tracks) {
            assert_eq!((&read.name, read.kind), (&expected.name, expected.kind));
            assert_eq!(read.clips.len(), expected.clips.len());
            for (read, expected) in read.clips.iter().zip(&expected.clips) {
                assert_eq!(
                    (&read.file, read.start, read.length, read.offset),
                    (
                        &expected.file,
                        expected.start,
                        expected.length,
                        expected.offset
                    )
                );
                assert!((read.gain - expected.gain).abs() < 1e-9);
            }
            assert_eq!(read.notes, expected.notes);
        }
    }

    #[test]
    fn the_file_looks_like_a_reaper_project() {
        let text = write(&session());

        assert!(text.starts_with("<REAPER_PROJECT 0.1 "), "{text}");
        assert!(text.trim_end().ends_with('>'), "{text}");
        assert!(text.contains("<SOURCE WAVE"), "{text}");
        assert!(text.contains("FILE samples/kick.wav"), "{text}");
        // A name with a space has to be quoted or REAPER reads two tokens.
        assert!(text.contains("TITLE \"a song\""), "{text}");
    }

    #[test]
    fn notes_are_written_as_midi_events() {
        let text = write(&session());

        assert!(text.contains("HASDATA 1 960 QN"), "{text}");
        // A note on at tick zero, its note off a beat later.
        assert!(text.contains("E 0 90 3c 64"), "{text}");
        assert!(text.contains("E 960 80 3c 00"), "{text}");
    }

    #[test]
    fn a_repeated_note_stops_before_it_starts_again() {
        let mut session = session();
        session.tracks[1].notes = vec![note(0, BEAT, 60, 100), note(BEAT, BEAT, 60, 100)];

        let read = parse(&write(&session)).unwrap();

        assert_eq!(read.tracks[1].notes.len(), 2);
        assert_eq!(read.tracks[1].notes[0].length, BEAT);
        assert_eq!(read.tracks[1].notes[1].start, BEAT);
    }

    #[test]
    fn quoting_picks_a_quote_the_value_does_not_use() {
        assert_eq!(quote("kick"), "kick");
        assert_eq!(quote("a song"), "\"a song\"");
        assert_eq!(quote("say \"hi\""), "'say \"hi\"'");
        assert_eq!(quote("it's \"here\""), "`it's \"here\"`");
        assert_eq!(quote(""), "\"\"");
    }

    #[test]
    fn a_project_written_elsewhere_is_understood() {
        let text = "<REAPER_PROJECT 0.1 \"7.16/linux\" 1700000000\n  \
            SAMPLERATE 44100\n  TEMPO 90 4 4\n  TITLE take\n  \
            <TRACK {2C1C1D66}\n    NAME Guitar\n    \
            <ITEM\n      POSITION 1.5\n      LENGTH 2\n      SOFFS 0.25\n      \
            NAME 'guitar take'\n      <SOURCE WAVE\n        FILE \"audio/guitar.wav\"\n      >\n    >\n  >\n>\n";

        let session = parse(text).unwrap();

        assert_eq!(session.sample_rate, 44_100);
        assert!((session.tempo - 90.0).abs() < 1e-9);
        assert_eq!(session.tracks[0].name, "Guitar");
        let clip = &session.tracks[0].clips[0];
        assert_eq!(clip.name, "guitar take");
        assert_eq!(clip.file, "audio/guitar.wav");
        assert_eq!(clip.start, 66_150);
        assert_eq!(clip.offset, 11_025);
    }

    #[test]
    fn a_tree_can_be_written_back_out_unchanged() {
        let text = write(&session());

        assert_eq!(render(&tree(&text).unwrap()), text);
    }

    #[test]
    fn broken_projects_are_reported() {
        let complain = |text: &str| parse(text).unwrap_err().to_string();

        assert!(complain("").contains("empty"));
        assert!(complain("<TRACK\n>\n").contains("not a REAPER project"));
        assert!(complain("<REAPER_PROJECT\n>\n").contains("no sample rate"));
        assert!(tree("NAME x\n")
            .unwrap_err()
            .to_string()
            .contains("outside"));
        assert!(tree(">\n")
            .unwrap_err()
            .to_string()
            .contains("closes nothing"));
        assert!(tree("<A\n")
            .unwrap_err()
            .to_string()
            .contains("ends inside"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-reaper-{}-{:?}/song.rpp",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session()).unwrap();

        assert_eq!(read_file(&path).unwrap().tracks.len(), 2);

        let _ = fs::remove_dir_all(&root);
    }
}
