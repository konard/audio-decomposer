---
bump: minor
---

### Added
- Exact reconstruction: `recompose::reconstruct` sums the stems, the placements and the residual back into the recording, sample for sample for integer PCM input.
- `recompose::verify` measures a decomposition against the recording it came from and reports the deviation, the share of energy left in the residual, and how many frames of timeline each stored frame accounts for.
- `recompose::arrange` plays notes through the sample bank, so a decomposition can voice a score it was never made from. Notes the bank cannot voice are either left silent or synthesised, velocity can scale the sample, and samples can be cut at the length of the note with a fade that stops the click.
