//! Standard MIDI files, written and read back.
//!
//! MIDI is the one interchange format every program named in the issue
//! understands, so it is the first export and the one the round trip is
//! measured against. The encoder and the decoder are both here because a
//! format that can only be written cannot be tested: every test in this module
//! writes a file, reads it back, and compares.
//!
//! Timing is chosen so that one tick is exactly one audio frame. A standard
//! MIDI file names a tempo in microseconds per quarter note and a division in
//! ticks per quarter note, and their ratio is the duration of a tick; picking
//! a pair whose ratio is `1 / sample_rate` makes note positions survive the
//! trip through MIDI without rounding. See [`timing`].

use std::path::Path;

use crate::decompose::model::Note;
use crate::dsp::pitch::midi_to_frequency;
use crate::error::{Error, Result};

/// Highest division a standard MIDI file can express in ticks per quarter.
const MAX_DIVISION: u32 = 0x7FFF;

/// Longest tempo a standard MIDI file can express, in microseconds per quarter.
const MAX_TEMPO: u32 = 0x00FF_FFFF;

/// A channel voice message or a meta event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Starts a note.
    NoteOn {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Note number.
        note: u8,
        /// Velocity; zero means the same as a note off.
        velocity: u8,
    },
    /// Ends a note.
    NoteOff {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Note number.
        note: u8,
        /// Release velocity.
        velocity: u8,
    },
    /// Sets a controller.
    ControlChange {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Controller number.
        controller: u8,
        /// Controller value.
        value: u8,
    },
    /// Selects an instrument.
    ProgramChange {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Program number.
        program: u8,
    },
    /// Pressure applied to one held key after it sounded.
    Aftertouch {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Note number the pressure applies to.
        note: u8,
        /// Pressure.
        pressure: u8,
    },
    /// Pressure applied to every held key of a channel.
    ChannelPressure {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Pressure.
        pressure: u8,
    },
    /// Bends every sounding note of a channel.
    PitchBend {
        /// MIDI channel, `0..16`.
        channel: u8,
        /// Fourteen-bit bend amount; `0x2000` is the centre.
        value: u16,
    },
    /// Names the track.
    TrackName(String),
    /// Names the instrument the track is played on.
    InstrumentName(String),
    /// A free-form text event.
    Text(String),
    /// Microseconds per quarter note.
    Tempo(u32),
    /// A time signature: `numerator / 2^denominator`.
    TimeSignature {
        /// Beats per bar.
        numerator: u8,
        /// Power of two the beat is expressed in; `2` means a quarter.
        denominator: u8,
        /// MIDI clocks per metronome click.
        clocks_per_click: u8,
        /// Notated 32nd notes per quarter note.
        thirty_seconds_per_quarter: u8,
    },
    /// Ends the track.
    EndOfTrack,
    /// A meta event this crate does not model, kept so files round trip.
    Meta {
        /// Meta event type byte, `0x00..0x80`.
        kind: u8,
        /// Payload.
        data: Vec<u8>,
    },
    /// A system exclusive message, kept as its raw payload.
    SysEx {
        /// Status byte the message was introduced with, `0xF0` or `0xF7`.
        status: u8,
        /// Payload.
        data: Vec<u8>,
    },
}

/// A message and the number of ticks between it and the previous one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Ticks since the previous event of the track.
    pub delta: u32,
    /// What happens.
    pub message: Message,
}

impl Event {
    /// An event from its parts.
    #[must_use]
    pub const fn new(delta: u32, message: Message) -> Self {
        Self { delta, message }
    }
}

/// One track chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// The events of the track, in order.
    pub events: Vec<Event>,
}

impl Track {
    /// A track from its events.
    #[must_use]
    pub const fn new(events: Vec<Event>) -> Self {
        Self { events }
    }

    /// The name of the track, when it carries one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.events.iter().find_map(|event| match &event.message {
            Message::TrackName(name) => Some(name.as_str()),
            _ => None,
        })
    }
}

