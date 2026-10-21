# Measured performance

Measured 20 October 2026 on CachyOS, Linux 7.2.10-1-cachyos, AMD Ryzen 7 7800X3D
(16 logical CPUs), approximately 32 GB RAM, Radeon RX9070 plus Raphael iGPU,
Python 3.14.8, GTK 4.24.1, Hyprland 0.56.2 and PipeWire 1.6.9. Both displays are
2560×1440; capture tests selected the SDR DP-5 output at 240 Hz and encoded 30 fps.
Samples are short engineering checks, not production service-level guarantees.

CPU is `(process CPU seconds / wall seconds) ×100`, so 100% is **one logical
CPU**, not the whole 16-thread machine. Service samples sum RSS across the
service, voice and pw-record tree; shared pages are double-counted. The external
meter samples every 250 ms and is excluded from service usage. Warm-up is excluded
from standby/UI samples. CPU accounting has scheduler-tick resolution: zero
recorded ticks over 15 seconds does not mean mathematically zero resource use.

## Costs kept separate

| Workload | Interval | CPU, one logical CPU | Observed memory | Raw result |
| --- | --- | --- | --- | --- |
| Muted service, microphone/model absent | 15.19 s | 0 recorded ticks | 26.95 MiB tree peak RSS | [JSON](../benchmarks/service-paused-benchmark.json) |
| Optional PTT standby, microphone/model absent | 15.17 s | 0 recorded ticks | 27.10 MiB tree peak RSS | [JSON](../benchmarks/service-ptt-benchmark.json) |
| Default hands-free waiting, **live microphone**, no commands/actions | 30.09 s | 0.166% | 83.80 MiB tree peak RSS | [JSON](../benchmarks/service-wake-benchmark.json) |
| Native Wayland GTK settings + PTT service, microphone closed | 15.26 s | 0.131% | 164.04 MiB combined tree peak RSS | [JSON](../benchmarks/service-ui-benchmark.json) |
| PocketSphinx active, one clean 2.786 s fixture repeated 20 times | offline | 16.59 ms median decode;17.05 ms p95 | 49.65 MiB worker peak RSS | [JSON](../benchmarks/speech-benchmark.json) |
| Full paced PTT replay → dry-run action | one fixture | ~0.36 s after energy endpoint proxy | not sampled | [JSON](../benchmarks/replay-integration.json) |
| Full paced wake → command → dry-run action | two fixture playbacks | ~0.40 s after energy endpoint proxy | not sampled | [JSON](../benchmarks/replay-wake-integration.json) |
| Native 1440p30 CPU screen recording lifecycle, no audio | 3.06 s | 196.21% | 211.83 MiB peak child RSS | [JSON](../benchmarks/desktop-integration-repeat.json) |
| Native 1440p30 VA-API screen recording lifecycle, no audio | 3.07 s | 6.47% | 105.18 MiB peak child RSS | [JSON](../benchmarks/desktop-vaapi-integration.json) |
| Synthetic 720p30 libx264 source→encoder→null | 9.57 s | 8.68% | 80.95 MiB peak FFmpeg RSS | [JSON](../benchmarks/encoder-benchmark.json) |

An [earlier 30.14 second sample](../benchmarks/service-wake-first-benchmark.json) measured
0.564% CPU and 85.74 MiB peak RSS. The final sample above measured 0.166%
and 83.80 MiB. Room noise was not recorded, so their difference is not attributed
to a code optimization or a specific acoustic condition.

The default standby path met the initial **<1% / <150 MB** targets in this sample.
It uses CPU-only PocketSphinx; there is no CUDA/ROCm/OpenCL/inference runtime or
voice GPU model allocation. This is an implementation property of the voice
backend, not a claim that the whole graphical desktop uses no GPU resources.
GTK may use the system graphics renderer while open. Video capture/VA-API uses
GPU copy/conversion/media-engine resources independently of voice.

The speech fixture is the upstream `goforward.raw`; nothing was recorded from a
person for replay. Hands-free replay uses **“go forward”** as a surrogate wake
phrase and a temporary command alias, not the default wake phrase. An explicitly
preselected final waveform frame above RMS 0.01 is the endpoint proxy. It is not
human-annotated speech end, and feedback is observed with 50 ms granularity. These
results do **not** establish live end-of-speech dispatch under one second.

