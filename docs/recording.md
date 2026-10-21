# Recording without leaving a game

Jarvis uses the installed `wf-recorder` executable. Opening recording settings and starting capture are separate actions. The start action launches no window, portal, region selector, or terminal; it names the configured monitor explicitly and disconnects standard input. A compositor permission prompt is still possible if permission has not been established. Complete and test setup outside a game first.

The desktop controls are the normal route. For integration checks with the daemon already running, use `murmur ui --page recording` to open setup, `murmur record start` to capture, and `murmur record stop` to save. `murmur config` shows the current settings. In a source checkout, prefix the equivalent invocations with `PYTHONPATH=src .venv/bin/python -m murmur`.

## Backend choice and limits

| Approach | License and maintenance evidence checked 2026-10-20 | Resource and permission tradeoff | Decision |
| --- | --- | --- | --- |
| [wf-recorder](https://github.com/ammen99/wf-recorder) | MIT; upstream commits visible in 2026; installed version 0.6.0 | Direct Wayland screencopy; CPU encoding or optional VA-API. No portal source picker when an output is named, but compositor permission rules apply. | Implemented: already installed, small subprocess interface, no new privileged helper. |
| [GPU Screen Recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) | GPL-3.0-only; maintained upstream with current CLI, signals and Unix-socket control | Hardware capture/encoding suits gaming, but monitor capture uses a privileged KMS helper. Portal capture avoids that helper and may require source selection. Its low-overhead claims are upstream claims, not Jarvis measurements. | Evaluated, not implemented or installed. |
| [ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html) | Interface provided by the desktop's installed portal implementation | Normally presents source/permission UI. Persistence depends on the backend and permission grant; restore tokens rotate after each successful session. Missing sources or revoked permission can cause another prompt. | No generic promise of unattended capture; no portal backend in this release. |

The capture backend performs video encoding; it is independent of speech recognition. Choosing VA-API deliberately uses the GPU's media engine and may also use graphics copies/conversion. The default voice path still performs no GPU inference.

The installed wf-recorder help/man page and [upstream usage](https://github.com/ammen99/wf-recorder#usage) establish the fixed-output, codec-parameter, explicit-audio and Ctrl+C stop interfaces used by the adapter. It accepts one audio capture source. Jarvis does not create hidden audio routing or pretend that checking two boxes mixes microphone and desktop audio.

## First-run setup

1. Open Recording settings and select a connected monitor. The source list comes from `hyprctl -j monitors`. A disconnected source causes a clear error; Jarvis does not silently record a different screen.
2. Choose an output folder with sufficient space. Jarvis creates it when capture starts, reserves each new file with mode `0600`, and does not overwrite recordings.
3. Select no audio, microphone, or system audio. Audio is off by default. For microphone, select the Pulse source name of the input. For system audio, choose the output sink's `.monitor` source. PipeWire systems normally provide these through `pipewire-pulse` and `pactl`.
4. To record both, create/review your own combined audio source before gaming, select it explicitly, and set `combined_audio_source: true`. That routing is outside this initial release. Selecting both without this acknowledgment fails safely; it does not silently omit either input.
5. Choose the encoder and quality. Begin with the 30 fps CPU preset below; compare with VA-API on the actual game and GPU before choosing.
6. Check compositor capture permission, make a short test clip, and play it back. Confirm that it shows the intended screen and audio, produces no permission prompt on a second capture, and leaves focus on the game. Then mark recording setup complete (`configured: true`). This flag records your setup acknowledgment; it does not grant permissions or prove future sessions will remain permitted.

[Hyprland's permission documentation](https://wiki.hypr.land/configuring/core/advanced-configuration/permissions/) describes screencopy allow/ask/deny rules. In current Lua configuration, an explicit grant looks like:

```lua
hl.permission({ binary = "/usr/bin/wf-recorder", type = "screencopy", mode = "allow" })
```

Review that grant before adding it: it allows the named recorder to capture without asking. Use the actual installed executable path. Permission rule changes require a Hyprland restart; a normal config reload is insufficient. Older releases use different syntax; consult that installed version's wiki. Jarvis does not edit your Hyprland configuration or change enforcement. An enforced denial can yield a valid black video containing a permission message, which structural file validation cannot distinguish from ordinary content. Visual first-run verification is necessary.

## Practical presets

```json
{
  "recording": {
    "configured": false,
    "monitor": "DP-5",
    "output_dir": "~/Videos/Murmur",
    "system_audio": false,
    "microphone": false,
    "audio_device": "",
    "fps": 30,
    "codec": "libx264",
    "quality": 23
  }
}
```

The CPU preset uses H.264, `preset=ultrafast`, two encoder threads, 30 fps, CRF 23 and Matroska. These are engineering choices to limit encoding work, not measured promises of gaming impact. It records the selected monitor at its native dimensions. `quality` is CRF 0–51 for libx264; lower values increase quality and file size. `ultrafast` can produce large files.

The optional hardware preset uses `codec: "h264_vaapi"`, `vaapi_device: "/dev/dri/renderD128"`, 30 fps, and `quality` as the encoder's QP value. Choose the correct render device; a multi-GPU machine may expose several. Device visibility and an FFmpeg encoder listing do not prove the driver can encode successfully. Perform a test clip before gaming. HDR preservation and tone mapping are outside this preset: the output is SDR H.264/yuv420p, and HDR monitors need explicit visual verification.

`--framerate 30` asks wf-recorder for constant-frame-rate output. Its default compositor damage handling remains enabled; no extra rendering timer runs in the voice daemon. Measure both the recorder child and the compositor/game, since a frame-rate setting alone does not establish capture cost.

## Ownership, stop and recovery

An idle recorder has no worker thread or timer. While recording, one monitor thread blocks on the owned child's stderr/process exit and keeps only the last 16 KiB of diagnostics in RAM. Repeated start commands return the existing state. It never signals unrelated recorder processes.

The status changes to `recording` once the backend has started; the message accurately says that the backend is running. This is not a guarantee that a first frame was captured. Early failure asynchronously changes it to `error`. Keep an always-visible recording indicator enabled in your status surface when the settings window is closed.

Stop sends SIGINT to the owned child, waits for completion, verifies a nonempty video stream using `ffprobe`, flushes the file, then atomically publishes a final `.mkv` name without overwriting another file. Only then does it report **Saved recording**. This is structural validation, not an exhaustive decode or content/privacy check. If the backend hangs, bounded termination follows and the partial file stays marked as partial. Keep the daemon's normal shutdown handler and user service enabled so owned recordings receive a graceful stop. A forced process or machine shutdown cannot guarantee a final trailer.

Crashes and failed finalization retain nonempty hidden `.Murmur_*.part.mkv` files and show the path. Empty partials are removed. Jarvis does not automatically publish them as successful clips or restart capture after a crash. To inspect/recover a retained partial, preserve the original and try:

```sh
ffprobe -v error -show_streams '/absolute/path/.Murmur_example.part.mkv'
ffmpeg -n -i '/absolute/path/.Murmur_example.part.mkv' -map 0 -c copy '/absolute/path/recovered.mkv'
```

Matroska often permits recovery of completed clusters, but recovery is not guaranteed. `-n` prevents an existing recovery file being overwritten. Play the recovery and check the end of the video before deleting the partial. Opening settings does not recover or delete files.

## Verification record and remaining checks

Read-only inspection found wf-recorder 0.6.0 and FFmpeg with libx264/h264_vaapi encoders. The sandbox cannot connect to Hyprland or PulseAudio; host session probes performed by the main implementation task established Hyprland 0.56.2, PipeWire 1.6.9, an AMD RX 9070 plus Raphael GPU, and two 2560×1440 outputs (`DP-3` HDR at 180 Hz and `DP-5` SDR at 240 Hz). Do not treat those encoder listings as successful hardware-capture tests.

Automated recorder unit tests use a fake child process and no screen/audio access. They cover first-run/source/dependency validation, source typing, duplicate starts, SIGINT ownership, private staging, saved-file validation, crash retention, empty output, spawn failure and a disconnected UI callback.

Run in this repository:

```sh
PYTHONPATH=src python -m unittest discover -s tests -p test_recorder.py -v
hyprctl -j monitors
pactl --format=json list sources
wf-recorder --version
ffmpeg -hide_banner -encoders
```

The synthetic encoder benchmark uses generated pixels and discards output. It never accesses a screen or microphone. Its fresh worker measures the FFmpeg child's CPU time and peak RSS directly, including synthetic input generation. Reproduce the checked-in [sample result](benchmarks/encoder-benchmark.json):

```sh
python scripts/bench_recording.py --seconds 10 --size 1280x720 --fps 30 --codec libx264 --output docs/encoder-benchmark.json
```

It is paced toward real time; FFmpeg startup/initial input buffering means wall time may differ from the requested synthetic duration. Optionally test the media engine with `--codec both --vaapi-device /dev/dri/renderD128`, using the actual render device. This explicitly runs hardware encoding. A successful synthetic run does not validate screen capture or a game. Use `--unthrottled` only for throughput comparisons, not as idle/gameplay CPU evidence.

Actual capture and GUI checks, when run, are recorded separately in the project's verification/performance results. Before relying on gaming capture, verify these procedures on the user session:

- Capture the selected SDR source twice after setup. Check playback and ensure the second start shows no picker/permission prompt and preserves game focus. Repeat after logout/reboot and after a monitor hotplug.
- Record the intended system audio source and microphone separately, check playback/synchronization, then verify your explicitly combined source if using both.
- Start a clip, stop it, and inspect with `ffprobe`; verify the final name appears only after a clean stop. Terminate the test backend unexpectedly and verify the daemon shows an error and retains a partial. Do not kill unrelated recorders.
- Compare no recording, CPU preset and VA-API in a reproducible game scene; report child CPU/RSS, compositor CPU, encoder/graphics usage, disk rate, game average FPS and frame-time percentiles. Capture cost is separate from idle/waiting speech cost.
- Check full disk, disconnected monitor, unavailable audio server, inaccessible render device, denied screencopy permission and suspend/resume. Observe the visible indicator in fullscreen. These failure cases require a real desktop; fake-process tests do not establish their behavior.

Actual short no-audio capture checks completed on DP-5. Both CPU and VA-API
outputs finalized and passed ffprobe, then the temporary test videos were
deleted. The repeated CPU test and the VA-API test preserved focus; an earlier
CPU test observed a focus change whose cause was not established. Three-second
lifecycle measurements were about 196% of one logical CPU for libx264 and 6.47%
for VA-API on renderD128. These are separate from voice costs and do not measure
compositor/game impact. See [performance results](performance.md) and the raw
[VA-API check](benchmarks/desktop-vaapi-integration.json). No post-reboot unattended-capture,
visual-content, audio-sync or game frame-time guarantee is established.
