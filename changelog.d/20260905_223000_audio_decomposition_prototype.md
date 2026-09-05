---
bump: minor
---

### Added
- Exact WAV and AIFF codecs with a planar audio buffer that keeps integer PCM on its own grid, so a decoded and re-encoded file is byte-identical.
- A DSP layer: FFT, STFT with exact overlap-add reconstruction, window functions, envelope followers, onset detection with attack snapping, monophonic pitch detection and polyphonic partial tracking, and harmonic/percussive source separation.
- A decomposer that splits a recording into stems, cuts them into events, deduplicates the events into a sample bank by matching pursuit, recognises the note of every placement, and keeps a residual that makes the reconstruction exact.
- An associative link store that deduplicates every value as a doublet, projects the whole decomposition to Links Notation, and can be mirrored into the upstream `doublets` storage behind the `doublets-native` feature.
- Standard MIDI file reading and writing, with a tempo and division chosen so one tick is exactly one audio frame.
