"""Download verified upstream sources/toolchain into an isolated build folder."""

import argparse
import hashlib
import json
from pathlib import Path
import tarfile
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    root = args.work_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    manifest = Path(__file__).resolve().parents[1] / "packaging/windows/toolchain.json"
    pins = json.loads(manifest.read_text())
    names = {
        "ffmpeg": "ffmpeg-",
        "llvm": "llvm-mingw-",
        "vpl": "vpl-",
        "nvcodec": "nvcodec-",
        "amf": "amf-",
        "x264": "x264-",
    }
    paths = {}
    for key, prefix in names.items():
        pin = next(item for item in pins if item["name"].startswith(prefix))
        archive = root / "downloads" / pin["name"]
        archive.parent.mkdir(exist_ok=True)
        if not archive.exists():
            temporary = archive.with_suffix(archive.suffix + ".part")
            urllib.request.urlretrieve(pin["url"], temporary)
            temporary.rename(archive)
        with archive.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if digest != pin["sha256"]:
            raise ValueError(f"Checksum mismatch: {archive}")
        destination = root / "sources" / key
        destination.mkdir(parents=True, exist_ok=True)
        with tarfile.open(archive) as bundle:
            members = bundle.getmembers()
            if key == "amf":
                members = [
                    m
                    for m in members
                    if "/amf/public/include/" in m.name
                    or m.name.endswith("/LICENSE.txt")
                ]
            bundle.extractall(destination, members=members, filter="data")
        folders = list(destination.iterdir())
        if len(folders) != 1:
            raise ValueError(f"Unexpected source layout: {destination}")
        paths[key] = str(folders[0].relative_to(root))
    (root / "source-paths.json").write_text(json.dumps(paths, indent=2) + "\n")
    print("Verified sources ready:", root)


if __name__ == "__main__":
    main()
