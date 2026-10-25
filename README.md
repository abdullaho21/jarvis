# Jarvis

Local voice control for Windows 10/11 x64 and Linux with Hyprland. Say **“Jarvis, open Firefox”**,
**“Jarvis, go to workspace three”**, or **“Jarvis, start recording”**.

Jarvis uses Vosk for wake detection and Whisper small.en for more accurate
commands. Recognition runs on your CPU without an API key, cloud service or GPU
inference. A separate native settings window (Windows or GTK 4) can close while voice control
continues in the background. Hands-free wake activation is the default after
microphone consent; listen-once activation is optional.

This is an early desktop application. Recognition depends on your voice,
microphone and room noise. Measurements and remaining checks are documented.

Windows 0.2.0 is a local release candidate: installer and portable ZIP, bundled
offline models and recording tools, with Discord moderation deferred. Read
[Windows setup and validation](docs/windows.md) before public release. Linux
installation instructions below remain available for Arch/CachyOS and Hyprland.

## Features

- Open or focus installed applications, with exact voice aliases.
- Switch workspaces, move a window, set fullscreen and adjust volume.
- Start and stop preconfigured screen recording, with automatic HDR detection.
- Optional instant replay: the last two minutes of screen, game sound and
  microphone stay in RAM; “clip that” saves them with a confirmation chirp
  (needs `gpu-screen-recorder`; idea from [Kairos](https://github.com/BasilForge/Kairos)).
- A Clips page that plays saved clips in the window (needs `gst-plugins-good`).
- Choose GPU AV1, HEVC or H.264, simple quality/bitrate budgets and an FPS slider with storage estimates.
- Preserve HDR in recordings and replays with 10-bit HEVC or AV1 and validated PQ/BT.2020 color metadata.
- Configure microphone, recognition, aliases, recording and startup in GTK.
- Mute immediately; the voice worker and microphone capture are released.
- Optional event-driven Waybar indicator and keyboard shortcuts.

Speech maps to allowlisted actions. It cannot run arbitrary shell commands.
Audio and transcripts are not saved by voice recognition. Screen recordings,
including optional microphone tracks, are separate deliberate user output.

## Install from this checkout

Python 3.11+, GTK 4/PyGObject, PipeWire/WirePlumber, Hyprland, GIO, `wpctl`,
`pw-record` and systemd user services are used on the tested setup. Screen
Recording needs FFmpeg / `ffprobe` and either `wf-recorder` for legacy SDR or `gpu-screen-recorder` for the new GPU formats, HDR and instant replay. GTK is supplied
by your distribution; do not build a second GTK with pip.

On Arch/CachyOS, the system dependencies used here are:

```sh
sudo pacman -S --needed python python-gobject gtk4 python-setuptools \
  pipewire pipewire-pulse wireplumber wf-recorder ffmpeg base-devel cmake ninja
```

From the checkout directory:

```sh
python3 -m venv --system-site-packages .venv
.venv/bin/python -m pip install -e '.[voice]'
./run-jarvis install-model
.venv/bin/python scripts/install_user.py --dry-run --service
.venv/bin/python scripts/install_user.py --service
./run-jarvis
```

The model download is about 680 MB and verified with pinned SHA-256 hashes.
Models are stored locally and are not included in the repository. The installer
creates a desktop launcher and an optional, initially disabled user service.
It does not edit Hyprland. Review an existing installation before using
`--replace`. Keep this checkout and venv at their installed paths.

In **Privacy & startup**, grant microphone consent, then unmute. The default
wake phrase is **“hey Jarvis”**; plain **“Jarvis”** also works. A command may
follow in the same breath or after a pause. Closing settings keeps the service
running. Use Mute to close voice input. In **Voice & microphone**, keep your
primary wake name and add other names separated by commas, such as **Alfred**
and **Jarvis**. Up to eight distinct names can be active together.

Without the downloaded models, the bundled PocketSphinx fallback has lower
recognition quality. After a system Python minor upgrade, rebuild the venv;
`MURMUR_CLI=python ./run-jarvis doctor` reports a Python/venv mismatch.

## Commands and app aliases

| Action | Examples |
| --- | --- |
| Applications | “open Firefox”, “launch Dolphin”, “switch to Steam” |
| Workspaces | “go to workspace three”, “move this window to workspace two” |
| Fullscreen | “make this window fullscreen”, “exit fullscreen” |
| Brightness | “set KTC brightness to sixty seven percent”, “dim Samsung”, “brighten main” |
| Volume | “turn the volume down”, “mute volume”, “turn the sound back on” |
| Recording | “start recording”, “stop recording”, “open the video recorder” |
| Instant replay | “clip that”, “save replay”, “start replay”, “stop replay” |
| Voice control | “cancel”, “stop listening” |

The **Commands** page contains the full reference. **Applications → Add voice
name** maps a phrase to an installed desktop entry. For example:

```sh
./run-jarvis alias streamio stremio
./run-jarvis alias
./run-jarvis alias streamio --remove
```

Each application voice name has an editable **Launch word**, initially `open`.
Change it to `watch` for “Alfred, watch video”, or clear it for “Jarvis, Firefox”.
Save the word beside an existing alias, or choose it when adding a voice name.
An optional `the` between the launch word and app name is ignored: “open the
video” resolves the same app as “open video”. Existing aliases without a saved
launch word retain the original open/launch/start verbs. Focus commands remain
available. A wake name (or an explicit Listen once session) is still required.

In **Displays**, refresh monitors, give each one a name and optional aliases
(for example, `KTC` and `k t c`), choose your main monitor, and save. Say “set KTC
brightness to 67 percent”, “dim Samsung to twenty percent”, or “brighten main”.
Dim/brighten without a percentage uses the saved step, initially 10 percentage
points. The Apply brightness button uses the same command dispatcher. Percentages
must be whole numbers from 0 to 100; monitor names and numbers are never guessed
by fuzzy matching.

Linux monitor control requires `ddcutil`, DDC/CI enabled in the monitor menu,
and user access to the monitor's `/dev/i2c-*` device. Discovery and naming do not
change brightness. Unsupported/disconnected displays show an error. Names use
the EDID manufacturer, model and serial so bus reordering preserves the target;
monitors without a unique serial are tied to their current connection, with a
notice in the app. Duplicate identities are refused. Up to eight monitor names
and aliases can be configured. Windows support is deferred.

An alias selects the application rather than adding an executable. Closest-match
recognition requires a clear lead over different commands. If both native and
Flatpak copies exist, Jarvis prefers the native application; use an alias to
select another copy.

## Background startup and recording

After installing the user unit, enable **Start at login** in Privacy & startup.
The graphical session must export its environment to the user service manager:

```sh
systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP XDG_RUNTIME_DIR
systemctl --user start murmur.service
```

The `murmur.service` identifier, `murmur` Python package, storage paths and old
launcher remain compatible with installations made under the earlier Murmur
name. The public app and preferred command are now Jarvis (`jarvis` or
`./run-jarvis`). Existing settings, models and recordings are not moved.

The unit follows `graphical-session.target`; verify your session manages it.
Manual foreground operation uses `./run-jarvis daemon`. Run one service at a time.
Optional [Waybar](examples/waybar.jsonc) and [Hyprland](examples/hyprland.conf)
examples require manual review and are never installed automatically.

Before screen recording, select the exact output, folder, encoder and optional
audio source in **Recorder**, verify a short clip and save the reviewed setup.
CPU video encoding can cost far more than voice standby; review the hardware
encoder and permission limits in [recording setup](docs/recording.md).
Muting voice control does not mute a microphone track in an existing recording.

## Performance and review

The completed review reports lower command memory peaks (**890–945 → 863–879
MiB**) and faster command matching on the tested Ryzen 7 7800X3D. Stronger
validation of commands spoken in one breath adds about **245 ms** of latency
to avoid executing the wrong application. These are fixture measurements;
see [performance](docs/performance.md) for the comparison and its limits.

Earlier live background samples used **0.2–0.6%
of one logical CPU**, about **0.01–0.04% of the 16-thread CPU's capacity**. After live
commands, the background process tree used about **657 MiB RSS**, down from
about **1,090 MiB** before allocator changes. Inference has a brief CPU burst;
standby numbers are not a gaming FPS measurement.

See [current performance and reproduction](docs/performance.md),
[memory/background evidence](docs/memory-background.md) and the
[review handoff](docs/review-ready.md) for workload scope, latency tradeoffs and
remaining acceptance checks. Older PocketSphinx measurements are archived and
do not describe the current recognizer.

The [Rust voice worker](docs/rust-voice.md), preferred on Linux when available, performs both
[Whisper recognition](docs/rust-whisper.md) and [GTCRN denoising](docs/rust-denoiser.md)
with Rust audio processing and neural kernels, using the existing model weights.
Vectorized kernels cut the first Rust version’s median decode time by about
57%. On the current public fixtures it roughly matches native transcription
latency while retaining about 57 MiB less memory in the isolated transcriber. Vosk wake detection still uses its native library.
Python remains available for Windows, PocketSphinx and installations without the native worker. Set `MURMUR_VOICE_WORKER=python` to select it explicitly; the guides explain build, selection and verification.

An optional [Rust background controller](docs/rust-controller.md) also implements
the local service API, configuration, command dispatch and recorder/replay
supervision. The current GTK settings window and CLI remain compatible. Build
it with `scripts/build_controller.py`; select it with
`MURMUR_CONTROLLER=rust ./run-jarvis daemon` or launch its executable directly.

A [memory audit](docs/rust-worker-memory.md) identified the voice worker as
Jarvis's largest RAM user. Lossless embedding storage reduced its controlled
retained RSS from **558.9 to 483.0 MiB**, with identical fixture outputs.

The [Rust CLI](docs/rust-cli.md) adds native controls, aliases, Waybar status,
diagnostics and verified model/user installation. Build it with
`scripts/build_cli.py --test`; `./run-jarvis` then prefers the native executable.
`MURMUR_CLI=python` selects the compatibility client. The GTK settings window
still uses Python. Building the client does not restart the service.

```sh
.venv/bin/python -m unittest discover -s tests -v
./run-jarvis doctor
.venv/bin/python scripts/install_user.py --dry-run --service
```

The October 24 refactor passed **261 Python tests with no skips** using local
voice models and the public speech fixture, plus **31 Rust unit tests**.
See the [local refactor review](docs/refactor-review.md) for measured parsing,
CPU and memory changes and the limits of local verification. The
earlier review passed 150 tests and CodeRabbit raised 0 issues across that
review's changes. Regular
tests do not open the microphone or capture the screen. Real-model tests require
explicitly supplied local model and fixture paths. The [GitHub Actions
workflow](https://github.com/abdullaho21/jarvis/actions) checks core tests,
optional voice dependencies and package construction.

## Project layout

```text
src/murmur/       Voice engine, service, desktop adapters and GTK UI
tests/            Policy, lifecycle, model and regression tests
scripts/          Installer, explicit integration checks and benchmarks
examples/         Portable configuration, Hyprland and Waybar examples
docs/             Current architecture, setup, verification and review notes
docs/benchmarks/  Raw measurements, with workload boundaries
docs/history/     Earlier decisions and development notes
.github/          CI and review template
```

[Documentation index](docs/README.md) · [Contributing](CONTRIBUTING.md) ·
[Security boundaries](SECURITY.md)

## License

Jarvis source is licensed under [MIT](LICENSE). Models and dependencies retain
their own licenses; see [dependency and model notices](docs/dependencies.md).