/// A standard MIDI file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiFile {
    /// `0` single track, `1` parallel tracks, `2` independent sequences.
    pub format: u16,
    /// Ticks per quarter note.
    pub division: u16,
    /// The tracks.
    pub tracks: Vec<Track>,
}

impl Default for MidiFile {
    fn default() -> Self {
        Self {
            format: 1,
            division: 960,
            tracks: Vec::new(),
        }
    }
}

impl MidiFile {
    /// The tempo the file opens with, in microseconds per quarter note.
    #[must_use]
    pub fn tempo(&self) -> Option<u32> {
        self.tracks.iter().find_map(|track| {
            track.events.iter().find_map(|event| match event.message {
                Message::Tempo(tempo) => Some(tempo),
                _ => None,
            })
        })
    }
}

/// A tempo and division whose ratio is one audio frame.
///
/// The duration of a tick is `tempo / division` microseconds, and one frame is
/// `1_000_000 / sample_rate` microseconds, so a tempo is usable when
/// `sample_rate * tempo` is a whole number of millions and the division that
/// falls out of it fits in the header. The candidates are ordinary musical
/// tempi first, so the exported file still looks like music to a DAW.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] when no candidate fits, which happens only
/// for sample rates that are not a multiple of 25 hertz.
pub fn timing(sample_rate: u32) -> Result<(u32, u16)> {
    const CANDIDATES: [u32; 8] = [
        500_000, 250_000, 1_000_000, 125_000, 2_000_000, 4_000_000, 8_000_000, 16_000_000,
    ];
    for tempo in CANDIDATES {
        let product = u64::from(sample_rate) * u64::from(tempo);
        if product % 1_000_000 != 0 {
            continue;
        }
        let division = product / 1_000_000;
        if (1..=u64::from(MAX_DIVISION)).contains(&division) && tempo <= MAX_TEMPO {
            return Ok((tempo, division as u16));
        }
    }
    Err(Error::Unsupported(format!(
        "no frame-exact MIDI timing exists for {sample_rate} Hz"
    )))
}

/// Converts a tick position to a frame position under a given timing.
#[must_use]
pub fn ticks_to_frames(ticks: u32, tempo: u32, division: u16, sample_rate: u32) -> u64 {
    let divisor = u128::from(division) * 1_000_000;
    if divisor == 0 {
        return 0;
    }
    let frames = u128::from(ticks) * u128::from(tempo) * u128::from(sample_rate) / divisor;
    frames as u64
}

/// Converts a frame position to a tick position under a given timing.
#[must_use]
pub fn frames_to_ticks(frames: u64, tempo: u32, division: u16, sample_rate: u32) -> u32 {
    let divisor = u128::from(tempo) * u128::from(sample_rate);
    if divisor == 0 {
        return 0;
    }
    let ticks = u128::from(frames) * u128::from(division) * 1_000_000 / divisor;
    ticks.min(u128::from(u32::MAX)) as u32
}

