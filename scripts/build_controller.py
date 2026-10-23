#!/usr/bin/env python3
"""Build the Rust background controller; never changes or starts a service."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    manifest = root / "native/controller/Cargo.toml"
    for action in (["test", "build"] if args.test else ["build"]):
        command = [args.cargo, action, "--release", "--locked", "--manifest-path", str(manifest)]
        if args.offline:
            command.append("--offline")
        subprocess.run(command, check=True)
    binary = manifest.parent / "target/release/jarvis-controller"
    subprocess.run([str(binary), "--version"], check=True)
    target = root / "src/murmur/bin/jarvis-controller"
    target.parent.mkdir(exist_ok=True)
    temporary = target.with_suffix(".tmp")
    shutil.copy2(binary, temporary)
    os.replace(temporary, target)
    print(f"Built {target}")
    print("Run it directly, or select MURMUR_CONTROLLER=rust with the existing daemon launcher.")


if __name__ == "__main__":
    main()
