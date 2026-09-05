//! MusicXML, the notation format Finale, Sibelius, Dorico and MuseScore read.
//!
//! MIDI says when a key went down; notation says what was played. Turning the
//! recognised notes into notation means committing to a grid, a bar length and
//! a set of note values, so this module quantises to a division of the beat,
//! splits anything that crosses a bar line into tied notes, and writes the rests
//! that fill the gaps. Notes that start together become a chord.
//!
//! What it does not do is invent voices: notes that overlap without starting
//! together are shortened to the next attack, because a single-voice part that
//! plays the right pitches at the right times is more useful than a
//! multi-voice guess. The exact timing always remains in the MIDI export and in
//! the manifest — notation is the readable view, not the authoritative one.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::session::Session;
use crate::formats::xml::{self, Element};

/// How the score is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreOptions {
    /// Divisions per quarter note; the grid everything is quantised to.
    pub divisions: u32,
    /// Beats in a bar.
    pub beats: u32,
    /// Note value that gets the beat.
    pub beat_type: u32,
}

impl Default for ScoreOptions {
    fn default() -> Self {
        Self {
            divisions: 8,
            beats: 4,
            beat_type: 4,
        }
    }
}

impl ScoreOptions {
    /// Divisions in one bar.
    #[must_use]
    pub const fn bar(&self) -> u32 {
        self.divisions * 4 * self.beats / self.beat_type
    }
}

/// The note values a duration can be written with, longest first.
const VALUES: [(&str, u32, u32); 6] = [
    ("whole", 4, 1),
    ("half", 2, 1),
    ("quarter", 1, 1),
    ("eighth", 1, 2),
    ("16th", 1, 4),
    ("32nd", 1, 8),
];

/// The letter and accidental of each semitone, sharps only.
const STEPS: [(&str, i32); 12] = [
    ("C", 0),
    ("C", 1),
    ("D", 0),
    ("D", 1),
    ("E", 0),
    ("F", 0),
    ("F", 1),
    ("G", 0),
    ("G", 1),
    ("A", 0),
    ("A", 1),
    ("B", 0),
];

/// Writes the notes of a session as a MusicXML score.
#[must_use]
pub fn write(session: &Session) -> String {
    with_options(session, &ScoreOptions::default())
}

/// Writes the score with a chosen grid and bar length.
#[must_use]
pub fn with_options(session: &Session, options: &ScoreOptions) -> String {
    xml::document(&score(session, options))
}

/// Writes the score to a file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, write(session))?;
    Ok(())
}

/// Builds the score element.
#[must_use]
pub fn score(session: &Session, options: &ScoreOptions) -> Element {
    let events = quantise(session, options);
    Element::new("score-partwise")
        .attribute("version", "4.0")
        .child(Element::new("work").child(Element::leaf("work-title", session.name.clone())))
        .child(
            Element::new("identification").child(
                Element::new("encoding")
                    .child(Element::leaf("software", "audio-decomposer"))
                    .child(Element::leaf(
                        "encoding-description",
                        "decomposed recording",
                    )),
            ),
        )
        .child(
            Element::new("part-list").child(
                Element::new("score-part")
                    .attribute("id", "P1")
                    .child(Element::leaf("part-name", "recognised notes")),
            ),
        )
        .child(part(&events, session, options))
}

/// A note after quantisation, in divisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    start: u32,
    duration: u32,
    pitch: u8,
    velocity: u8,
}

/// Quantises the notes of a session onto the grid, shortening anything that
/// runs into the next attack.
fn quantise(session: &Session, options: &ScoreOptions) -> Vec<Event> {
    let mut chords: BTreeMap<u32, Vec<Event>> = BTreeMap::new();
    for note in session.notes() {
        let start = divisions_of(session, note.start, options);
        let end = divisions_of(session, note.end(), options).max(start + 1);
        chords.entry(start).or_default().push(Event {
            start,
            duration: end - start,
            pitch: note.note,
            velocity: note.velocity,
        });
    }

    let starts: Vec<u32> = chords.keys().copied().collect();
    let mut events = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let limit = starts.get(index + 1).map_or(u32::MAX, |next| next - start);
        let Some(chord) = chords.get(start) else {
            continue;
        };
        let duration = chord
            .iter()
            .map(|event| event.duration)
            .min()
            .unwrap_or(1)
            .min(limit)
            .max(1);
        events.extend(chord.iter().map(|event| Event { duration, ..*event }));
    }
    events
}

