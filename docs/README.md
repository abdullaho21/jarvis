# Documentation

Start with the root [README](../README.md) for installation and everyday use.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Process ownership, voice path, bounds and trust boundaries |
| [Performance](performance.md) | Current CPU/RAM measurements and reproduction |
| [Memory and background check](memory-background.md) | Allocator comparison and live background verification |
| [Review handoff](review-ready.md) | Where to review and what remains unverified |
| [Voice models](speech-research.md) | Current model path and public test fixture |
| [Rust voice worker](rust-voice.md) | Optional native worker, build/selection and measured memory comparison |
| [Rust denoiser](rust-denoiser.md) | Rust GTCRN computation, standalone binary and waveform verification |
| [Rust Whisper](rust-whisper.md) | Rust recognition computation, cache reconstruction and memory/latency tradeoffs |
| [Voice-worker memory](rust-worker-memory.md) | Process memory audit and lossless Whisper embedding compaction |
| [On-demand Whisper](whisper-on-demand.md) | Loading Whisper on a wake name, idle release, and measured RAM/latency tradeoff |
| [Rust controller](rust-controller.md) | Native service API, process ownership, compatibility checks and controller memory comparison |
| [Rust CLI](rust-cli.md) | Native controls, Waybar, aliases, diagnostics and verified setup |
| [Recording](recording.md) | Permissions, audio sources, encoders and recovery |
| [UI](ui.md) | Native settings interface and accessibility checks |
| [Troubleshooting](troubleshooting.md) | Operational errors and recovery |
| [Verification](verification.md) | Automated and interactive acceptance checks |
| [Dependencies](dependencies.md) | Model provenance and third-party notices |

Raw measurement JSON lives in [benchmarks/](benchmarks/). Earlier engineering
notes are preserved in [history/](history/) and describe their dated versions.
They do not override current setup instructions or performance claims.
