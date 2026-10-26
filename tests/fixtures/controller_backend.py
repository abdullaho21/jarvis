#!/usr/bin/python3
"""Fake desktop, encoder and replay backends; no desktop/audio/capture access."""
import json
import os
from pathlib import Path
import select
import signal
import socket
import sys
import time

program = Path(sys.argv[0]).name
args = sys.argv[1:]
fd = os.open(os.environ["MURMUR_TEST_LOG"], os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
os.write(fd, (json.dumps({"program": program, "args": args, "pid": os.getpid()}) + "\n").encode())
os.close(fd)
if program in ("hyprctl", "pactl", "pw-dump"):
    time.sleep(float(os.environ.get("MURMUR_TEST_QUERY_DELAY", "0")))

if program == "hyprctl":
    if args == ["-j", "monitors"]:
        print(json.dumps([{"name": "DP-1", "width": 1920, "height": 1080, "focused": True,
                           "colorManagementPreset": os.environ.get("MURMUR_TEST_COLOR", "sdr")}]))
    elif args == ["-j", "clients"]:
        print("[]")
    else:
        print("ok")
elif program == "pw-dump":
    print(json.dumps([{"info": {"props": {"media.class": "Audio/Source", "node.name": "fake-mic", "node.description": "Fake input"}}}]))
elif program == "pactl":
    print(json.dumps([{"name": "fake-mic", "description": "Fake input"}, {"name": "fake.monitor", "monitor_of_sink": 1}]))
elif program == "ffprobe":
    time.sleep(float(os.environ.get("MURMUR_TEST_VERIFY_DELAY", "0")))
    hdr = os.environ.get("MURMUR_TEST_COLOR") == "hdr" and os.environ.get("MURMUR_TEST_BAD_HDR") != "1"
    print(json.dumps({"streams": [{"codec_type": "video", "codec_name": os.environ.get("MURMUR_TEST_CODEC", "h264"), "width": 1920, "height": 1080,
                                   "pix_fmt": "yuv420p10le" if hdr else "yuv420p",
                                   "color_transfer": "smpte2084" if hdr else "bt709",
                                   "color_primaries": "bt2020" if hdr else "bt709",
                                   "color_space": "bt2020nc" if hdr else "bt709", "color_range": "tv"}]}))
elif program in ("wf-recorder", "gpu-screen-recorder"):
    if args == ["--version"]:
        print("6.1.5")
        sys.exit(0)
    signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
    if "-ipc" in args:
        ram_fixture = b"x" * int(os.environ.get("MURMUR_TEST_REPLAY_RAM_BYTES", "0"))
        if "-replay-storage" in args and args[args.index("-replay-storage") + 1] == "disk":
            buffer = Path(args[args.index("-o") + 1]) / "gsr-replay-fake.gsr"
            buffer.mkdir(mode=0o700)
            with (buffer / "Replay_0.gsr").open("wb") as f:
                f.truncate(int(os.environ.get("MURMUR_TEST_REPLAY_BYTES", "1024")))
        if os.environ.get("MURMUR_TEST_REPLAY_WRITE_ERROR") == "1":
            sys.stderr.write("gsr_encoder_receive_packets: failed to add replay buffer data\n")
            sys.stderr.flush()
        path = Path(args[args.index("-ipc") + 1])
        path.unlink(missing_ok=True)
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(path))
        server.listen()
        while True:
            if not select.select([server], [], [], 1)[0]:
                continue
            connection, _ = server.accept()
            with connection, connection.makefile("rwb") as stream:
                request = json.loads(stream.readline())
                destination = "-ro" if "-ro" in args else "-o"
                output = Path(args[args.index(destination) + 1]) / "Replay_test.mp4"
                output.write_bytes(b"fake video")
                stream.write((json.dumps({"id": request["id"], "result": "ok", "data": str(output)}) + "\n").encode())
                stream.flush()
    else:
        output = Path(args[args.index("--file") + 1] if "--file" in args else args[args.index("-o") + 1])
        output.write_bytes(b"fake video")
        if os.environ.get("MURMUR_TEST_CAPTURE_CRASH") == "1":
            sys.exit(4)
        while True:
            signal.pause()
