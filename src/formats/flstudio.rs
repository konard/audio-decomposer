//! FL Studio projects, the binary `.flp` format.
//!
//! An `.flp` file is two chunks. `FLhd` says how many channels the rack has
//! and how many ticks a quarter note is worth; `FLdt` is a flat list of
//! events. Every event starts with a one byte identifier whose range gives its
//! size: `0..64` carry one byte, `64..128` two, `128..192` four, and `192..256`
//! carry a length prefixed block. That rule is the reason this module can read
//! files it does not understand: an unknown event is still exactly as long as
//! its identifier says, so [`events`] never loses its place.
//!
//! What the writer emits is the part of a project a decomposition fills: the
//! tempo, one sampler channel per sample in the bank, one pattern holding the
//! recognised notes, and playlist items placing the samples on the timeline.
//!
//! The layout of the events themselves is not defined by any published
//! specification; it is the layout the community parsers agree on, and the
//! identifiers used here are named in [`id`] so a reader can check them. What
//! is verified here is that the container and every event round trips through
//! this crate byte for byte, and that a file written by this module reads back
//! into the session it came from. It has not been opened in FL Studio.

use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::session::{Clip, Session, Track, TrackKind};

/// Magic of the header chunk.
pub const HEADER_MAGIC: &[u8; 4] = b"FLhd";
/// Magic of the event chunk.
pub const DATA_MAGIC: &[u8; 4] = b"FLdt";
/// Ticks in a quarter note. FL Studio itself writes 96.
pub const PPQ: u16 = 96;
/// Project format of a full song, as opposed to a score or a state.
pub const FORMAT_SONG: i16 = 0;
/// Version string written into the project.
pub const VERSION: &str = "20.8.3.2304";
/// Pattern identifiers start here in a playlist item.
pub const PATTERN_BASE: u16 = 0x5000;
/// Playlist tracks are numbered downwards from this in FL Studio 20.
pub const TRACK_BASE: u16 = 500;
/// Bytes in one note of a pattern.
const NOTE_SIZE: usize = 24;
/// Bytes in one playlist item.
const ITEM_SIZE: usize = 32;

/// The event identifiers this module writes and reads.
pub mod id {
    /// Kind of a channel, `0` being a sampler.
    pub const CHANNEL_TYPE: u8 = 21;
    /// Starts a channel, its value being the channel index.
    pub const NEW_CHANNEL: u8 = 64;
    /// Starts a pattern, its value being the pattern number.
    pub const NEW_PATTERN: u8 = 65;
    /// The pattern later events belong to.
    pub const CURRENT_PATTERN: u8 = 67;
    /// Sample rate the project renders at.
    pub const SAMPLE_RATE: u8 = 153;
    /// Tempo in thousandths of a beat per minute.
    pub const FINE_TEMPO: u8 = 156;
    /// Name of a channel.
    pub const CHANNEL_NAME: u8 = 192;
    /// Name of a pattern.
    pub const PATTERN_NAME: u8 = 193;
    /// Title of the project.
    pub const TITLE: u8 = 194;
    /// File a sampler channel plays.
    pub const SAMPLE_PATH: u8 = 196;
    /// Version of FL Studio the project claims to come from.
    pub const VERSION: u8 = 199;
    /// The notes of the current pattern.
    pub const PATTERN_NOTES: u8 = 224;
    /// The items on the playlist.
    pub const PLAYLIST: u8 = 233;
    /// Name of a playlist track.
    pub const TRACK_NAME: u8 = 239;
}

/// One event of the `FLdt` chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// An event carrying a single byte.
    Byte(u8, u8),
    /// An event carrying a sixteen bit word.
    Word(u8, u16),
    /// An event carrying a thirty two bit word.
    Dword(u8, u32),
    /// An event carrying a length prefixed block.
    Data(u8, Vec<u8>),
}

impl Event {
    /// Identifier of the event.
    #[must_use]
    pub const fn id(&self) -> u8 {
        match *self {
            Self::Byte(id, _) | Self::Word(id, _) | Self::Dword(id, _) | Self::Data(id, _) => id,
        }
    }

