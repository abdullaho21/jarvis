# Changelog

## 0.2.0 — 2026-10-26

### New

- **Windows 10/11 x64**: per-user installer and portable ZIP with the offline
  models, FFmpeg and license notices. Native settings window, tray icon,
  microphone and system-sound capture, GPU recording and an in-RAM instant
  replay. Tested under Wine, not yet on real Windows hardware; see
  [Windows setup](docs/windows.md).
- **Brightness by voice**: name each monitor in **Displays**, pick a main
  monitor and say “dim Samsung” or “set KTC brightness to 60 percent”. Uses
  `ddcutil` on Linux and WMI or DDC/CI on Windows.
- **Custom wake names**: up to eight, such as “Alfred” alongside “Jarvis”.
- **Launch words**: each app voice name can use its own verb (“watch video”)
  or none (“Jarvis, Firefox”).
- **Recording choices**: GPU H.264, HEVC or AV1, size or quality budgets, an
  FPS slider with storage estimates, and HDR with AV1 as well as HEVC.
- **One-line Arch Linux installer** (`install.sh`) that also updates and
  uninstalls.

### Faster and lighter

- Whisper recognition and GTCRN noise suppression run in a Rust voice worker,
  preferred on Linux when built.
- Whisper loads only after the wake word and unloads after 60 seconds idle:
  RAM while waiting fell from 469 to 156 MiB, and the worker is ready in
  0.2 s instead of 1.9 s.
- Whisper model loading streams its weights: startup peak fell from 529 to
  344 MiB.
- The idle worker returns freed memory every 10 seconds, which reduces RAM
  creep during long sessions.
- Optional Rust background controller and a Rust `jarvis` command-line client.

## 0.1.0 — 2026-10-21

First public version: offline voice control for Hyprland on Linux with Vosk
wake detection and Whisper small.en commands, app launching and aliases,
workspaces, fullscreen, volume, screen recording with HDR, instant replay and
a Clips page, a GTK settings window and an optional user service.