/// Where a frame lands on the grid.
fn divisions_of(session: &Session, frames: usize, options: &ScoreOptions) -> u32 {
    let beats = session.beats(frames) * f64::from(options.divisions);
    let rounded = beats.round();
    if rounded <= 0.0 {
        0
    } else {
        rounded.min(f64::from(u32::MAX)) as u32
    }
}

/// Builds the single part of the score.
fn part(events: &[Event], session: &Session, options: &ScoreOptions) -> Element {
    let bar = options.bar();
    let end = events
        .iter()
        .map(|event| event.start + event.duration)
        .max()
        .unwrap_or(0);
    let bars = end.div_ceil(bar).max(1);

    let mut part = Element::new("part").attribute("id", "P1");
    for index in 0..bars {
        let from = index * bar;
        let mut measure = Element::new("measure").attribute("number", (index + 1).to_string());
        if index == 0 {
            measure = measure.child(attributes(options)).child(
                Element::new("direction")
                    .attribute("placement", "above")
                    .child(
                        Element::new("sound").attribute("tempo", format!("{:.4}", session.tempo)),
                    ),
            );
        }
        for element in measure_notes(events, from, bar, options.divisions) {
            measure = measure.child(element);
        }
        part = part.child(measure);
    }
    part
}

/// The `attributes` element that opens the score.
fn attributes(options: &ScoreOptions) -> Element {
    Element::new("attributes")
        .child(Element::leaf("divisions", options.divisions.to_string()))
        .child(Element::new("key").child(Element::leaf("fifths", "0")))
        .child(
            Element::new("time")
                .child(Element::leaf("beats", options.beats.to_string()))
                .child(Element::leaf("beat-type", options.beat_type.to_string())),
        )
        .child(
            Element::new("clef")
                .child(Element::leaf("sign", "G"))
                .child(Element::leaf("line", "2")),
        )
}

/// The notes and rests of one bar.
fn measure_notes(events: &[Event], from: u32, bar: u32, divisions: u32) -> Vec<Element> {
    let to = from + bar;
    let mut out = Vec::new();
    let mut cursor = from;
    let mut starts: Vec<u32> = events
        .iter()
        .filter(|event| event.start + event.duration > from && event.start < to)
        .map(|event| event.start.max(from))
        .collect();
    starts.sort_unstable();
    starts.dedup();

    for start in starts {
        if start > cursor {
            out.extend(rests(start - cursor, divisions));
        }
        let chord: Vec<&Event> = events
            .iter()
            .filter(|event| event.start.max(from) == start && event.start < to)
            .collect();
        let length = chord
            .iter()
            .map(|event| (event.start + event.duration).min(to) - start)
            .max()
            .unwrap_or(0);
        if length == 0 {
            continue;
        }
        for (index, event) in chord.iter().enumerate() {
            let played = (event.start + event.duration).min(to) - start;
            let tied = event.start + event.duration > to;
            out.extend(note_elements(
                event,
                played,
                divisions,
                index > 0,
                tied,
                event.start < from,
            ));
        }
        cursor = start + length;
    }
    if cursor < to {
        out.extend(rests(to - cursor, divisions));
    }
    out
}

/// One note, split into as many tied values as it takes to write it down.
fn note_elements(
    event: &Event,
    length: u32,
    divisions: u32,
    chord: bool,
    tie_forward: bool,
    tie_back: bool,
) -> Vec<Element> {
    let pieces = split(length, divisions);
    let count = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(index, (duration, name, dots))| {
            let starts_tie = tie_forward || index + 1 < count;
            let stops_tie = tie_back || index > 0;
            let mut note = Element::new("note")
                .attribute("dynamics", format!("{:.4}", dynamics(event.velocity)));
            if chord {
                note = note.child(Element::new("chord"));
            }
            let (step, alter) = STEPS[event.pitch as usize % 12];
            let mut pitch = Element::new("pitch").child(Element::leaf("step", step));
            if alter != 0 {
                pitch = pitch.child(Element::leaf("alter", alter.to_string()));
            }
            note = note
                .child(pitch.child(Element::leaf(
                    "octave",
                    (i32::from(event.pitch) / 12 - 1).to_string(),
                )))
                .child(Element::leaf("duration", duration.to_string()));
            for (condition, kind) in [(stops_tie, "stop"), (starts_tie, "start")] {
                if condition {
                    note = note.child(Element::new("tie").attribute("type", kind));
                }
            }
            note = note.child(Element::leaf("voice", "1"));
            note = note.child(Element::leaf("type", name));
            for _ in 0..dots {
                note = note.child(Element::new("dot"));
            }
            if starts_tie || stops_tie {
                let mut notations = Element::new("notations");
                for (condition, kind) in [(stops_tie, "stop"), (starts_tie, "start")] {
                    if condition {
                        notations = notations.child(Element::new("tied").attribute("type", kind));
                    }
                }
                note = note.child(notations);
            }
            note
        })
        .collect()
}

