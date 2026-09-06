---
bump: minor
---

### Added
- Tempo measurement from where the events actually fall, with a confidence the exporter uses to decide between the measured tempo and the one it was given.
- `audio::read_file` and `audio::write_file` choose the container from the bytes and from the extension, so WAV and AIFF are handled without the caller naming either.

### Fixed
- I/O failures name the file they happened to, instead of reporting "No such file or directory" with no hint which path was meant.