/// Builds a MIDI file from recognised notes, one track per audio channel.
///
/// The first track carries the tempo map, as format 1 requires. Notes keep
/// their frame positions exactly, because [`timing`] makes a tick a frame.
pub fn from_notes(notes: &[Note], sample_rate: u32, name: &str) -> Result<MidiFile> {
    let (tempo, division) = timing(sample_rate)?;
    let mut file = MidiFile {
        format: 1,
        division,
        tracks: vec![Track::new(vec![
            Event::new(0, Message::TrackName(name.to_string())),
            Event::new(0, Message::Tempo(tempo)),
            Event::new(
                0,
                Message::TimeSignature {
                    numerator: 4,
                    denominator: 2,
                    clocks_per_click: 24,
                    thirty_seconds_per_quarter: 8,
                },
            ),
            Event::new(0, Message::EndOfTrack),
        ])],
    };

    let channels = notes.iter().map(|note| note.channel).max();
    for channel in 0..=channels.unwrap_or(0) {
        if channels.is_none() {
            break;
        }
        let mut moments: Vec<(u32, u8, Message)> = Vec::new();
        for note in notes.iter().filter(|note| note.channel == channel) {
            let voice = (channel % 16) as u8;
            let start = frames_to_ticks(note.start as u64, tempo, division, sample_rate);
            let end = frames_to_ticks(
                (note.start + note.length) as u64,
                tempo,
                division,
                sample_rate,
            );
            moments.push((
                start,
                1,
                Message::NoteOn {
                    channel: voice,
                    note: note.note.min(127),
                    velocity: note.velocity.min(127),
                },
            ));
            moments.push((
                end.max(start),
                0,
                Message::NoteOff {
                    channel: voice,
                    note: note.note.min(127),
                    velocity: 0,
                },
            ));
        }
        // Note offs come before note ons at the same tick, so a repeat of the
        // same pitch ends before it starts again.
        moments.sort_by_key(|(tick, kind, _)| (*tick, *kind));

        let mut events = vec![Event::new(
            0,
            Message::TrackName(format!("channel-{channel}")),
        )];
        let mut previous = 0;
        for (tick, _, message) in moments {
            events.push(Event::new(tick - previous, message));
            previous = tick;
        }
        events.push(Event::new(0, Message::EndOfTrack));
        file.tracks.push(Track::new(events));
    }
    Ok(file)
}

/// Reads notes back out of a MIDI file.
///
/// Each track becomes one audio channel, in track order, skipping a leading
/// tempo track that holds no notes. The frequency and confidence a
/// [`Note`] carries are not part of MIDI, so they are derived from the note
/// number and reported as certain.
#[must_use]
pub fn to_notes(file: &MidiFile, sample_rate: u32) -> Vec<Note> {
    let tempo = file.tempo().unwrap_or(500_000);
    let mut notes = Vec::new();
    let mut channel = 0;
    for track in &file.tracks {
        let mut sounding: Vec<(u8, u32, u8)> = Vec::new();
        let mut tick = 0_u32;
        let mut heard = false;
        for event in &track.events {
            tick = tick.saturating_add(event.delta);
            let (note, velocity, starting) = match event.message {
                Message::NoteOn { note, velocity, .. } if velocity > 0 => (note, velocity, true),
                // A note on with velocity zero is a note off, as the standard
                // allows so that running status can carry a whole phrase.
                Message::NoteOn { note, .. } | Message::NoteOff { note, .. } => (note, 0, false),
                _ => continue,
            };
            heard = true;
            if starting {
                sounding.push((note, tick, velocity));
                continue;
            }
            if let Some(index) = sounding.iter().rposition(|(open, _, _)| *open == note) {
                let (_, start, velocity) = sounding.remove(index);
                let from = ticks_to_frames(start, tempo, file.division, sample_rate) as usize;
                let to = ticks_to_frames(tick, tempo, file.division, sample_rate) as usize;
                notes.push(Note {
                    channel,
                    start: from,
                    length: to.saturating_sub(from),
                    note,
                    velocity,
                    frequency: midi_to_frequency(f64::from(note)),
                    confidence: 1.0,
                });
            }
        }
        if heard {
            channel += 1;
        }
    }
    notes.sort_by_key(|note| (note.channel, note.start, note.note));
    notes
}

/// Encodes a MIDI file into bytes.
#[must_use]
pub fn encode(file: &MidiFile) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&6_u32.to_be_bytes());
    bytes.extend_from_slice(&file.format.to_be_bytes());
    bytes.extend_from_slice(&(file.tracks.len() as u16).to_be_bytes());
    bytes.extend_from_slice(&file.division.to_be_bytes());

    for track in &file.tracks {
        let mut body = Vec::new();
        let mut running = Running::default();
        let mut ended = false;
        for event in &track.events {
            write_vlq(&mut body, event.delta);
            write_message(&mut body, &event.message, &mut running);
            ended = event.message == Message::EndOfTrack;
        }
        if !ended {
            write_vlq(&mut body, 0);
            write_message(&mut body, &Message::EndOfTrack, &mut running);
        }
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&body);
    }
    bytes
}

