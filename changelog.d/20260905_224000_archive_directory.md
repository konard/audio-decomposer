---
bump: minor
---

### Added
- A decomposition archive: `archive::write_dir` lays a decomposition out as a directory holding the manifest in Links Notation, one file per stem, one file per bank sample and the residual, and `archive::read_dir` returns the decomposition it was written from.
- Name collisions between stems and between samples are resolved when the archive is written, so two samples whose names differ only in case never share a file.
