---
bump: minor
---

### Added
- `cargo run --example public_domain_corpus` fetches real recordings from Wikimedia Commons, keeping only what Commons states is Public Domain or CC0, writes a `CREDITS.md` naming each performer and licence, and then decomposes and rebuilds each one to show the reconstruction is exact.
- An integration test that runs the whole round trip over that corpus when `AUDIO_DECOMPOSER_CORPUS` names it, and does nothing when it does not, so `cargo test` stays offline and hermetic by default.
