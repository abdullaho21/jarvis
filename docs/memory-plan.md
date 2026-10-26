# Memory plan — 2026-10-26

Goal: Jarvis should run comfortably on an ordinary 8 GB Windows 10/11 laptop,
not only on a 32 GB Arch desktop. Today the Linux voice worker alone ranges
from 160 MiB to 750+ MiB, and instant replay adds 270–560 MiB.

Two rules apply throughout:

- **Savings are hypotheses until measured.** Every figure below marked
  "expected" is an estimate. A step is done only when before/after numbers
  exist on the platform it claims.
- **Recognition quality is a hard constraint.** A smaller worker that misses
  commands, or acts on speech it should have ignored, is a regression.

## Where the memory goes today

Linux, Rust controller and worker, measured live and in the benchmarks under
[docs/benchmarks](benchmarks). RSS in MiB.

| Process | Idle | Peak | Notes |
| --- | ---: | ---: | --- |
| Voice worker, waiting for the wake phrase | 156–165 | — | Vosk small model and three Vosk decoders |
| Voice worker, Whisper loaded | 475–695 | 754–803 | Whisper small.en int8 adds ~315; loading adds scratch on top |
| Voice worker after 3 h, Whisper released | 366 | — | floor rose ~8 MiB per load on average; cause open, not load/release itself ([details](whisper-on-demand.md)) |
| Instant replay (gpu-screen-recorder) | 271–334 | 559 | ~110 for the process; the rest is the 120 s RAM buffer |
| Controller | 10 | — | |
| `pw-record` | 9 | — | |

Windows runs the Python service with Vosk and sherpa-onnx Whisper small.en,
and keeps up to 256 MiB of encoded replay in RAM. An earlier review of the
Python worker on Linux measured about 590 MiB PSS idle and 863–945 MiB peak
during commands ([performance](performance.md)). **Windows itself has not been
measured, and Linux numbers cannot establish Windows requirements.**

## What went wrong

1. **Two general speech recognizers for a closed set of commands.** Jarvis
   recognizes a known list of phrases, yet runs Vosk's general English model
   with grammars and then Whisper small (244 M parameters) as a second pass.
   Whisper is the single biggest cost. Vosk's free-form decoder is not pure
   duplication: `Matcher::choose` uses its text to reject grammar results
   whose intent disagrees ([matching.rs](../native/voice-worker/src/matching.rs)).
2. **Wake detection uses a full recognizer.** Listening for the wake name
   keeps the whole Vosk model and decoder resident (~150 MiB).
3. **Model weights are private heap copies.** tract parses, quantizes and
   plans the ONNX files in anonymous memory, which raises the loading peak and
   makes unloading depend on the allocator.
4. **Instant replay keeps 120 seconds of very-high-quality video in RAM**, on
   by default. During games that is ~350 MB of buffer.
5. **Windows still ships the Python service**, the heavier implementation, on
   the platform where memory matters most.
6. **A missed-wake bug, found while benchmarking.** After background speech,
   the next wake name can be missed entirely: only ~1.5 s of quiet reaches
   Vosk, which is not enough for its grammar decoder to end an utterance of
   unrecognizable speech, so the wake name never starts a new one.
7. **Smaller leftovers:** Vosk keeps feature history for up to 10 minutes of
   speech; partial hypotheses trigger Whisper loads (27 loads in 3 hours live).

## What others do