/// Decodes a MIDI file from bytes.
pub fn decode(bytes: &[u8]) -> Result<MidiFile> {
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != b"MThd" {
        return Err(Error::Format("not a standard MIDI file".to_string()));
    }
    let header = reader.u32()?;
    if header < 6 {
        return Err(Error::Format(format!("MIDI header of {header} bytes")));
    }
    let format = reader.u16()?;
    let count = reader.u16()?;
    let division = reader.u16()?;
    reader.skip((header - 6) as usize)?;

    let mut tracks = Vec::with_capacity(count as usize);
    while reader.remaining() >= 8 {
        let kind = reader.take(4)?.to_vec();
        let length = reader.u32()? as usize;
        if kind != b"MTrk" {
            reader.skip(length)?;
            continue;
        }
        let body = reader.take(length)?.to_vec();
        tracks.push(read_track(&body)?);
    }
    Ok(MidiFile {
        format,
        division,
        tracks,
    })
}

/// Writes a MIDI file to disk.
pub fn write_file(path: impl AsRef<Path>, file: &MidiFile) -> Result<()> {
    std::fs::write(path, encode(file)).map_err(Error::Io)
}

/// Reads a MIDI file from disk.
pub fn read_file(path: impl AsRef<Path>) -> Result<MidiFile> {
    decode(&std::fs::read(path).map_err(Error::Io)?)
}

fn write_vlq(bytes: &mut Vec<u8>, value: u32) {
    let mut buffer = [0_u8; 5];
    let mut length = 0;
    let mut rest = value;
    loop {
        buffer[length] = (rest & 0x7F) as u8;
        length += 1;
        rest >>= 7;
        if rest == 0 {
            break;
        }
    }
    for index in (0..length).rev() {
        let last = index == 0;
        bytes.push(buffer[index] | if last { 0 } else { 0x80 });
    }
}

/// The status byte a track is currently running from.
///
/// A standard MIDI file may leave out the status byte of a channel message
/// when it repeats the previous one. Writing that abbreviation keeps exported
/// files the same size as the ones DAWs write, and — more usefully here —
/// makes encoding the inverse of decoding, so a file read from disk is written
/// back byte for byte.
#[derive(Clone, Copy, Debug, Default)]
struct Running(Option<u8>);

impl Running {
    /// Emits `status` unless it is already the running one.
    fn status(&mut self, bytes: &mut Vec<u8>, status: u8) {
        if self.0 != Some(status) {
            bytes.push(status);
            self.0 = Some(status);
        }
    }

    /// Cancels the abbreviation, as every system message does.
    const fn clear(&mut self) {
        self.0 = None;
    }
}

fn write_meta(bytes: &mut Vec<u8>, kind: u8, data: &[u8]) {
    bytes.push(0xFF);
    bytes.push(kind);
    write_vlq(bytes, data.len() as u32);
    bytes.extend_from_slice(data);
}

