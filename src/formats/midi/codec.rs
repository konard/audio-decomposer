//! The bytes of a standard MIDI file, written and read back.
//!
//! The encoder and the decoder live together because they are inverses: a file
//! read from disk and written back produces the same bytes, which is the
//! property the tests at the bottom of this file check. That includes running
//! status, the abbreviation a file uses when a channel message repeats the
//! previous status byte.

use crate::error::{Error, Result};

use super::{Event, Message, MidiFile, Track};

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
}
