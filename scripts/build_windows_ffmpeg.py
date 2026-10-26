"""Linux cross-build recipe; no capture, audio devices or network are used here."""

import json
import os
import shutil
import subprocess
from pathlib import Path
import argparse

parser = argparse.ArgumentParser(
    description="Cross-build Windows FFmpeg from the prepared, pinned sources"
)
parser.add_argument("--work-dir", type=Path, required=True)
args = parser.parse_args()
root = args.work_dir.absolute()
if " " in str(root):
    raise SystemExit("Use a build directory without spaces (or a symlink to one)")
paths = json.loads((root / "source-paths.json").read_text())
paths = {k: str(root / v) if not Path(v).is_absolute() else v for k, v in paths.items()}
# LLVM-MinGW does not define _MSC_VER. Keep the old-MSVC fallback from
# shadowing the modern SDK's secure-CRT overloads.
header = Path(paths["vpl"]) / "libvpl/src/windows/mfx_dispatcher_defs.h"
header.write_text(
    header.read_text().replace(
        "#if _MSC_VER < 1400", "#if defined(_MSC_VER) && _MSC_VER < 1400"
    )
)
llvm = Path(paths["llvm"])
prefix = root / "ffmpeg-prefix"
prefix.mkdir(exist_ok=True)
env = dict(
    os.environ,
    PATH=str(llvm / "bin") + os.pathsep + os.environ["PATH"],
    PKG_CONFIG_LIBDIR=str(prefix / "lib/pkgconfig"),
)
subprocess.run(
    ["make", "install", f"PREFIX={prefix}"], cwd=paths["nvcodec"], env=env, check=True
)
include = prefix / "include/AMF"
include.parent.mkdir(exist_ok=True)
shutil.copytree(Path(paths["amf"]) / "amf/public/include", include, dirs_exist_ok=True)
toolchain = root / "mingw.cmake"
toolchain.write_text(f'''set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER "{llvm}/bin/x86_64-w64-mingw32-clang")
set(CMAKE_CXX_COMPILER "{llvm}/bin/x86_64-w64-mingw32-clang++")
set(CMAKE_RC_COMPILER "{llvm}/bin/x86_64-w64-mingw32-windres")
set(CMAKE_FIND_ROOT_PATH "{llvm}/x86_64-w64-mingw32")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
''')
subprocess.run(
    [
        "cmake",
        "-S",
        paths["vpl"],
        "-B",
        str(root / "vpl-build-safe"),
        "-G",
        "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={toolchain}",
        f"-DCMAKE_INSTALL_PREFIX={prefix}",
        "-DCMAKE_INSTALL_LIBDIR=lib",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DBUILD_TESTS=OFF",
        "-DBUILD_EXAMPLES=OFF",
        "-DBUILD_TOOLS=OFF",
    ],
    env=env,
    check=True,
)
subprocess.run(
    ["cmake", "--build", str(root / "vpl-build-safe"), "-j", "4"], env=env, check=True
)
subprocess.run(
    ["cmake", "--install", str(root / "vpl-build-safe")], env=env, check=True
)
x264 = root / "x264-build"
x264.mkdir(exist_ok=True)
subprocess.run(
    [
        paths["x264"] + "/configure",
        "--host=x86_64-w64-mingw32",
        f"--cross-prefix={llvm}/bin/x86_64-w64-mingw32-",
        f"--prefix={prefix}",
        "--enable-static",
        "--disable-cli",
        "--disable-opencl",
        "--disable-lavf",
        "--disable-swscale",
    ],
    cwd=x264,
    env=dict(env, CC=str(llvm / "bin/x86_64-w64-mingw32-clang")),
    check=True,
)
subprocess.run(["make", "-j", "4"], cwd=x264, env=env, check=True)
subprocess.run(["make", "install"], cwd=x264, env=env, check=True)
build = root / "ffmpeg-build"
build.mkdir(exist_ok=True)
args = [
    paths["ffmpeg"] + "/configure",
    "--target-os=mingw32",
    "--arch=x86_64",
    "--enable-cross-compile",
    f"--cross-prefix={llvm}/bin/x86_64-w64-mingw32-",
    f"--cc={llvm}/bin/x86_64-w64-mingw32-clang",
    f"--cxx={llvm}/bin/x86_64-w64-mingw32-clang++",
    f"--prefix={root}/ffmpeg-tools",
    "--disable-autodetect",
    "--disable-filter=gfxcapture",
    "--disable-network",
    "--enable-gpl",
    "--enable-version3",
    "--enable-libx264",
    "--disable-nonfree",
    "--disable-doc",
    "--disable-debug",
    "--disable-ffplay",
    "--enable-static",
    "--disable-shared",
    "--enable-d3d11va",
    "--enable-dxva2",
    "--enable-mediafoundation",
    "--enable-amf",
    "--enable-nvenc",
    "--enable-ffnvcodec",
    "--enable-libvpl",
    f"--extra-cflags=-I{prefix}/include",
    f"--extra-ldflags=-L{prefix}/lib",
    "--extra-libs=-l:libc++.a -l:libc++abi.a -l:libunwind.a",
    "--pkg-config=pkg-config",
    "--pkg-config-flags=--static",
]
(root / "ffmpeg-configure.json").write_text(json.dumps(args, indent=2))
subprocess.run(args, cwd=build, env=env, check=True)
subprocess.run(["make", "-j", "4"], cwd=build, env=env, check=True)
subprocess.run(["make", "install"], cwd=build, env=env, check=True)