fn write_message(bytes: &mut Vec<u8>, message: &Message, running: &mut Running) {
    match message {
        Message::NoteOff {
            channel,
            note,
            velocity,
        } => {
            running.status(bytes, 0x80 | (channel & 0x0F));
            bytes.extend_from_slice(&[*note, *velocity]);
        }
        Message::NoteOn {
            channel,
            note,
            velocity,
        } => {
            running.status(bytes, 0x90 | (channel & 0x0F));
            bytes.extend_from_slice(&[*note, *velocity]);
        }
        Message::Aftertouch {
            channel,
            note,
            pressure,
        } => {
            running.status(bytes, 0xA0 | (channel & 0x0F));
            bytes.extend_from_slice(&[*note, *pressure]);
        }
        Message::ControlChange {
            channel,
            controller,
            value,
        } => {
            running.status(bytes, 0xB0 | (channel & 0x0F));
            bytes.extend_from_slice(&[*controller, *value]);
        }
        Message::ProgramChange { channel, program } => {
            running.status(bytes, 0xC0 | (channel & 0x0F));
            bytes.push(*program);
        }
        Message::ChannelPressure { channel, pressure } => {
            running.status(bytes, 0xD0 | (channel & 0x0F));
            bytes.push(*pressure);
        }
        Message::PitchBend { channel, value } => {
            running.status(bytes, 0xE0 | (channel & 0x0F));
            bytes.extend_from_slice(&[(value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8]);
        }
        Message::Text(text) => {
            running.clear();
            write_meta(bytes, 0x01, text.as_bytes());
        }
        Message::TrackName(name) => {
            running.clear();
            write_meta(bytes, 0x03, name.as_bytes());
        }
        Message::InstrumentName(name) => {
            running.clear();
            write_meta(bytes, 0x04, name.as_bytes());
        }
        Message::Tempo(tempo) => {
            running.clear();
            write_meta(bytes, 0x51, &tempo.to_be_bytes()[1..]);
        }
        Message::TimeSignature {
            numerator,
            denominator,
            clocks_per_click,
            thirty_seconds_per_quarter,
        } => {
            running.clear();
            write_meta(
                bytes,
                0x58,
                &[
                    *numerator,
                    *denominator,
                    *clocks_per_click,
                    *thirty_seconds_per_quarter,
                ],
            );
        }
        Message::EndOfTrack => {
            running.clear();
            write_meta(bytes, 0x2F, &[]);
        }
        Message::Meta { kind, data } => {
            running.clear();
            write_meta(bytes, *kind, data);
        }
        Message::SysEx { status, data } => {
            running.clear();
            bytes.push(*status);
            write_vlq(bytes, data.len() as u32);
            bytes.extend_from_slice(data);
        }
    }
}

/// A cursor over a byte slice that reports what it ran out of.
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| Error::Format("MIDI file ends in the middle of a chunk".to_string()))?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn skip(&mut self, length: usize) -> Result<()> {
        self.take(length).map(|_| ())
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn vlq(&mut self) -> Result<u32> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Format(
            "variable-length quantity longer than four bytes".to_string(),
        ))
    }
}

fn read_track(body: &[u8]) -> Result<Track> {
    let mut reader = Reader::new(body);
    let mut events = Vec::new();
    let mut running: Option<u8> = None;
    while reader.remaining() > 0 {
        let delta = reader.vlq()?;
        let next = reader
            .peek()
            .ok_or_else(|| Error::Format("MIDI track ends after a delta time".to_string()))?;
        let status = if next < 0x80 {
            // Running status: the data bytes follow the previous status byte,
            // which is not repeated.
            running.ok_or_else(|| {
                Error::Format("MIDI event without a status byte to run from".to_string())
            })?
        } else {
            reader.skip(1)?;
            next
        };
        // Only channel messages may be run from; a system message cancels the
        // abbreviation, exactly as the encoder assumes.
        running = (status < 0xF0).then_some(status);
        let message = read_message(&mut reader, status)?;
        let end = message == Message::EndOfTrack;
        events.push(Event::new(delta, message));
        if end {
            break;
        }
    }
    Ok(Track::new(events))
}

