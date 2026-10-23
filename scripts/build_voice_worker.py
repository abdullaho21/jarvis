#!/usr/bin/env python3
"""Build the optional Rust worker, Whisper recognizer and GTCRN denoiser.

Requires Cargo/Rust. No Python or C++ inference libraries are linked. Models
are not downloaded. --test runs Rust unit tests before installing the binary.
"""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", default="cargo", help="Cargo executable (default: PATH)")
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--offline", action="store_true", help="Use already cached Cargo dependencies")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    manifest = root / "native/voice-worker/Cargo.toml"
    denoiser_manifest = root / "native/gtcrn-denoiser/Cargo.toml"
    whisper_manifest = root / "native/whisper-recognizer/Cargo.toml"
    for standalone in (denoiser_manifest, whisper_manifest):
        for action in (["test", "build"] if args.test else ["build"]):
            command = [args.cargo, action, "--release", "--locked", "--manifest-path", str(standalone)]
            if args.offline:
                command.append("--offline")
            subprocess.run(command, env=env, check=True)
    for action in (["test", "build"] if args.test else ["build"]):
        command = [args.cargo, action, "--release", "--locked", "--manifest-path", str(manifest)]
        if args.offline:
            command.append("--offline")
        subprocess.run(command, env=env, check=True)
    binary = root / "native/voice-worker/target/release/jarvis-voice"
    subprocess.run([str(binary), "--version"], env=env, check=True)
    destination = root / "src/murmur/bin/jarvis-voice"
    destination.parent.mkdir(exist_ok=True)
    temporary = destination.with_suffix(".tmp")
    shutil.copy2(binary, temporary)
    temporary.replace(destination)
    for crate, name in (("gtcrn-denoiser", "jarvis-denoise"), ("whisper-recognizer", "jarvis-whisper")):
        target = destination.with_name(name)
        temporary = target.with_suffix(".tmp")
        shutil.copy2(root / "native" / crate / "target/release" / name, temporary)
        temporary.replace(target)
    print(f"Built {destination}")
    print("Select it with MURMUR_VOICE_WORKER=rust; Python remains the default.")


if __name__ == "__main__":
    main()