    /// An event holding text, which FL Studio 12 and later store as UTF-16.
    #[must_use]
    pub fn text(id: u8, text: &str) -> Self {
        let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        Self::Data(id, bytes)
    }

    /// The text of the event, whether it was stored as UTF-16 or as bytes.
    #[must_use]
    pub fn as_text(&self) -> Option<String> {
        let Self::Data(_, bytes) = self else {
            return None;
        };
        // Only UTF-16 puts a zero in every second byte of ASCII text.
        let wide = bytes.len() % 2 == 0
            && bytes.len() >= 2
            && bytes.chunks_exact(2).all(|pair| pair[1] == 0);
        let text = if wide {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&byte| char::from(byte)).collect()
        };
        Some(text.trim_end_matches('\0').to_string())
    }

    /// Appends the event to a file being written.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(self.id());
        match self {
            Self::Byte(_, value) => out.push(*value),
            Self::Word(_, value) => out.extend_from_slice(&value.to_le_bytes()),
            Self::Dword(_, value) => out.extend_from_slice(&value.to_le_bytes()),
            Self::Data(_, bytes) => {
                write_varint(out, bytes.len());
                out.extend_from_slice(bytes);
            }
        }
    }
}

/// Writes the length of a data event, seven bits to a byte.
fn write_varint(out: &mut Vec<u8>, mut length: usize) {
    loop {
        let byte = u8::try_from(length & 0x7f).unwrap_or_default();
        length >>= 7;
        if length == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads such a length back.
fn read_varint(bytes: &[u8], at: &mut usize) -> Result<usize> {
    let mut length = 0usize;
    for shift in 0..5 {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| Error::Parse("an event length runs past the end".to_string()))?;
        *at += 1;
        length |= usize::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return Ok(length);
        }
    }
    Err(Error::Parse("an event length never ends".to_string()))
}

/// Splits the `FLdt` chunk into events.
pub fn events(bytes: &[u8]) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        let event = match id {
            0..=63 => Event::Byte(id, *take(bytes, &mut at, 1)?.first().unwrap_or(&0)),
            64..=127 => {
                let slice = take(bytes, &mut at, 2)?;
                Event::Word(id, u16::from_le_bytes([slice[0], slice[1]]))
            }
            128..=191 => {
                let slice = take(bytes, &mut at, 4)?;
                Event::Dword(
                    id,
                    u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]),
                )
            }
            _ => {
                let length = read_varint(bytes, &mut at)?;
                Event::Data(id, take(bytes, &mut at, length)?.to_vec())
            }
        };
        out.push(event);
    }
    Ok(out)
}

/// Takes a run of bytes, complaining rather than panicking at the end.
fn take<'a>(bytes: &'a [u8], at: &mut usize, length: usize) -> Result<&'a [u8]> {
    let end = at
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| Error::Parse("an event runs past the end of the project".to_string()))?;
    let slice = &bytes[*at..end];
    *at = end;
    Ok(slice)
}

/// Writes a session as the bytes of an `.flp` file.
#[must_use]
pub fn write(session: &Session) -> Vec<u8> {
    let events = project(session);
    let channels = u16::try_from(session.instruments.len()).unwrap_or(u16::MAX);

    let mut data = Vec::new();
    for event in &events {
        event.write(&mut data);
    }

    let mut out = Vec::with_capacity(data.len() + 32);
    out.extend_from_slice(HEADER_MAGIC);
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_SONG.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&PPQ.to_le_bytes());
    out.extend_from_slice(DATA_MAGIC);
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Writes a session as an `.flp` file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(fs::write(path, write(session))?)
}