fn read_message(reader: &mut Reader<'_>, status: u8) -> Result<Message> {
    let channel = status & 0x0F;
    match status & 0xF0 {
        0x80 => Ok(Message::NoteOff {
            channel,
            note: reader.byte()?,
            velocity: reader.byte()?,
        }),
        0x90 => Ok(Message::NoteOn {
            channel,
            note: reader.byte()?,
            velocity: reader.byte()?,
        }),
        0xA0 => Ok(Message::Aftertouch {
            channel,
            note: reader.byte()?,
            pressure: reader.byte()?,
        }),
        0xB0 => Ok(Message::ControlChange {
            channel,
            controller: reader.byte()?,
            value: reader.byte()?,
        }),
        0xC0 => Ok(Message::ProgramChange {
            channel,
            program: reader.byte()?,
        }),
        0xD0 => Ok(Message::ChannelPressure {
            channel,
            pressure: reader.byte()?,
        }),
        0xE0 => {
            let low = reader.byte()?;
            let high = reader.byte()?;
            Ok(Message::PitchBend {
                channel,
                value: (u16::from(high & 0x7F) << 7) | u16::from(low & 0x7F),
            })
        }
        _ => read_system(reader, status),
    }
}

fn read_system(reader: &mut Reader<'_>, status: u8) -> Result<Message> {
    match status {
        0xF0 | 0xF7 => {
            let length = reader.vlq()? as usize;
            Ok(Message::SysEx {
                status,
                data: reader.take(length)?.to_vec(),
            })
        }
        0xFF => {
            let kind = reader.byte()?;
            let length = reader.vlq()? as usize;
            let data = reader.take(length)?.to_vec();
            Ok(meta_message(kind, data))
        }
        other => Err(Error::Format(format!(
            "unexpected MIDI status byte {other:#04X}"
        ))),
    }
}