/// The rests that fill a gap.
fn rests(length: u32, divisions: u32) -> Vec<Element> {
    split(length, divisions)
        .into_iter()
        .map(|(duration, name, dots)| {
            let mut rest = Element::new("note")
                .child(Element::new("rest"))
                .child(Element::leaf("duration", duration.to_string()))
                .child(Element::leaf("voice", "1"))
                .child(Element::leaf("type", name));
            for _ in 0..dots {
                rest = rest.child(Element::new("dot"));
            }
            rest
        })
        .collect()
}

/// Splits a length in divisions into writable note values, longest first.
///
/// The grid is a division of the quarter note, so the greedy split always
/// terminates: whatever is left over is written as the shortest value that
/// fits, and one division always fits.
fn split(length: u32, divisions: u32) -> Vec<(u32, &'static str, u32)> {
    let mut out = Vec::new();
    let mut left = length;
    while left > 0 {
        let piece = value_for(left, divisions);
        left -= piece.0;
        out.push(piece);
    }
    out
}

/// The longest note value that fits, with a dot when the dotted value fits too.
///
/// A length the grid cannot express — one division of a grid that is not a
/// power of two, say — is written as the shortest value there is, so the score
/// stays readable even when the timing is unusual.
fn value_for(length: u32, divisions: u32) -> (u32, &'static str, u32) {
    let mut best = (1, "32nd", 0);
    for (name, numerator, denominator) in VALUES {
        let base = divisions * numerator;
        if base % denominator != 0 {
            continue;
        }
        let base = base / denominator;
        for dots in [1u32, 0] {
            let dotted = base * (2 + dots);
            if dotted % 2 != 0 {
                continue;
            }
            let value = dotted / 2;
            if value > 0 && value <= length && value > best.0 {
                best = (value, name, dots);
            }
        }
    }
    best
}

/// Turns a MIDI velocity into the percentage MusicXML calls dynamics, where
/// 100 means a velocity of 90.
fn dynamics(velocity: u8) -> f64 {
    f64::from(velocity) * 100.0 / 90.0
}

/// Turns the percentage back into a velocity.
fn velocity(dynamics: f64) -> u8 {
    let value = (dynamics * 90.0 / 100.0).round();
    if value <= 0.0 {
        0
    } else if value >= 127.0 {
        127
    } else {
        value as u8
    }
}

/// Reads a score back into notes on a timeline.
///
/// The score carries beats, so turning it back into frames needs the sample
/// rate and the tempo the score was written at; the tempo written into the
/// score is used when the caller passes `None`.
pub fn parse(text: &str, sample_rate: u32, tempo: Option<f64>) -> Result<Vec<Note>> {
    let root = xml::parse(text)?;
    if root.name != "score-partwise" {
        return Err(Error::Format(format!(
            "`{}` is not a partwise MusicXML score",
            root.name
        )));
    }
    let part = root
        .find("part")
        .ok_or_else(|| Error::Format("the score has no part".to_string()))?;
    let first = part.find("measure");
    let divisions = first
        .and_then(|measure| measure.find("attributes"))
        .and_then(|attributes| attributes.find("divisions"))
        .map_or(Ok(1), |element| number(&element.content(), "divisions"))?
        .max(1);
    let tempo = match tempo {
        Some(tempo) => tempo,
        None => first
            .and_then(|measure| measure.find("direction"))
            .and_then(|direction| direction.find("sound"))
            .and_then(|sound| sound.get("tempo"))
            .map_or(Ok(120.0), |value| {
                value
                    .parse::<f64>()
                    .map_err(|_| Error::Parse(format!("`{value}` is not a tempo")))
            })?,
    };

    let mut events = read_events(part)?;
    events.sort_by_key(|event| (event.start, event.pitch));
    Ok(events
        .into_iter()
        .map(|event| Note {
            channel: 0,
            start: frames_of(event.start, divisions, tempo, sample_rate),
            length: frames_of(event.start + event.duration, divisions, tempo, sample_rate)
                - frames_of(event.start, divisions, tempo, sample_rate),
            note: event.pitch,
            velocity: event.velocity,
            frequency: frequency(event.pitch),
            confidence: 1.0,
        })
        .collect())
}