/// Builds the events of a project.
#[must_use]
pub fn project(session: &Session) -> Vec<Event> {
    let mut out = vec![
        Event::text(id::VERSION, VERSION),
        Event::text(id::TITLE, &session.name),
        Event::Dword(id::SAMPLE_RATE, session.sample_rate),
        Event::Dword(
            id::FINE_TEMPO,
            (session.tempo * 1000.0).round().max(0.0) as u32,
        ),
    ];

    for (index, instrument) in session.instruments.iter().enumerate() {
        let index = u16::try_from(index).unwrap_or(u16::MAX);
        out.push(Event::Word(id::NEW_CHANNEL, index));
        // Zero is the sampler, which is what a bank sample is played by.
        out.push(Event::Byte(id::CHANNEL_TYPE, 0));
        out.push(Event::text(id::CHANNEL_NAME, &instrument.name));
        out.push(Event::text(id::SAMPLE_PATH, &instrument.file));
    }

    for (index, track) in session.instrument_tracks().enumerate() {
        let index = u16::try_from(index).unwrap_or(u16::MAX);
        out.push(Event::Word(id::NEW_PATTERN, index));
        out.push(Event::Word(id::CURRENT_PATTERN, index));
        out.push(Event::text(id::PATTERN_NAME, &track.name));
        out.push(Event::Data(id::PATTERN_NOTES, notes(&track.notes, session)));
    }

    // Track names come in playlist order, which is the order the items below
    // point back into.
    for track in &session.tracks {
        out.push(Event::text(id::TRACK_NAME, &track.name));
    }
    out.push(Event::Data(id::PLAYLIST, playlist(session)));
    out
}

/// The notes of a pattern, twenty four bytes each.
fn notes(notes: &[Note], session: &Session) -> Vec<u8> {
    let mut out = Vec::with_capacity(notes.len() * NOTE_SIZE);
    for note in notes {
        let mut bytes = [0u8; NOTE_SIZE];
        bytes[0..4].copy_from_slice(&ticks(session, note.start).to_le_bytes());
        bytes[6..8].copy_from_slice(&0u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&ticks(session, note.length).to_le_bytes());
        bytes[12] = note.note;
        // Fine pitch, release and panning sit at their centre.
        bytes[14] = 120;
        bytes[16] = 128;
        bytes[18] = 128;
        bytes[19] = note.velocity;
        bytes[20] = 128;
        bytes[21] = 128;
        out.extend_from_slice(&bytes);
    }
    out
}

/// The playlist, thirty two bytes an item.
fn playlist(session: &Session) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pattern = 0u16;
    for (index, track) in session.tracks.iter().enumerate() {
        // FL Studio numbers playlist tracks downwards from the last one.
        let lane = TRACK_BASE.saturating_sub(u16::try_from(index).unwrap_or(0));
        match track.kind {
            TrackKind::Audio => {
                for clip in &track.clips {
                    let channel = session
                        .instruments
                        .iter()
                        .position(|instrument| instrument.file == clip.file)
                        .unwrap_or(0);
                    out.extend_from_slice(&item(
                        ticks(session, clip.start),
                        u16::try_from(channel).unwrap_or(0),
                        ticks(session, clip.length),
                        lane,
                        ticks(session, clip.offset),
                    ));
                }
            }
            TrackKind::Instrument => {
                let start = track.notes.iter().map(|note| note.start).min().unwrap_or(0);
                let end = track.notes.iter().map(Note::end).max().unwrap_or(0);
                out.extend_from_slice(&item(
                    ticks(session, start),
                    PATTERN_BASE + pattern,
                    ticks(session, end.saturating_sub(start)),
                    lane,
                    0,
                ));
                pattern += 1;
            }
        }
    }
    out
}

/// One playlist item.
fn item(position: u32, source: u16, length: u32, lane: u16, offset: u32) -> [u8; ITEM_SIZE] {
    let mut bytes = [0u8; ITEM_SIZE];
    bytes[0..4].copy_from_slice(&position.to_le_bytes());
    bytes[4..6].copy_from_slice(&source.to_le_bytes());
    bytes[6..10].copy_from_slice(&length.to_le_bytes());
    bytes[10..12].copy_from_slice(&lane.to_le_bytes());
    // Bytes 12 to 18 are the group and the flags of the item, which a fresh
    // clip leaves at zero.
    bytes[18..22].copy_from_slice(&(offset as f32).to_le_bytes());
    bytes[26..30].copy_from_slice(&((offset + length) as f32).to_le_bytes());
    bytes
}

/// Ticks a run of frames lasts.
fn ticks(session: &Session, frames: usize) -> u32 {
    let ticks = session.beats(frames) * f64::from(PPQ);
    if ticks <= 0.0 {
        0
    } else {
        ticks.round() as u32
    }
}