The recording measurements include wf-recorder, Hyprland queries and ffprobe
startup/finalization; they exclude compositor, game, PipeWire and disk-cache
costs. Peak child RSS is the largest child, not the summed service tree. Desktop
contents differed between short samples, so the CPU/VA-API contrast is a useful
setup observation, not a controlled equal-workload speedup. The synthetic 720p
case excludes screencopy entirely; it must not stand in for actual 1440p capture.
Real audio/video test clips were deleted after verification.

## Stack decision

The measured Python service baseline is about 27 MiB and the full small-model
hands-free tree about 84–86 MiB. This meets the initial memory target without a
browser runtime, Rust rewrite or complicated native UI binding layer. GTK uses
the already-installed native toolkit and remains in its own process. Closing
the window releases its memory. Rust/C could reduce interpreter overhead, but
no claim of a measured advantage is made here; CPU decoding remains native in
the current implementation.

PocketSphinx's isolated silent standby measured 0.051% CPU; the whole capture
pipeline measured 0.166–0.564% across two short samples. Vosk's small English model comparison peaked at 205 MiB,
which misses the resident voice budget. A lazy Vosk subprocess would improve
active recognition flexibility at a load/active-memory cost and would still
need a separate wake detector. PocketSphinx is older and less accurate; many
app names require aliases. See [speech research](../speech-research.md) for exact
model sizes, maintenance limitations and primary sources. No broader recognition
quality conclusion can be drawn from one clean fixture.

The portable CPU video preset costs enough to affect a game at native 1440p.
For this tested AMD setup, **VA-API 30 fps, QP 23** is the practical low-CPU option
after capture/content/audio setup verification. It uses the GPU media engine,
is not a promise of zero graphics contention, and is not automatically selected
on other hardware. Hardware source/driver/format validation is mandatory.

## Reproduce

Install the benchmark-only dependency into the local venv:

```sh
.venv/bin/python -m pip install '.[benchmark]'
.venv/bin/python scripts/benchmark_service.py --mode paused --seconds 30
.venv/bin/python scripts/benchmark_service.py --mode ptt --seconds 30
.venv/bin/python scripts/benchmark_service.py --mode wake --seconds 30 --live-microphone
.venv/bin/python scripts/benchmark_service.py --mode ui --seconds 30
```

The live-input flag is an explicit opt-in. These scripts use temporary config/socket paths and
disable action dispatch. The UI test opens a native window, then closes it;
run in a real Wayland session. A GTK Broadway renderer assertion occurred with
a connected test browser and was rejected by the benchmark's liveness check.
No successful Broadway result is being presented as native Wayland rendering.

Speech fixtures, checksums and isolated decoder commands are in
[speech research](../speech-research.md). A supplied fixture can exercise the real
pipeline safely:

```sh
.venv/bin/python scripts/check_replay.py /tmp/goforward.raw
.venv/bin/python scripts/check_replay.py /tmp/goforward.raw --wake
```

Actual recording tests deliberately access the specified screen, without audio,
then validate and delete their videos. Run outside a sensitive/game session:

```sh
.venv/bin/python scripts/check_desktop.py --record --monitor DP-5 --seconds 3
.venv/bin/python scripts/check_desktop.py --record --monitor DP-5 --seconds 3 \
  --codec h264_vaapi --vaapi-device /dev/dri/renderD128
.venv/bin/python scripts/bench_recording.py --seconds 10 --size 1280x720 --fps 30
```

## Still to measure

- At least 30 minutes idle and representative speaker/game audio, reporting false
  wakes/hour, false commands/hour, command success by phrase and rejection rate.
- Human voices/accents and default wake phrase on the chosen microphone; annotated
  end-of-speech→handler latency with median/p95 and model-cold/model-warm states.
- A reproducible game scene with no recording, CPU recording and VA-API recording:
  record average FPS, 1% lows, frame-time p95/p99, compositor CPU, recording CPU,
  disk throughput and media-engine/graphics utilization. Keep resolution, game
  settings, scene duration and power profile identical; repeat each condition.
- Long recordings, disk-full/error behavior, suspend/resume, audio mix sync,
  monitor hotplug and permission persistence across logout/reboot.

No gaming FPS or GPU/media-engine utilization numbers have been invented. The
current machine was not put into a controlled game benchmark by this task.
