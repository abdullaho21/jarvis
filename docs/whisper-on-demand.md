# On-demand Whisper — 2026-10-25

The Rust voice worker no longer holds Whisper while it waits for the wake
phrase. With GTCRN noise suppression on, the complete worker's RSS while
waiting fell from **468.9 to 156.4 MiB (−312.6 MiB, 66.7%)**. Peak RSS and
command latency on the public one-breath fixture were unchanged. Without
GTCRN, waiting RSS fell from 465.2 to 156.2 MiB.

## Behavior

- Startup loads Vosk only, so the worker reports ready in about 0.2 seconds
  instead of 1.8–1.9 seconds.
- Whisper (and GTCRN, when enabled) starts loading on a background
  `whisper-load` thread when any of these happens:
  - every fourth speech frame, Vosk's in-progress hypothesis has a wake name
    in its first three words (the same rule as the final wake check, without
    its confidence threshold);
  - a wake result is accepted;
  - a command session begins (after a separate wake, Listen once, or `once`
    mode).

  The capture loop keeps feeding Vosk during the load. When a command needs
  Whisper, the worker waits for the load. If no thread could be started, it
  loads Whisper on the spot instead.
- Whisper is released after `whisper_idle_seconds` without a transcription or
  wake. Both the Python and Rust controllers send 60 seconds. Release happens
  only while waiting for the wake phrase, never during a command session, and
  is followed by `malloc_trim`.
- A Whisper load error still stops the worker with an error event. It is
  reported at the first wake instead of at startup.
- A payload without `whisper_idle_seconds` (from an older controller) loads
  Whisper at startup, before Vosk, and keeps it, exactly as before. Valid
  values are 1–86,400 seconds. The Python worker is unchanged.

### One malloc arena

The worker now calls `mallopt(M_ARENA_MAX, 1)` before any thread starts. With
glibc's per-thread arenas, the loader and inference threads kept pages after
each release. RSS when each command started grew from 155.7 MiB through 194.0,
210.9, 214.0, 225.7 to 231.8 MiB over six release cycles. One arena stopped that
growth without a latency cost. A fixed 128 KiB mmap threshold also stopped it,
but made every command about 440 ms slower. Disabling transparent huge pages
did not. [Allocator comparison](benchmarks/whisper-on-demand-allocator-2026-10-25.json):

| Six on-demand cycles, GTCRN on | RSS at each command start, MiB | Peak MiB | Median latency |
| --- | --- | ---: | ---: |
| glibc defaults | 155.7 → 231.8 | 618.1 | 992.8 ms |
| One arena (adopted) | 156.3 → 194.1 | 570.8 | 996.0 ms |
| 128 KiB mmap threshold | 155.7 → 188.4 | 535.6 | 1,433.2 ms |
| THP disabled | 142.1 → 212.1 | 582.0 | 1,008.4 ms |

## Measurements

Each run used a fresh, sequential worker process with real Vosk and Whisper
small.en, GTCRN where noted, and eight inference threads. A paced fake
`pw-record` played the public `goforward.raw` fixture six times as a one-breath
"go forward ten meters", using a surrogate wake name and command. Each
on-demand command started with Whisper released: the idle limit was 3 seconds
and the gaps were 6 seconds. The "kept loaded" column is the saved previous
worker.

Latency runs from the fixture's last voiced 30 ms frame (0.416 s before its end)
to receipt of the command event. It therefore includes Vosk's endpoint wait,
GTCRN and Whisper decoding. The first command of every run was slower in both
versions, so it is reported separately.

| Complete worker, GTCRN on | Whisper kept loaded | On demand |
| --- | ---: | ---: |
| Ready after start | 1.91 s | 0.21 s |
| RSS waiting for the wake phrase | 468.9 MiB | 156.4 MiB |
| RSS after release, median | — | 193.6 MiB |
| Peak RSS | 569.8 MiB | 552.5 MiB |
| First command | 1,208.5 ms | 1,186.3 ms |
| Median of commands 2–6 | 995.4 ms | 979.4 ms |
| Worker CPU for six commands | 10.31 s | 18.59 s |

