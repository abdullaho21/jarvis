#!/usr/bin/env python3
"""Build and install the native CLI in this checkout; never starts a service."""
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
    manifest = root / "native/cli/Cargo.toml"
    for action in (["test", "build"] if args.test else ["build"]):
        command = [args.cargo, action, "--release", "--locked", "--manifest-path", str(manifest)]
        if args.offline:
            command.append("--offline")
        subprocess.run(command, check=True)
    binary = manifest.parent / "target/release/jarvis"
    subprocess.run([str(binary), "--version"], check=True)
    target = root / "src/murmur/bin/jarvis"
    target.parent.mkdir(exist_ok=True)
    temporary = target.with_suffix(".tmp")
    shutil.copy2(binary, temporary)
    os.replace(temporary, target)
    print(f"Built {target}")
    print("run-jarvis now uses the native CLI. MURMUR_CLI=python selects the compatibility client.")


if __name__ == "__main__":
    main()
