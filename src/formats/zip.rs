//! A store-only ZIP container.
//!
//! DAWproject files are ZIP archives, and so is anything else that needs to
//! carry several files as one. Everything this crate puts inside a container is
//! either XML or audio that has already been encoded exactly, and compressing
//! it would only add a dependency and a way to lose bytes, so entries are
//! stored uncompressed. Every ZIP reader accepts stored entries — that is the
//! method the format has had since the beginning.
//!
//! Reading is supported for the same subset, which is what makes the exporters
//! testable: a test writes a project and opens it again without shelling out to
//! `unzip`.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// One file inside the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Path inside the archive, with forward slashes.
    pub name: String,
    /// File contents.
    pub data: Vec<u8>,
}

impl Entry {
    /// An entry holding bytes.
    #[must_use]
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            data: data.into(),
        }
    }

    /// An entry holding text.
    #[must_use]
    pub fn text(name: impl Into<String>, text: impl AsRef<str>) -> Self {
        Self::new(name, text.as_ref().as_bytes().to_vec())
    }
}

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_DIRECTORY: u32 = 0x0605_4b50;
/// 1980-01-01 00:00, the earliest timestamp the format can express.
const DOS_TIME: u16 = 0;
/// See [`DOS_TIME`]; day 1, month 1, year 1980.
const DOS_DATE: u16 = 0x0021;

/// Builds the archive bytes.
///
/// The output is a pure function of the entries: no timestamps, no ordering
/// surprises, so two runs on the same input produce the same file.
#[must_use]
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for entry in entries {
        let offset = out.len() as u32;
        let checksum = crc32(&entry.data);
        let size = entry.data.len() as u32;
        push32(&mut out, LOCAL_HEADER);
        push16(&mut out, 20);
        push16(&mut out, 0);
        push16(&mut out, 0);
        push16(&mut out, DOS_TIME);
        push16(&mut out, DOS_DATE);
        push32(&mut out, checksum);
        push32(&mut out, size);
        push32(&mut out, size);
        push16(&mut out, entry.name.len() as u16);
        push16(&mut out, 0);
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&entry.data);

        push32(&mut directory, CENTRAL_HEADER);
        push16(&mut directory, 20);
        push16(&mut directory, 20);
        push16(&mut directory, 0);
        push16(&mut directory, 0);
        push16(&mut directory, DOS_TIME);
        push16(&mut directory, DOS_DATE);
        push32(&mut directory, checksum);
        push32(&mut directory, size);
        push32(&mut directory, size);
        push16(&mut directory, entry.name.len() as u16);
        push16(&mut directory, 0);
        push16(&mut directory, 0);
        push16(&mut directory, 0);
        push16(&mut directory, 0);
        push32(&mut directory, 0);
        push32(&mut directory, offset);
        directory.extend_from_slice(entry.name.as_bytes());
    }
    let start = out.len() as u32;
    let length = directory.len() as u32;
    out.extend_from_slice(&directory);
    push32(&mut out, END_OF_DIRECTORY);
    push16(&mut out, 0);
    push16(&mut out, 0);
    push16(&mut out, entries.len() as u16);
    push16(&mut out, entries.len() as u16);
    push32(&mut out, length);
    push32(&mut out, start);
    push16(&mut out, 0);
    out
}

/// Writes the archive to a file.
pub fn write_file(path: impl AsRef<Path>, entries: &[Entry]) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, write(entries))?;
    Ok(())
}

/// Reads every entry out of an archive.
pub fn read(bytes: &[u8]) -> Result<Vec<Entry>> {
    let end = end_of_directory(bytes)?;
    let count = read16(bytes, end + 10)? as usize;
    let mut offset = read32(bytes, end + 16)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if read32(bytes, offset)? != CENTRAL_HEADER {
            return Err(Error::Format(format!(
                "no central directory entry at byte {offset}"
            )));
        }
        let method = read16(bytes, offset + 10)?;
        let size = read32(bytes, offset + 24)? as usize;
        let name_length = read16(bytes, offset + 28)? as usize;
        let extra_length = read16(bytes, offset + 30)? as usize;
        let comment_length = read16(bytes, offset + 32)? as usize;
        let local = read32(bytes, offset + 42)? as usize;
        let name = text(bytes, offset + 46, name_length)?;
        if method != 0 {
            return Err(Error::Unsupported(format!(
                "`{name}` uses compression method {method}, only stored entries are supported"
            )));
        }
        entries.push(Entry {
            name,
            data: payload(bytes, local, size)?,
        });
        offset += 46 + name_length + extra_length + comment_length;
    }
    Ok(entries)
}

/// Reads every entry out of an archive file.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<Entry>> {
    read(&fs::read(path)?)
}

