# Case Study: Issue #1 - Does Deduplication Actually Work on Real Music?

## Summary

Issue [#1](https://github.com/konard/audio-decomposer/issues/1) asks for a tool
that splits a recording into samples and deduplicates them "by subtracting and
different positions". The synthesised fixtures in the test suite deduplicate
well — a sixteen-second programmed loop collapses to 14 distinct waveforms
placed 64 times, a reuse factor of 3.7. Public-domain recordings of live
acoustic music do not: the pipeline reports a reuse factor of exactly 1.00 on
mono material and exactly 2.00 on stereo.

That is either a defect in the matcher or a property of the music. This case
study is the measurement that decided which, and it is written down because the
first measurement said the opposite of the truth.

## The recordings

Fetched by `cargo run --release --example public_domain_corpus`, which only
accepts files Wikimedia Commons states are Public Domain or CC0. Twelve seconds
of each, analysed with default options.

| Recording | Rate | Ch | Samples | Placements | Reuse | Exact |
|---|---|---|---|---|---|---|
| Missa Papae Marcelli — VI. Agnus Dei II | 44 100 | 1 | 141 | 141 | 1.00x | yes |
| Missa Papae Marcelli — V. Agnus Dei I | 44 100 | 1 | 147 | 147 | 1.00x | yes |
| Kol Nidre, cello and piano (1900s Zonophone) | 44 100 | 2 | 94 | 188 | 2.00x | yes |
| Bartók — Contrastes, Sebes | 48 000 | 1 | 148 | 152 | 1.01x | yes |
| When Johnny Comes Marching Home | 44 100 | 2 | 72 | 144 | 2.00x | yes |
| Ikke-identificeret violin-solo | 44 100 | 1 | 121 | 121 | 1.00x | yes |

Six of six reconstruct bit-exactly, so nothing here is a correctness problem.
The 2.00x on the stereo files is the two channels sharing one bank entry, which
is real deduplication but not the temporal kind the issue is about.

## First measurement, and why it was wrong

`experiments/match-quality` took every event of a recording, and for each one
asked the matcher for the smallest remainder achievable against any earlier
event, with the tolerance removed so the number reported is the remainder
itself rather than a pass or a fail. It said a third to two thirds of all
events had a near-perfect match:

```
56-2-ikke-identificeret-violin-solo.flac    remainder <= 0.05:  53 (26.9%)
bartok-contrastes-sebes-13-temps.flac       remainder <= 0.05: 171 (61.7%)
kol-nidre-cello-with-piano.flac             remainder <= 0.05: 192 (57.7%)
missa-papae-marcelli-v-agnus-dei-i.flac     remainder <= 0.05:  74 (30.0%)
missa-papae-marcelli-vi-agnus-dei-ii.flac   remainder <= 0.05:  79 (31.2%)
when-johnny-comes-marching-home.flac        remainder <= 0.05: 145 (56.0%)
```

If that were true the pipeline would be leaving a third of its reuse on the
floor, and something between the matcher and the pursuit — the candidate list,
the search radius, the stem split — would have to be discarding it.

It was not true. The probe handed the matcher the **original** signal and a
search radius of 50 ms. An event cut from that signal can therefore be slid
back onto the stretch it was cut from, where it matches itself exactly. Every
event whose predecessor begins within 50 ms scores a remainder of precisely
zero. Note the tell that was visible from the first run and went unread:
`best 0.000` in all six files, and a *bimodal* distribution — a cliff at 0.05
and then nothing at all until 0.6. Real similarity does not distribute like
that; a mixture of exact self-matches and genuine non-matches does.

The pipeline never has this problem because it subtracts each placement from
the working signal as it goes, so a waveform's own source is already silence by
the time anything is compared against it.

## Second measurement

The probe now reports both figures, refusing any pair whose source regions
could touch under the search radius:

| Recording | ≤ 0.05 | ≤ 0.12 | ≤ 0.25 | ≤ 0.40 | best | median |
|---|---|---|---|---|---|---|
| Ikke-identificeret violin-solo | 0.0% | 0.0% | 0.0% | 0.0% | 0.407 | 0.877 |
| Bartók — Contrastes | 0.0% | 0.0% | 3.6% | 19.7% | 0.198 | 0.549 |
| Kol Nidre | 0.0% | 0.0% | 0.6% | 23.0% | 0.185 | 0.532 |
| Missa Papae Marcelli — V | 0.0% | 0.0% | 0.0% | 3.3% | 0.332 | 0.674 |
| Missa Papae Marcelli — VI | 0.0% | 0.0% | 0.0% | 0.0% | 0.415 | 0.687 |
| When Johnny Comes Marching Home | 0.0% | 0.0% | 0.0% | 0.0% | 0.416 | 0.702 |

Zero events, in any of the six recordings, out of 1 554 compared, have a best
match under the 0.12 default tolerance. The single best pair in the whole
corpus still leaves 18.5% of its window behind.

## Third measurement: nothing in the options moves it

`experiments/reuse-sweep` runs the real pipeline over the same audio, changing
one thing at a time:

| Option change | Samples | Placements | Reuse |
|---|---|---|---|
| default | 72 | 144 | 2.00x |
| candidates 24 → 4096 | 72 | 144 | 2.00x |
| + search radius 10 ms → 50 ms | 72 | 144 | 2.00x |
| + tolerance 0.12 → 0.4 | 72 | 144 | 2.00x |
| + tolerance 0.4 → 0.8 | 62 | 144 | 2.25x |
| + no stem split | 62 | 144 | 2.25x |
| + unbounded gain | 62 | 144 | 2.25x |

(*When Johnny Comes Marching Home*, 12 s, stereo; every row reconstructs
exactly.) The other two files sweep the same way — the violin solo goes
1.00x → 1.00x → 1.00x → 1.00x → 1.04x, the Agnus Dei 1.00x → … → 1.35x, with
the first change of any kind arriving only at a tolerance of 0.8, and the stem
split and the gain bounds costing nothing anywhere.

Widening the candidate list by a factor of 170 changes nothing.
Tripling the tolerance changes nothing. Only accepting matches that leave 80%
of the window behind changes anything at all, and a "match" that explains a
fifth of what it is placed over is not a match — it moves work into the
residual instead of removing it.

## Conclusion

The matcher is doing what it should. A violin, a choir and a cello simply do
not play the same waveform twice: the same written note is a different physical
event each time, and gain and time shift are not enough to relate them. What
does repeat verbatim in these files is the relationship between the two
channels of a stereo recording, and the pipeline finds all of it.

Deduplication earns its keep where waveforms genuinely repeat — programmed
drums, looped bars, sampled instruments, anything sequenced rather than played
— which is also the material the issue names as the destination, since that is
what a DAW project is made of. On live acoustic recordings the tool still does
the rest of its job: stems, events, note recognition, projects, and an exact
reconstruction.

## Best Practices Identified

### 1. A probe that shares state with what it measures will measure itself

The matcher is a pure function of a signal, an anchor and a pattern. That made
it look safe to call directly on the original audio. It was not: the pattern
*came from* that audio, so the search space contained the answer. The pipeline
avoids this by construction rather than by care — it subtracts as it goes — and
the probe had to be told explicitly what the pipeline gets for free.

The general form: when an experiment reuses the production signal but not the
production sequencing, the parts of the sequencing that were load-bearing stop
being visible.

### 2. Read the shape of a distribution, not just its threshold counts

`26.9%` under 0.05 and `26.9%` under 0.40 is not a fact about music, it is a
fact about a spike at zero. The identical counts across four thresholds were on
screen in the first run and were read as "a third of events repeat" instead of
"a third of events are the same event". A distribution with a hard cliff and a
long empty gap is nearly always two populations, one of which is an artefact.

### 3. A sweep that moves nothing is a result worth keeping

`experiments/reuse-sweep` produced seven rows, six of them identical. It is
tempting to delete an experiment that found no effect. It is the evidence that
the defaults are not the problem, and it is what turned the search from "which
option is mistuned" to "the premise is wrong".

### 4. Test on material that can disagree with you

Every fixture in the suite is synthesised, which is the right default: it keeps
the repository free of copyrighted audio and the test run offline. But
synthesised fixtures are built out of repeated waveforms, so they can only ever
confirm that deduplication works. The public-domain corpus is the part of the
suite that is allowed to say no, and it is the only reason this question was
asked at all.

### 5. Report the honest number

`reuse 1.00x` is an unflattering thing for a deduplicating tool to print about
a piece of music. Printing it, rather than tuning a threshold until it read
better, is what made the measurement possible.

## Reproducing

```bash
cargo run --release --example public_domain_corpus            # fills ./corpus
AUDIO_DECOMPOSER_CORPUS=corpus cargo test --release --test integration public_domain

cd experiments/match-quality && cargo run --release -- ../../corpus/*.flac
cd experiments/reuse-sweep   && cargo run --release -- ../../corpus/*.flac
```

The corpus is fetched, never committed. `CREDITS.md` is written beside the
audio naming every recording, its performer, its licence and its source page.
