
# At a glance

```no_run
use audio_decomposer::audio::wav;
use audio_decomposer::decompose::{DecomposeOptions, Decomposer};
use audio_decomposer::recompose;

let original = wav::read_file("song.wav")?;
let decomposition = Decomposer::new(DecomposeOptions::default()).run(&original)?;
let rebuilt = recompose::reconstruct(&decomposition)?;

assert_eq!(original.channels(), rebuilt.channels());
# Ok::<(), audio_decomposer::Error>(())
```