/// Walks the measures of a part and collects what sounds, in divisions,
/// rejoining anything the notation had to write as tied pieces.
fn read_events(part: &Element) -> Result<Vec<Event>> {
    let mut events: Vec<Event> = Vec::new();
    let mut cursor = 0u32;
    let mut previous = 0u32;
    for measure in part.find_all("measure") {
        for element in measure.find_all("note") {
            let duration = element
                .find("duration")
                .map_or(Ok(0), |value| number(&value.content(), "duration"))?;
            let chord = element.find("chord").is_some();
            let start = if chord { previous } else { cursor };
            if element.find("rest").is_none() {
                let pitch = pitch_of(element)?;
                let velocity = element.get("dynamics").map_or(Ok(64), |value| {
                    value
                        .parse::<f64>()
                        .map(velocity)
                        .map_err(|_| Error::Parse(format!("`{value}` is not a dynamics value")))
                })?;
                let continued = element
                    .find_all("tie")
                    .any(|tie| tie.get("type") == Some("stop"))
                    && events.iter_mut().rev().any(|event| {
                        let joins = event.pitch == pitch && event.start + event.duration == start;
                        if joins {
                            event.duration += duration;
                        }
                        joins
                    });
                if !continued {
                    events.push(Event {
                        start,
                        duration,
                        pitch,
                        velocity,
                    });
                }
            }
            if !chord {
                previous = cursor;
                cursor += duration;
            }
        }
    }
    Ok(events)
}

/// Reads a score file back into notes.
pub fn read_file(
    path: impl AsRef<Path>,
    sample_rate: u32,
    tempo: Option<f64>,
) -> Result<Vec<Note>> {
    parse(&fs::read_to_string(path)?, sample_rate, tempo)
}

/// Frames a length in divisions lasts.
fn frames_of(divisions_count: u32, divisions: u32, tempo: f64, sample_rate: u32) -> usize {
    if divisions == 0 || tempo <= 0.0 {
        return 0;
    }
    let beats = f64::from(divisions_count) / f64::from(divisions);
    let seconds = beats * 60.0 / tempo;
    (seconds * f64::from(sample_rate)).round().max(0.0) as usize
}

/// The pitch of a note element.
fn pitch_of(element: &Element) -> Result<u8> {
    let pitch = element
        .find("pitch")
        .ok_or_else(|| Error::Format("a sounding note has no pitch".to_string()))?;
    let step = pitch
        .find("step")
        .map(Element::content)
        .ok_or_else(|| Error::Format("a pitch has no step".to_string()))?;
    let octave: i32 = pitch
        .find("octave")
        .map(Element::content)
        .ok_or_else(|| Error::Format("a pitch has no octave".to_string()))?
        .trim()
        .parse()
        .map_err(|_| Error::Parse("a pitch has an octave that is not a number".to_string()))?;
    let alter: i32 = pitch.find("alter").map_or(Ok(0), |element| {
        element
            .content()
            .trim()
            .parse()
            .map_err(|_| Error::Parse("a pitch has an alter that is not a number".to_string()))
    })?;
    let semitone = match step.trim() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        other => return Err(Error::Format(format!("`{other}` is not a step"))),
    };
    let value = (octave + 1) * 12 + semitone + alter;
    u8::try_from(value.clamp(0, 127))
        .map_err(|_| Error::Format(format!("{value} is not a MIDI note")))
}

/// Parses a number out of an element.
fn number(text: &str, what: &str) -> Result<u32> {
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("`{}` is not a {what}", text.trim())))
}

