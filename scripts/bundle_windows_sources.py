"""Package the exact FFmpeg/library source trees and build recipe for distribution."""

import argparse
import json
import os
from pathlib import Path
import tarfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    work = args.work_dir.resolve()
    paths = json.loads((work / "source-paths.json").read_text())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_suffix(".tmp")
    try:
        with tarfile.open(temporary, "w:xz", preset=3) as bundle:
            for key in ("ffmpeg", "vpl", "amf", "nvcodec", "x264"):
                path = Path(paths[key])
                if not path.is_absolute():
                    path = work / path
                bundle.add(
                    path,
                    arcname=f"FFmpeg-Corresponding-Source/sources/{key}/{path.name}",
                )
            for file in (
                "scripts/build_windows_ffmpeg.py",
                "scripts/prepare_windows_ffmpeg.py",
                "packaging/windows/toolchain.json",
            ):
                bundle.add(root / file, arcname="FFmpeg-Corresponding-Source/" + file)
            bundle.add(
                work / "ffmpeg-configure.json",
                arcname="FFmpeg-Corresponding-Source/original-configure.json",
            )
        os.replace(temporary, args.output)
    finally:
        temporary.unlink(missing_ok=True)
    print(args.output)


if __name__ == "__main__":
    main()