/// Frames a run of ticks lasts.
fn frames_of(session: &Session, ticks: u32) -> usize {
    if session.tempo <= 0.0 {
        return 0;
    }
    let seconds = f64::from(ticks) / f64::from(PPQ) * 60.0 / session.tempo;
    (seconds * f64::from(session.sample_rate)).round().max(0.0) as usize
}

/// Reads an `.flp` file back into a session.
pub fn read_file(path: impl AsRef<Path>) -> Result<Session> {
    read(&fs::read(path)?)
}

/// Reads the bytes of an `.flp` file back into a session.
pub fn read(bytes: &[u8]) -> Result<Session> {
    if bytes.len() < 8 || &bytes[0..4] != HEADER_MAGIC {
        return Err(Error::Format(
            "this is not an FL Studio project".to_string(),
        ));
    }
    let header = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let mut at = 8 + header;
    if bytes.len() < at + 8 || &bytes[at..at + 4] != DATA_MAGIC {
        return Err(Error::Format("the project holds no events".to_string()));
    }
    let length =
        u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
    at += 8;
    let end = (at + length).min(bytes.len());
    parse(&events(&bytes[at..end])?)
}

/// Reads a list of events back into a session.
pub fn parse(events: &[Event]) -> Result<Session> {
    let mut session = Session {
        name: String::new(),
        sample_rate: 44_100,
        tempo: crate::formats::session::DEFAULT_TEMPO,
        frames: 0,
        channels: 2,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };
    let mut names: Vec<String> = Vec::new();
    let mut patterns: Vec<(String, Vec<u8>)> = Vec::new();
    let mut items: Vec<u8> = Vec::new();
    let mut channel = String::new();

    for event in events {
        match event {
            Event::Dword(id::SAMPLE_RATE, rate) if *rate > 0 => session.sample_rate = *rate,
            Event::Dword(id::FINE_TEMPO, tempo) => session.tempo = f64::from(*tempo) / 1000.0,
            Event::Data(id::TITLE, _) => session.name = event.as_text().unwrap_or_default(),
            Event::Data(id::CHANNEL_NAME, _) => channel = event.as_text().unwrap_or_default(),
            Event::Data(id::SAMPLE_PATH, _) => {
                let file = event.as_text().unwrap_or_default();
                session
                    .instruments
                    .push(crate::formats::session::Instrument {
                        id: session.instruments.len(),
                        name: if channel.is_empty() {
                            file.clone()
                        } else {
                            channel.clone()
                        },
                        file,
                        note: None,
                        frames: 0,
                        peak: 1.0,
                    });
                channel.clear();
            }
            Event::Data(id::PATTERN_NAME, _) => {
                patterns.push((event.as_text().unwrap_or_default(), Vec::new()));
            }
            Event::Data(id::PATTERN_NOTES, bytes) => {
                if let Some(pattern) = patterns.last_mut() {
                    pattern.1.clone_from(bytes);
                } else {
                    patterns.push((String::new(), bytes.clone()));
                }
            }
            Event::Data(id::PLAYLIST, bytes) => items.clone_from(bytes),
            Event::Data(id::TRACK_NAME, _) => names.push(event.as_text().unwrap_or_default()),
            _ => {}
        }
    }

    read_playlist(&mut session, &items, &names, &patterns);
    session.frames = session.end();
    Ok(session)
}