fn meta_message(kind: u8, data: Vec<u8>) -> Message {
    let text = || String::from_utf8_lossy(&data).into_owned();
    match kind {
        0x01 => Message::Text(text()),
        0x03 => Message::TrackName(text()),
        0x04 => Message::InstrumentName(text()),
        0x2F => Message::EndOfTrack,
        0x51 if data.len() == 3 => Message::Tempo(
            (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]),
        ),
        0x58 if data.len() == 4 => Message::TimeSignature {
            numerator: data[0],
            denominator: data[1],
            clocks_per_click: data[2],
            thirty_seconds_per_quarter: data[3],
        },
        _ => Message::Meta { kind, data },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(channel: usize, start: usize, length: usize, number: u8, velocity: u8) -> Note {
        Note {
            channel,
            start,
            length,
            note: number,
            velocity,
            frequency: midi_to_frequency(f64::from(number)),
            confidence: 1.0,
        }
    }

    #[test]
    fn variable_length_quantities_round_trip() {
        for value in [0, 1, 0x7F, 0x80, 0x2000, 0x3FFF, 0x1F_FFFF, 0x0FFF_FFFF] {
            let mut bytes = Vec::new();
            write_vlq(&mut bytes, value);
            let mut reader = Reader::new(&bytes);
            assert_eq!(reader.vlq().unwrap(), value, "value {value}");
            assert_eq!(reader.remaining(), 0);
        }
        assert_eq!(
            {
                let mut bytes = Vec::new();
                write_vlq(&mut bytes, 0x3FFF);
                bytes
            },
            vec![0xFF, 0x7F]
        );
        assert!(Reader::new(&[0x80, 0x80, 0x80, 0x80]).vlq().is_err());
    }

    #[test]
    fn a_tick_is_exactly_one_frame() {
        for rate in [
            8_000, 11_025, 22_050, 44_100, 48_000, 88_200, 96_000, 192_000,
        ] {
            let (tempo, division) = timing(rate).unwrap();
            assert_eq!(
                u64::from(rate) * u64::from(tempo) / 1_000_000,
                u64::from(division),
                "rate {rate}"
            );
            for frames in [0_u64, 1, 4095, 1_000_000] {
                let ticks = frames_to_ticks(frames, tempo, division, rate);
                assert_eq!(u64::from(ticks), frames, "rate {rate} frames {frames}");
                assert_eq!(ticks_to_frames(ticks, tempo, division, rate), frames);
            }
        }
        assert!(timing(44_101).is_err());
        assert_eq!(ticks_to_frames(10, 500_000, 0, 44_100), 0);
        assert_eq!(frames_to_ticks(10, 0, 960, 44_100), 0);
    }

    #[test]
    fn notes_survive_the_trip_through_a_midi_file() {
        let notes = vec![
            note(0, 0, 22_050, 57, 100),
            note(0, 22_050, 11_025, 60, 80),
            note(0, 22_050, 11_025, 64, 80),
            note(1, 4_096, 8_192, 45, 127),
        ];
        let file = from_notes(&notes, 44_100, "fixture").unwrap();
        let bytes = encode(&file);
        let decoded = decode(&bytes).unwrap();

        assert_eq!(decoded, file);
        assert_eq!(decoded.tracks.len(), 3);
        assert_eq!(decoded.tracks[0].name(), Some("fixture"));
        assert_eq!(decoded.tracks[1].name(), Some("channel-0"));
        assert_eq!(decoded.tempo(), Some(500_000));
        assert_eq!(to_notes(&decoded, 44_100), notes);
    }

    #[test]
    fn a_repeated_pitch_ends_before_it_starts_again() {
        let notes = vec![note(0, 0, 1_000, 60, 90), note(0, 1_000, 1_000, 60, 90)];
        let file = from_notes(&notes, 44_100, "repeat").unwrap();
        let track = &file.tracks[1];
        let kinds: Vec<&Message> = track.events.iter().map(|event| &event.message).collect();
        let positions: Vec<usize> = kinds
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, Message::NoteOn { .. }))
            .map(|(index, _)| index)
            .collect();
        let offs: Vec<usize> = kinds
            .iter()
            .enumerate()
            .filter(|(_, message)| matches!(message, Message::NoteOff { .. }))
            .map(|(index, _)| index)
            .collect();

        assert_eq!(positions.len(), 2);
        assert!(offs[0] < positions[1], "the first note must end first");
        assert_eq!(to_notes(&file, 44_100), notes);
    }

    #[test]
    fn an_empty_score_still_produces_a_playable_file() {
        let file = from_notes(&[], 48_000, "empty").unwrap();
        assert_eq!(file.tracks.len(), 1);
        let decoded = decode(&encode(&file)).unwrap();
        assert_eq!(decoded, file);
        assert!(to_notes(&decoded, 48_000).is_empty());
    }

    #[test]
    fn running_status_and_unknown_events_are_understood() {
        let mut body = Vec::new();
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0x90, 60, 100]);
        write_vlq(&mut body, 96);
        body.extend_from_slice(&[62, 100]); // running status
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0x80, 60, 0]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[62, 0]); // running status again
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xF0, 2, 0x7E, 0xF7]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xE0, 0, 64]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xA0, 60, 88]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xD0, 77]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xFF, 0x7F, 3, 1, 2, 3]);
        write_vlq(&mut body, 0);
        body.extend_from_slice(&[0xFF, 0x2F, 0]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u16.to_be_bytes());
        bytes.extend_from_slice(&1_u16.to_be_bytes());
        bytes.extend_from_slice(&960_u16.to_be_bytes());
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&body);

        let file = decode(&bytes).unwrap();
        let events = &file.tracks[0].events;
        assert_eq!(
            events[1],
            Event::new(
                96,
                Message::NoteOn {
                    channel: 0,
                    note: 62,
                    velocity: 100
                }
            )
        );
        assert_eq!(
            events[4].message,
            Message::SysEx {
                status: 0xF0,
                data: vec![0x7E, 0xF7]
            }
        );
        assert_eq!(
            events[5].message,
            Message::PitchBend {
                channel: 0,
                value: 8_192
            }
        );
        assert_eq!(
            events[6].message,
            Message::Aftertouch {
                channel: 0,
                note: 60,
                pressure: 88
            }
        );
        assert_eq!(
            events[7].message,
            Message::ChannelPressure {
                channel: 0,
                pressure: 77
            }
        );
        assert_eq!(
            events[8].message,
            Message::Meta {
                kind: 0x7F,
                data: vec![1, 2, 3]
            }
        );
        assert_eq!(encode(&file), bytes);
    }

    #[test]
    fn every_message_this_crate_writes_can_be_read_again() {
        let file = MidiFile {
            format: 1,
            division: 480,
            tracks: vec![Track::new(vec![
                Event::new(0, Message::TrackName("name".to_string())),
                Event::new(0, Message::InstrumentName("piano".to_string())),
                Event::new(0, Message::Text("hello".to_string())),
                Event::new(0, Message::Tempo(600_000)),
                Event::new(
                    0,
                    Message::TimeSignature {
                        numerator: 3,
                        denominator: 2,
                        clocks_per_click: 24,
                        thirty_seconds_per_quarter: 8,
                    },
                ),
                Event::new(
                    0,
                    Message::ProgramChange {
                        channel: 2,
                        program: 40,
                    },
                ),
                Event::new(
                    0,
                    Message::ControlChange {
                        channel: 2,
                        controller: 7,
                        value: 100,
                    },
                ),
                Event::new(
                    0,
                    Message::Aftertouch {
                        channel: 2,
                        note: 64,
                        pressure: 33,
                    },
                ),
                Event::new(
                    0,
                    Message::ChannelPressure {
                        channel: 2,
                        pressure: 44,
                    },
                ),
                Event::new(
                    0,
                    Message::PitchBend {
                        channel: 2,
                        value: 0x2F_3A,
                    },
                ),
                Event::new(
                    0,
                    Message::SysEx {
                        status: 0xF7,
                        data: vec![0x7E, 0x00],
                    },
                ),
                Event::new(
                    0,
                    Message::Meta {
                        kind: 0x7F,
                        data: vec![1, 2, 3],
                    },
                ),
                Event::new(0, Message::EndOfTrack),
            ])],
        };
        assert_eq!(decode(&encode(&file)).unwrap(), file);
    }

    #[test]
    fn a_missing_end_of_track_is_written_anyway() {
        let file = MidiFile {
            format: 0,
            division: 96,
            tracks: vec![Track::new(vec![Event::new(
                0,
                Message::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 64,
                },
            )])],
        };
        let decoded = decode(&encode(&file)).unwrap();
        assert_eq!(
            decoded.tracks[0].events.last().unwrap().message,
            Message::EndOfTrack
        );
        assert_eq!(decoded.tracks[0].name(), None);
    }

    #[test]
    fn damaged_files_are_reported_rather_than_guessed() {
        assert!(decode(b"").is_err());
        assert!(decode(b"RIFF\0\0\0\0").is_err());
        let mut short = b"MThd".to_vec();
        short.extend_from_slice(&2_u32.to_be_bytes());
        assert!(decode(&short).is_err());
        let mut truncated = b"MThd".to_vec();
        truncated.extend_from_slice(&6_u32.to_be_bytes());
        truncated.extend_from_slice(&[0, 1, 0, 1, 0, 96]);
        truncated.extend_from_slice(b"MTrk");
        truncated.extend_from_slice(&16_u32.to_be_bytes());
        assert!(decode(&truncated).is_err());
        assert!(read_track(&[0x00, 0x40, 0x40]).is_err());
        assert!(read_track(&[0x00, 0xF1]).is_err());
    }

    #[test]
    fn files_are_written_to_and_read_from_disk() {
        let directory = std::env::temp_dir().join(format!("midi-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("score.mid");
        let file = from_notes(&[note(0, 0, 4_410, 69, 90)], 44_100, "disk").unwrap();

        write_file(&path, &file).unwrap();
        assert_eq!(read_file(&path).unwrap(), file);
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"MThd");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