| Complete worker, GTCRN off | Whisper kept loaded | On demand |
| --- | ---: | ---: |
| RSS waiting for the wake phrase | 465.2 MiB | 156.2 MiB |
| RSS after release, median | — | 191.2 MiB |
| Peak RSS | 566.1 MiB | 548.7 MiB |
| First command | 1,090.3 ms | 1,069.2 ms |
| Median of commands 2–6 | 916.0 ms | 911.0 ms |

A third run per setting used a 20-second idle limit and 3.5-second gaps. Only
its first command loaded Whisper. Peak RSS was 567.9 MiB in both settings. The
reused commands' medians were 1,004.5 and 938.0 ms. All 38 commands across the
six runs were the expected `ten meters`. Raw results:
[GTCRN on](benchmarks/whisper-on-demand-denoise-2026-10-25.json) and
[GTCRN off](benchmarks/whisper-on-demand-2026-10-25.json).

### What these results do not show

- On this fixture, about 1.2 seconds of command speech plus Vosk's endpoint
  wait follow the wake name, which hides the ~1.6-second load. A very short
  command right after the name, such as "Jarvis, mute", leaves less time. It
  can therefore add up to the remaining load time to the first command after a
  minute of silence. That case was not measured, and neither was a partial
  hypothesis that misses the wake name.
- Each reload costs about 1.4 CPU-seconds ((18.59 − 10.31) / 6). It also
  re-reads the 374 MB model files, normally from the page cache.
- RSS after release still rose about 1 MiB per cycle in the six-cycle runs.
  Longer sessions were not measured.
- These runs used one speaker, one fixture and one machine. There was no live
  microphone, service restart, desktop action, game or room noise.

## Verification

- Rust: five new tests cover background loading, idle release and reload,
  busy sessions, idle-clock refresh and load errors. All 10 voice-worker tests
  pass, the controller has 4 tests and the CLI has 9. Clippy (with warnings
  denied) and rustfmt pass for each crate.
- Python: two new real-model worker tests count the `whisper-*` threads. With
  a 1-second limit there are none before a wake, some after the command, and
  none again after release. Without the field, Whisper is resident at ready.
  Other new checks confirm that invalid idle values are rejected and that the
  payload carries 60 seconds.
- The full suite with the local Vosk, Whisper and GTCRN models and the public
  fixture ran 303 tests: 299 passed and four Windows-only audio cases were
  skipped.

### Live service check

After `murmur.service` was restarted on the staged binaries, a sampler read the
live voice worker four times a second. The noise-suppression setting was on.
The wake phrase was followed by speech that was not a known command, and
Whisper transcribed it. Whisper was released 71 seconds after loading: 60
seconds of idle time after the 8-second listening session timed out.

| Live voice worker | RSS MiB | PSS MiB |
| --- | ---: | ---: |
| Previous worker, Whisper kept loaded, 30 min after start | 552.3 | 547.9 |
| On demand, waiting for the wake phrase (after one earlier load) | 182.1 | 177.7 |
| Whisper loaded, after the wake and transcription (median) | 492.1 | 487.7 |
| After a minute without a command (released) | 202.1 | 197.7 |

The process peak (VmHWM) was 556.5 MiB. RSS after release was 20 MiB above the
previous waiting value after this second live cycle. Longer live use has not
been measured.

## Reproduce

Save the existing worker before building, then run the sequential comparison:

```sh
cp src/murmur/bin/jarvis-voice /tmp/jarvis-voice-before
.venv/bin/python scripts/build_voice_worker.py --test --offline
.venv/bin/python scripts/benchmark_whisper_on_demand.py \
  --fixture build/refactor-review/goforward.raw \
  --before /tmp/jarvis-voice-before --repeats 6 --denoise
```

Omit `--denoise` for the run without GTCRN. The script uses the installed models
and the deliberately supplied fixture. It never opens a microphone or
dispatches an action.