/// Turns the playlist back into tracks of clips and notes.
fn read_playlist(
    session: &mut Session,
    items: &[u8],
    names: &[String],
    patterns: &[(String, Vec<u8>)],
) {
    let copy = Session {
        tracks: Vec::new(),
        instruments: Vec::new(),
        ..session.clone()
    };
    for bytes in items.chunks_exact(ITEM_SIZE) {
        let position = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let source = u16::from_le_bytes([bytes[4], bytes[5]]);
        let length = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
        let lane = u16::from_le_bytes([bytes[10], bytes[11]]);
        let index = usize::from(TRACK_BASE.saturating_sub(lane));
        let name = names.get(index).cloned().unwrap_or_default();

        if source >= PATTERN_BASE {
            let pattern = usize::from(source - PATTERN_BASE);
            let notes = patterns
                .get(pattern)
                .map(|pattern| read_notes(&copy, &pattern.1, frames_of(&copy, position)))
                .unwrap_or_default();
            place(
                session,
                index,
                Track {
                    name,
                    kind: TrackKind::Instrument,
                    clips: Vec::new(),
                    notes,
                },
            );
        } else {
            let offset = f32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
            let instrument = session.instruments.get(usize::from(source));
            let clip = Clip {
                name: instrument
                    .map(|instrument| instrument.name.clone())
                    .unwrap_or_default(),
                file: instrument
                    .map(|instrument| instrument.file.clone())
                    .unwrap_or_default(),
                start: frames_of(&copy, position),
                length: frames_of(&copy, length),
                offset: frames_of(&copy, offset.max(0.0) as u32),
                gain: 1.0,
                channel: 0,
            };
            let track = Track {
                name,
                kind: TrackKind::Audio,
                clips: Vec::new(),
                notes: Vec::new(),
            };
            place(session, index, track).clips.push(clip);
        }
    }
}

/// Finds the track a playlist item belongs on, making it if it is new.
fn place(session: &mut Session, index: usize, track: Track) -> &mut Track {
    while session.tracks.len() <= index {
        session.tracks.push(Track {
            name: String::new(),
            kind: track.kind,
            clips: Vec::new(),
            notes: Vec::new(),
        });
    }
    let found = &mut session.tracks[index];
    if found.name.is_empty() {
        found.name = track.name;
    }
    found.kind = track.kind;
    found.notes.extend(track.notes);
    found
}

