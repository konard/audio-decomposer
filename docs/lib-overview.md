# At a glance

Take a recording apart and put it back together:

```no_run
use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::{audio, recompose};

let original = audio::read_file("song.wav")?;
let decomposition = decompose(&original, &DecomposeOptions::default())?;
let rebuilt = recompose::reconstruct(&decomposition);

// For integer PCM input the reconstruction is exact, not approximate.
assert_eq!(original.channels(), rebuilt.channels());
# Ok::<(), audio_decomposer::Error>(())
```

Keep it on disk and read it back unchanged:

```no_run
use audio_decomposer::archive;
# use audio_decomposer::decompose::{decompose, DecomposeOptions};
# use audio_decomposer::audio;
# let original = audio::read_file("song.wav")?;
# let decomposition = decompose(&original, &DecomposeOptions::default())?;
archive::write_dir("song-decomposition", &decomposition)?;
let same = archive::read_dir("song-decomposition")?;
assert_eq!(same.samples.len(), decomposition.samples.len());
# Ok::<(), audio_decomposer::Error>(())
```

Write it out as the projects music software already reads:

```no_run
use audio_decomposer::formats::project;
use audio_decomposer::formats::session::SessionOptions;

let written = project::export("song-decomposition", &SessionOptions::default())?;
for path in &written {
    println!("{}", path.display());
}
# Ok::<(), audio_decomposer::Error>(())
```

## The pipeline

| Module        | Responsibility                                                     |
| ------------- | ------------------------------------------------------------------ |
| `audio`       | WAV and AIFF codecs, planar buffers on an exact integer grid        |
| `dsp`         | FFT, STFT, windows, onsets, pitch detection, stem separation        |
| `decompose`   | Stems, events, the sample bank, matching pursuit, note recognition  |
| `associative` | The doublet link network and its Links Notation projection          |
| `archive`     | Writing a decomposition to a directory and reading it back          |
| `formats`     | MIDI, MusicXML, DAWproject and the DAW project writers              |
| `recompose`   | Rebuilding the audio, sample-exactly for integer PCM                |
