#!/usr/bin/env python3
"""Benchmark a synthetic encoder workload. Never accesses a screen or microphone.

This measures FFmpeg encoding, not wf-recorder capture, compositor cost, or gaming
performance. Each measurement gets a fresh worker so child resource usage and
peak RSS belong to that FFmpeg process alone. Linux reports RSS in KiB.
"""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import re
import resource
import shutil
import subprocess
import sys
import time


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=float, default=5)
    parser.add_argument("--size", default="1280x720")
    parser.add_argument("--fps", type=int, default=30)
    parser.add_argument("--quality", type=int, default=23)
    parser.add_argument("--codec", choices=("libx264", "h264_vaapi", "both"), default="libx264")
    parser.add_argument("--vaapi-device", default="/dev/dri/renderD128")
    parser.add_argument("--unthrottled", action="store_true", help="Measure throughput instead of a realtime-paced input.")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 0.1 <= args.seconds <= 300:
        parser.error("--seconds must be between 0.1 and 300")
    if not re.fullmatch(r"[1-9][0-9]{1,3}x[1-9][0-9]{1,3}", args.size):
        parser.error("--size must be WIDTHxHEIGHT, e.g. 1280x720")
    width, height = map(int, args.size.split("x"))
    if width > 7680 or height > 4320 or width % 2 or height % 2:
        parser.error("dimensions must be even, up to 7680x4320")
    if not 1 <= args.fps <= 120 or not 0 <= args.quality <= 51:
        parser.error("--fps must be 1–120 and --quality 0–51")
    return args


def worker(args):
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return {"codec": args.codec, "error": "ffmpeg is missing"}
    argv = [ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error"]
    if args.codec == "h264_vaapi":
        if not os.access(args.vaapi_device, os.R_OK | os.W_OK):
            return {"codec": args.codec, "error": "VA-API render device unavailable or inaccessible"}
        argv += ["-vaapi_device", args.vaapi_device]
    if not args.unthrottled:
        argv += ["-re"]
    argv += ["-f", "lavfi", "-i", f"testsrc2=size={args.size}:rate={args.fps}",
             "-t", str(args.seconds), "-an", "-c:v", args.codec]
    if args.codec == "libx264":
        argv += ["-preset", "ultrafast", "-crf", str(args.quality), "-threads", "2",
                 "-pix_fmt", "yuv420p"]
    else:
        argv += ["-vf", "format=nv12,hwupload", "-qp", str(args.quality)]
    argv += ["-f", "null", "-"]
    began = time.monotonic()
    try:
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.PIPE, text=True, timeout=args.seconds + 60,
                                check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"codec": args.codec, "error": str(exc)}
    wall = time.monotonic() - began
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = usage.ru_utime + usage.ru_stime
    return {"codec": args.codec, "ok": result.returncode == 0, "returncode": result.returncode,
            "wall_seconds": round(wall, 4), "cpu_seconds": round(cpu, 4),
            "average_cpu_percent_one_logical_cpu": round(100 * cpu / wall, 2),
            "peak_rss_mib": round(usage.ru_maxrss / 1024, 2),
            "error": result.stderr[-2000:] if result.returncode else None,
            "command": argv}


def main():
    args = arguments()
    if args.worker:
        print(json.dumps(worker(args)))
        return
    codecs = ["libx264", "h264_vaapi"] if args.codec == "both" else [args.codec]
    measurements = []
    for codec in codecs:
        argv = [sys.executable, str(Path(__file__).resolve()), "--worker", "--codec", codec,
                "--seconds", str(args.seconds), "--size", args.size, "--fps", str(args.fps),
                "--quality", str(args.quality), "--vaapi-device", args.vaapi_device]
        if args.unthrottled:
            argv += ["--unthrottled"]
        result = subprocess.run(argv, capture_output=True, text=True, check=False,
                                timeout=args.seconds + 70)
        if result.returncode:
            measurements.append({"codec": codec, "error": result.stderr[-2000:]})
        else:
            measurements.append(json.loads(result.stdout))
    version = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True,
                             check=False).stdout.splitlines()[0] if shutil.which("ffmpeg") else "missing"
    report = {"measured_at_utc": datetime.now(timezone.utc).isoformat(),
              "scope": "FFmpeg synthetic source generation, encoding and null output; no display/audio capture; excludes compositor and game impact.",
              "method": "Fresh worker per encoder; getrusage(RUSAGE_CHILDREN) CPU and peak RSS; wall includes initialization and finalization.",
              "platform": platform.platform(), "logical_cpus": os.cpu_count(), "ffmpeg": version,
              "input": {"size": args.size, "fps": args.fps, "seconds": args.seconds,
                        "realtime_paced": not args.unthrottled, "quality": args.quality},
              "measurements": measurements}
    encoded = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end="")
    if any(not item.get("ok", False) for item in measurements):
        sys.exit(1)


if __name__ == "__main__":
    main()