/// Reads the notes of a pattern.
fn read_notes(session: &Session, bytes: &[u8], start: usize) -> Vec<Note> {
    let mut out = Vec::new();
    for note in bytes.chunks_exact(NOTE_SIZE) {
        let at = u32::from_le_bytes([note[0], note[1], note[2], note[3]]);
        let length = u32::from_le_bytes([note[8], note[9], note[10], note[11]]);
        let pitch = note[12];
        out.push(Note {
            channel: 0,
            start: start + frames_of(session, at),
            length: frames_of(session, length),
            note: pitch,
            velocity: note[19],
            frequency: 440.0 * ((f64::from(pitch) - 69.0) / 12.0).exp2(),
            confidence: 1.0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::session::Instrument;

    /// One beat at 120 bpm and 48 kHz.
    const BEAT: usize = 24_000;

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
            name: "song".to_string(),
            sample_rate: 48_000,
            tempo: 120.0,
            frames: 8 * BEAT,
            channels: 2,
            tracks: vec![
                Track {
                    name: "kick".to_string(),
                    kind: TrackKind::Audio,
                    clips: vec![
                        Clip {
                            name: "kick".to_string(),
                            file: "samples/kick.wav".to_string(),
                            start: 0,
                            length: BEAT,
                            offset: 0,
                            gain: 1.0,
                            channel: 0,
                        },
                        Clip {
                            name: "kick".to_string(),
                            file: "samples/kick.wav".to_string(),
                            start: 4 * BEAT,
                            length: BEAT,
                            offset: 0,
                            gain: 1.0,
                            channel: 0,
                        },
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
        let expected = session();

        let read = read(&write(&expected)).unwrap();

        assert_eq!(read.name, "song");
        assert_eq!(read.sample_rate, 48_000);
        assert!((read.tempo - 120.0).abs() < 1e-9);
        assert_eq!(read.instruments.len(), 1);
        assert_eq!(read.instruments[0].file, "samples/kick.wav");
        assert_eq!(read.tracks.len(), 2);
        assert_eq!(read.tracks[0].clips, expected.tracks[0].clips);
        assert_eq!(read.tracks[0].name, "kick");
        assert_eq!(read.tracks[1].notes, expected.tracks[1].notes);
        assert_eq!(read.tracks[1].name, "notes");
    }

    #[test]
    fn the_file_starts_the_way_a_project_has_to() {
        let bytes = write(&session());

        assert_eq!(&bytes[0..4], HEADER_MAGIC);
        assert_eq!(&bytes[4..8], &6u32.to_le_bytes());
        assert_eq!(&bytes[8..10], &0i16.to_le_bytes());
        // One sample in the bank, one channel in the rack.
        assert_eq!(&bytes[10..12], &1u16.to_le_bytes());
        assert_eq!(&bytes[12..14], &PPQ.to_le_bytes());
        assert_eq!(&bytes[14..18], DATA_MAGIC);
        let length = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
        assert_eq!(length, bytes.len() - 22);
    }

    #[test]
    fn every_event_keeps_its_size_and_its_bytes() {
        let expected = vec![
            Event::Byte(id::CHANNEL_TYPE, 3),
            Event::Word(id::NEW_CHANNEL, 0xbeef),
            Event::Dword(id::FINE_TEMPO, 140_000),
            Event::text(id::TITLE, "a song"),
            // A block long enough to need two bytes of length.
            Event::Data(id::PLAYLIST, vec![7; 300]),
        ];

        let mut bytes = Vec::new();
        for event in &expected {
            event.write(&mut bytes);
        }

        assert_eq!(events(&bytes).unwrap(), expected);
    }

    #[test]
    fn unknown_events_are_stepped_over_not_stumbled_on() {
        // An identifier this module says nothing about, of every size class.
        let mut bytes = Vec::new();
        for event in [
            Event::Byte(7, 1),
            Event::Word(99, 2),
            Event::Dword(140, 3),
            Event::Data(250, vec![0xff; 5]),
            Event::Dword(id::FINE_TEMPO, 90_500),
        ] {
            event.write(&mut bytes);
        }

        let read = parse(&events(&bytes).unwrap()).unwrap();

        assert!((read.tempo - 90.5).abs() < 1e-9);
    }

    #[test]
    fn text_is_read_whether_it_is_wide_or_not() {
        assert_eq!(Event::text(id::TITLE, "hi").as_text().unwrap(), "hi");
        // FL Studio 11 and earlier wrote plain bytes.
        assert_eq!(
            Event::Data(id::TITLE, b"hi\0".to_vec()).as_text().unwrap(),
            "hi"
        );
        assert_eq!(Event::Byte(0, 0).as_text(), None);
    }

    #[test]
    fn notes_land_on_the_tick_grid_the_header_promises() {
        let session = session();
        let events = project(&session);
        let Event::Data(_, bytes) = events
            .iter()
            .find(|event| event.id() == id::PATTERN_NOTES)
            .unwrap()
        else {
            panic!("the pattern holds no notes");
        };

        assert_eq!(bytes.len(), 2 * NOTE_SIZE);
        // The second note starts two beats in, which is two times the ppq.
        let at = u32::from_le_bytes([
            bytes[NOTE_SIZE],
            bytes[NOTE_SIZE + 1],
            bytes[NOTE_SIZE + 2],
            bytes[NOTE_SIZE + 3],
        ]);
        assert_eq!(at, u32::from(PPQ) * 2);
        assert_eq!(bytes[NOTE_SIZE + 12], 67);
        assert_eq!(bytes[NOTE_SIZE + 19], 80);
    }

    #[test]
    fn broken_projects_are_reported() {
        let complain = |bytes: &[u8]| read(bytes).unwrap_err().to_string();

        assert!(complain(b"nope").contains("not an FL Studio project"));
        assert!(complain(b"FLhd\x06\0\0\0\0\0\x01\0\x60\0").contains("no events"));
        // A data event whose length promises more than the chunk holds.
        assert!(events(&[192, 0x0a, 1, 2])
            .unwrap_err()
            .to_string()
            .contains("past the end"));
        assert!(events(&[192, 0x80, 0x80, 0x80, 0x80, 0x80])
            .unwrap_err()
            .to_string()
            .contains("never ends"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-flp-{}-{:?}/song.flp",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session()).unwrap();
        let read = read_file(&path).unwrap();

        assert_eq!(read.tracks.len(), 2);
        assert_eq!(read.name, "song");

        let _ = fs::remove_dir_all(&root);
    }
}