| Project | Approach | Lesson, with caveats |
| --- | --- | --- |
| [Home Assistant Speech-to-Phrase](https://www.home-assistant.io/blog/2025/02/13/voice-chapter-9-speech-to-phrase/) ([code](https://github.com/OHF-Voice/speech-to-phrase)) | Compiles known sentences into a Kaldi language model (FST, OpenGRM) with a fuzzy correction layer; under a second per command on a Raspberry Pi 4, versus ~8 s for Whisper there | One grammar-trained recognizer plus fuzzy matching can replace general ASR for fixed commands. Its free-form gaps (timers, shopping lists) mirror Jarvis's app names |
| [Picovoice Rhino](https://picovoice.ai/platform/rhino) and Porcupine | Domain "context" models for speech-to-intent; Porcupine uses about 1 MB of memory on a Pi 3 ([benchmark](https://picovoice.ai/docs/benchmark/wake-word/)) | Shows how small wake and intent can be; proprietary licensing rules it out for Jarvis |
| [openWakeWord](https://github.com/dscripka/openWakeWord) | Shared speech-embedding backbone plus a small classifier per wake word; one Pi 3 core runs 15–20 models | Cheap always-on listening; each wake word needs training |
| [sherpa-onnx keyword spotting](https://k2-fsa.github.io/sherpa/onnx/kws/index.html) | Streaming Zipformer KWS (~3.3 M parameters); keywords configured at run time without retraining | A candidate for configurable wake names. Thresholds trade detection against false alarms, and a ~4 MB model file does not establish the running footprint |
| [sherpa-onnx hotwords](https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html) | Biases a general recognizer toward listed phrases | Not equivalent to a constrained grammar; it can still output other text |
| [Moonshine](https://arxiv.org/pdf/2410.15608) | ASR for short commands: Tiny is 27 M parameters (~26 MB int8 ONNX, [source](https://huggingface.co/moonshine-ai/moonshine/discussions/6)), lower WER than Whisper tiny.en/base.en in the authors' tests | A candidate second pass. Needs an integration: the worker's recognizer is Whisper-specific |
| [Handy](https://handy.computer/docs/faq) | Configurable model unload timeout and a tray "Unload model" action | On-demand loading with user control is common |
| [Steam background recording](https://help.steampowered.com/faqs/view/23B7-49AD-4A28-9590), [NVIDIA instant replay](https://shacknews.com/article/142202/change-shadowplay-save-file-location-nvidia-app?amphtml=1) | Rolling buffer in temporary files on disk with a size limit | Disk buffers are normal for replay |
| [OBS replay buffer](https://obsproject.com/forum/threads/replay-buffer-maximum-memory-supported.167531/) | RAM buffer with a maximum-memory setting | A RAM mode should have an MB cap |

Whisper sizes for reference (whisper.cpp ggml files): small q5_1 181 MiB,
base q5_1 57 MiB, tiny q5_1 31 MiB
([model list](https://huggingface.co/ggerganov/whisper.cpp)).

## Budgets and acceptance criteria

Per-process targets stay, but the release gate is a **combined budget** for
everything Jarvis runs: voice worker, replay, controller, capture helper and
the settings window when open.

| Budget, 8 GB laptop | Target (to confirm in step 1) |
| --- | --- |
| All Jarvis processes, waiting, replay off | ≤ 150 MiB |
| All Jarvis processes, waiting, replay on | ≤ 300 MiB |
| Peak while recognizing a command | ≤ 400 MiB total |
| Growth over a week of normal use | none beyond the soak tolerance |
| Standby CPU | no worse than today (~0.1% of one core) |
| Command latency, median and p95 | no worse than today (~1.0 s / ~1.1 s after a pause) |
| Under memory pressure | keeps working or fails with a clear message; no crash loop |

Measure Windows with Private Working Set and commit charge, Linux with RSS
and PSS. Memory pressure: Linux under a cgroup memory limit; Windows on a
machine or VM with 8 GB and other applications open.

Recognition acceptance suite, required by every step that touches recognition:

- correctly spoken commands (fixtures plus a recorded set of real commands);
- similar commands that must map to the right one, not a neighbour;
- unrelated speech, music and game audio that must not trigger any action;
- repeated background speech → quiet → wake, **without** added noise;
- false actions counted and gated at zero for the fixture set.

Soak criteria (long-run memory):

- discard a warm-up period (first hour, or the first N load cycles);
- fit the RSS floor (value at each wake) over several windows; growth means
  a positive slope beyond a stated tolerance (for example 1 MiB/hour) in each
  of three repeated runs;
- a plateau after warm-up is bounded allocator caching and passes; a slope
  that persists across windows fails.

## Order of work

Steps depend on each other; the order matters.

### Step 1 — baselines and recognition tests

- Whole-application measurement on Linux and Windows, in the units above,
  stored as benchmark JSON. Report per-process memory in `jarvis status` and
  `doctor`, so users and tests can see it.
- Build the recognition acceptance suite above.
- Harden the scripts used as gates: `benchmark_load_cycles.py` now exits
  non-zero when any command differs from the fixture (done 2026-10-26). Add
  the soak criteria to a 24-hour variant.

### Step 2 — fix the missed wake

- Reproduce: `scripts/benchmark_load_cycles.py --no-room-noise` (currently
  fails after the first cycle) plus a dedicated test with repeated background
  speech → quiet → wake. The default benchmark adds room noise specifically to
  help Vosk end the utterance, which hides the bug; it is a memory benchmark,
  not evidence that wakes work.
- Candidate fixes to compare: feed Vosk enough quiet frames to end the
  utterance, or finalize/reset the wake decoder after a quiet gap.
- Done when the no-noise test passes repeatedly and the unrelated-speech
  tests show no new false wakes.

### Step 3 — replay on disk

Clearest expected saving (160–450 MiB while replay runs) with no recognition
risk. Requirements:

- bounded disk usage (time and size caps); cleanup of stale buffer files after
  a crash, at the next start; clear errors and a safe stop when the disk is
  full; tests that save during buffer rollover;
- the buffer directory must be real disk: Linux temporary directories can be
  RAM-backed [tmpfs](https://docs.kernel.org/filesystems/tmpfs.html), which
  would only move the memory (check the filesystem type, refuse tmpfs);
  private permissions;
- **privacy:** README, recording.md and windows.md promise that replay stays in
  RAM until saved. Disk buffering breaks that promise, so it must be an
  explicit, explained setting, and those documents updated. Keep a capped RAM
  mode;
- Linux: gpu-screen-recorder's `-replay-storage disk`. Windows: write the
  one-second segments to the buffer directory instead of RAM.
- SSD writes equal the bitrate (~1 GB/h desktop, ~10 GB/h games); state it.

### Step 4 — find the per-command growth, then decide allocator or process

- **Measured 2026-10-26: mimalloc not adopted.** In the load-cycle benchmark
  (results below) the floor grew ~3 MiB per cycle with glibc, with mimalloc
  for Rust allocations, and with Whisper kept loaded. The growth is not caused
  by Whisper load/release, so an allocator for Whisper does not address it.
  (That test was Linux GNU only and says nothing about Windows.)
- Next: run long enough to see whether the growth plateaus (a 32-cycle run
  was started), and attribute it. The leading candidate is Vosk's feature
  history: `Result()`/`Reset()` keep it until ~10 minutes of decoded speech,
  per decoder. Test by recreating or `FinalResult()`-finalizing the decoders
  between utterances, checking wake accuracy while doing so.
- Fallback if growth stays unexplained: a recognizer child process, a strong
  reclamation boundary for its own memory. It needs audio buffering while it
  starts, cancellation, and a measured startup latency. Keep it alive briefly
  after commands instead of assuming a restart on every wake is cheap.

### Step 5 — recognition fast path

- Skip the Whisper pass when Vosk's grammar result is exact and confident.
- Test it **separately** from removing the free-form decoder, which today
  rejects disagreeing grammar results.
- Done when the acceptance suite passes with zero false actions and memory,
  load count and latency improve measurably.

### Step 6 — smaller models and a wake detector, as experiments

- Second pass: compare Moonshine Tiny/Base and Whisper base.en/tiny.en with
  small.en on the acceptance suite (accuracy, false actions, latency, peak
  memory). Moonshine needs a recognizer integration first.
- Wake: trial sherpa-onnx keyword spotting for the configurable wake names.
  Measure its running footprint, tune thresholds against false alarms on the
  unrelated-speech set, and compare with the fixed Vosk wake path.
- Do not treat hotword biasing as a replacement for the command grammar.

### Then — choose the architecture

With steps 1–6 measured, decide among: Vosk grammar plus a small on-demand
second pass; a keyword spotter with an on-demand recognizer; a recognizer
child process; and when to move Windows to the Rust worker and controller.
These interact: a fast path makes loads rare (the allocator matters less), a
smaller model makes a child process cheaper to start, and a keyword spotter
changes where the missed-wake fix lives.

### Deferred

- Memory-mapped model weights: runtime engineering with an uncertain payoff.
  Revisit only if smaller models and fewer loads are not enough.

## Keep Whisper loaded or load on demand?

Keeping Whisper loaded does not stop the growth: in the benchmark below it
grew at the same rate as on-demand, from a floor 300 MiB higher. It saves the
reload CPU and avoids a possible wait on very short commands, but it is over
the budget above. On-demand stays the default; steps 4–6 decide the rest.

## Allocator and resident benchmark

15 live-like cycles per worker (wake plus command, 35 s of speech-like audio,
3 s of room noise), run one after another on Linux.
[Raw results](benchmarks/load-cycles-allocator-2026-10-26.json); method in
[whisper-on-demand](whisper-on-demand.md#load-cycles-allocator-and-resident-whisper).

| Worker | RSS at first → last wake, MiB | Growth per cycle | Peak MiB | Median latency | CPU |
| --- | --- | ---: | ---: | ---: | ---: |
| glibc, periodic trim, on demand (current) | 156.2 → 232.0 | 3.05 MiB | 597.8 | 908.5 ms | 59.6 s |
| mimalloc for Rust allocations, on demand | 158.5 → 231.6 | 2.71 MiB | 638.7 | 908.8 ms | 59.8 s |
| glibc, Whisper kept loaded | 459.8 → 518.1 | 2.74 MiB | 598.7 | 991.9 ms | 36.2 s |