/// The bytes of the entry whose local header sits at an offset.
fn payload(bytes: &[u8], local: usize, size: usize) -> Result<Vec<u8>> {
    if read32(bytes, local)? != LOCAL_HEADER {
        return Err(Error::Format(format!(
            "no local file header at byte {local}"
        )));
    }
    let name_length = read16(bytes, local + 26)? as usize;
    let extra_length = read16(bytes, local + 28)? as usize;
    let start = local + 30 + name_length + extra_length;
    let data = bytes
        .get(start..start + size)
        .ok_or_else(|| Error::Format(format!("entry at byte {local} is cut short")))?
        .to_vec();
    let expected = read32(bytes, local + 14)?;
    let actual = crc32(&data);
    if expected != actual {
        return Err(Error::Format(format!(
            "checksum {actual:#010x} does not match the stored {expected:#010x}"
        )));
    }
    Ok(data)
}

/// Finds the end-of-central-directory record, which is at the end unless the
/// archive carries a comment.
fn end_of_directory(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < 22 {
        return Err(Error::Format("not a zip archive: too short".to_string()));
    }
    let limit = bytes.len().saturating_sub(22 + 0xffff);
    (limit..=bytes.len() - 22)
        .rev()
        .find(|&offset| read32(bytes, offset).is_ok_and(|word| word == END_OF_DIRECTORY))
        .ok_or_else(|| Error::Format("not a zip archive: no directory found".to_string()))
}

/// Reads a name or comment.
fn text(bytes: &[u8], offset: usize, length: usize) -> Result<String> {
    let raw = bytes
        .get(offset..offset + length)
        .ok_or_else(|| Error::Format(format!("a name at byte {offset} is cut short")))?;
    String::from_utf8(raw.to_vec()).map_err(|error| Error::Format(error.to_string()))
}

/// Reads a little-endian `u16`.
fn read16(bytes: &[u8], offset: usize) -> Result<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Format(format!("the archive ends before byte {offset}")))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

/// Reads a little-endian `u32`.
fn read32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Format(format!("the archive ends before byte {offset}")))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

/// Appends a little-endian `u16`.
fn push16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `u32`.
fn push32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// The CRC-32 both ZIP and gzip use.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut checksum = u32::MAX;
    for byte in bytes {
        checksum ^= u32::from(*byte);
        for _ in 0..8 {
            let carry = checksum & 1;
            checksum >>= 1;
            if carry != 0 {
                checksum ^= 0xedb8_8320;
            }
        }
    }
    !checksum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksum_matches_the_published_values() {
        // The two vectors every CRC-32 implementation is checked against.
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414f_a339
        );
    }

    #[test]
    fn an_archive_survives_the_round_trip() {
        let entries = vec![
            Entry::text("project.xml", "<Project/>"),
            Entry::text("metadata.xml", "<MetaData/>"),
            Entry::new("audio/kick.wav", vec![0u8, 1, 2, 3, 255]),
            Entry::new("empty.bin", Vec::new()),
        ];

        let bytes = write(&entries);
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(read(&bytes).unwrap(), entries);
    }

    #[test]
    fn the_same_input_always_produces_the_same_bytes() {
        let entries = vec![Entry::text("a.xml", "<a/>")];

        assert_eq!(write(&entries), write(&entries));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-zip-{}-{:?}/nested/archive.zip",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        let entries = vec![Entry::text("hello.txt", "hello")];
        write_file(&path, &entries).unwrap();

        assert_eq!(read_file(&path).unwrap(), entries);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn damage_is_reported_rather_than_ignored() {
        assert!(read(b"").unwrap_err().to_string().contains("too short"));
        assert!(read(&[0u8; 64])
            .unwrap_err()
            .to_string()
            .contains("no directory"));

        let mut bytes = write(&[Entry::text("a.txt", "hello")]);
        let flipped = bytes.iter().position(|byte| *byte == b'h').unwrap();
        bytes[flipped] = b'H';
        assert!(read(&bytes).unwrap_err().to_string().contains("checksum"));

        let mut bytes = write(&[Entry::text("a.txt", "hello")]);
        let signature = bytes.len() - 22 + 16;
        // Point the directory at the local header instead.
        bytes[signature] = 0;
        assert!(read(&bytes)
            .unwrap_err()
            .to_string()
            .contains("central directory entry"));
    }

    #[test]
    fn compressed_entries_are_refused_with_a_reason() {
        let mut bytes = write(&[Entry::text("a.txt", "hello")]);
        let directory = bytes.len() - 22 - (46 + 5);
        // Claim method 8 (deflate) in the central directory.
        bytes[directory + 10] = 8;

        let error = read(&bytes).unwrap_err().to_string();
        assert!(error.contains("compression method 8"), "{error}");
        assert!(error.contains("a.txt"), "{error}");
    }
}