/// The frequency of a MIDI note in twelve-tone equal temperament.
fn frequency(note: u8) -> f64 {
    440.0 * ((f64::from(note) - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::session::{Session, Track, TrackKind};

    fn session(notes: Vec<Note>) -> Session {
        Session {
            name: "song".to_string(),
            sample_rate: 48_000,
            tempo: 120.0,
            frames: notes.iter().map(Note::end).max().unwrap_or(0),
            channels: 1,
            tracks: vec![Track {
                name: "notes".to_string(),
                kind: TrackKind::Instrument,
                clips: Vec::new(),
                notes,
            }],
            instruments: Vec::new(),
        }
    }

    /// One quarter note at 120 bpm and 48 kHz.
    const QUARTER: usize = 24_000;

    fn note(start: usize, length: usize, pitch: u8, velocity: u8) -> Note {
        Note {
            channel: 0,
            start,
            length,
            note: pitch,
            velocity,
            frequency: frequency(pitch),
            confidence: 1.0,
        }
    }

    #[test]
    fn notes_survive_the_round_trip_through_notation() {
        let notes = vec![
            note(0, QUARTER, 60, 90),
            note(QUARTER, QUARTER / 2, 64, 72),
            note(2 * QUARTER, 2 * QUARTER, 67, 108),
        ];
        let session = session(notes.clone());

        let text = write(&session);
        let read = parse(&text, 48_000, Some(120.0)).unwrap();

        assert_eq!(read.len(), notes.len());
        for (read, expected) in read.iter().zip(&notes) {
            assert_eq!((read.start, read.length), (expected.start, expected.length));
            assert_eq!(
                (read.note, read.velocity),
                (expected.note, expected.velocity)
            );
        }
    }

    #[test]
    fn notes_that_start_together_become_a_chord() {
        let session = session(vec![
            note(0, QUARTER, 60, 90),
            note(0, QUARTER, 64, 90),
            note(0, QUARTER, 67, 90),
        ]);

        let text = write(&session);
        assert_eq!(text.matches("<chord/>").count(), 2);

        let read = parse(&text, 48_000, None).unwrap();
        assert_eq!(
            read.iter().map(|note| note.note).collect::<Vec<_>>(),
            [60, 64, 67]
        );
        assert!(read.iter().all(|note| note.start == 0));
        assert!(read.iter().all(|note| note.length == QUARTER));
    }

    #[test]
    fn a_note_across_a_bar_line_is_written_as_tied_notes() {
        // Three beats in, lasting two beats: it crosses into the second bar.
        let session = session(vec![note(3 * QUARTER, 2 * QUARTER, 62, 90)]);

        let text = write(&session);
        assert!(text.contains("<tie type=\"start\"/>"), "{text}");
        assert!(text.contains("<tie type=\"stop\"/>"), "{text}");
        assert_eq!(text.matches("<measure").count(), 2);

        let read = parse(&text, 48_000, Some(120.0)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!((read[0].start, read[0].length), (3 * QUARTER, 2 * QUARTER));
    }

    #[test]
    fn gaps_are_filled_with_rests() {
        let session = session(vec![note(2 * QUARTER, QUARTER, 60, 90)]);

        let text = write(&session);

        assert!(text.contains("<rest/>"));
        // Two beats of rest before the note and one after it.
        assert_eq!(text.matches("<rest/>").count(), 2);
        assert_eq!(parse(&text, 48_000, Some(120.0)).unwrap().len(), 1);
    }

    #[test]
    fn accidentals_and_octaves_are_written() {
        let session = session(vec![
            note(0, QUARTER, 61, 90),
            note(QUARTER, QUARTER, 90, 90),
        ]);

        let text = write(&session);
        assert!(text.contains("<step>C</step>"));
        assert!(text.contains("<alter>1</alter>"));
        assert!(text.contains("<octave>4</octave>"));
        assert!(text.contains("<octave>6</octave>"));

        let read = parse(&text, 48_000, Some(120.0)).unwrap();
        assert_eq!(read[0].note, 61);
        assert_eq!(read[1].note, 90);
    }

    #[test]
    fn overlapping_notes_are_shortened_to_the_next_attack() {
        let session = session(vec![
            note(0, 4 * QUARTER, 60, 90),
            note(QUARTER, QUARTER, 64, 90),
        ]);

        let read = parse(&write(&session), 48_000, Some(120.0)).unwrap();

        assert_eq!(read[0].length, QUARTER);
        assert_eq!(read[1].start, QUARTER);
    }

    #[test]
    fn a_score_without_notes_is_still_a_score() {
        let text = write(&session(Vec::new()));

        assert!(text.contains("<measure number=\"1\">"));
        assert!(parse(&text, 48_000, None).unwrap().is_empty());
    }

    #[test]
    fn a_dotted_value_is_used_where_it_fits() {
        let session = session(vec![note(0, QUARTER + QUARTER / 2, 60, 90)]);

        let text = write(&session);

        assert!(text.contains("<type>quarter</type>"), "{text}");
        assert!(text.contains("<dot/>"), "{text}");
    }

    #[test]
    fn broken_scores_are_reported() {
        let complain = |text: &str| parse(text, 48_000, None).unwrap_err().to_string();

        assert!(complain("<score-timewise/>").contains("not a partwise"));
        assert!(complain("<score-partwise/>").contains("no part"));
        assert!(complain(
            "<score-partwise><part><measure><note><duration>1</duration></note></measure></part></score-partwise>"
        )
        .contains("no pitch"));
        assert!(complain(
            "<score-partwise><part><measure><note><pitch><step>H</step><octave>4</octave></pitch></note></measure></part></score-partwise>"
        )
        .contains("not a step"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-musicxml-{}-{:?}/song.musicxml",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session(vec![note(0, QUARTER, 60, 90)])).unwrap();

        let read = read_file(&path, 48_000, Some(120.0)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].note, 60);

        let _ = fs::remove_dir_all(&root);
    }
}
