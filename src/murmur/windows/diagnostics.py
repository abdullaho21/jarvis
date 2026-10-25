"""Read-only Windows checks. Never opens an audio stream or captures a frame."""

import importlib.util
import platform
import subprocess
import sys
from ..host import app_dir, child_options, tool


def report():
    from ..doctor import _voice_engine

    tools = {}
    for name in ("ffmpeg", "ffprobe"):
        binary = tool(name)
        try:
            result = (
                subprocess.run(
                    [binary, "-version"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                    **child_options(),
                )
                if binary
                else None
            )
            tools[name] = {
                "path": binary,
                "ok": bool(result and result.returncode == 0),
                "version": result.stdout.splitlines()[:1] if result else [],
            }
        except (OSError, subprocess.TimeoutExpired) as exc:
            tools[name] = {"path": binary, "ok": False, "error": str(exc)}
    return {
        "system": platform.platform(),
        "python": platform.python_version(),
        "architecture": platform.machine(),
        "windows_build": sys.getwindowsversion().build,
        "app_dir": str(app_dir()),
        "frozen": bool(getattr(sys, "frozen", False)),
        "voice_engine": _voice_engine(),
        "tools": tools,
        "dependencies": {
            name: importlib.util.find_spec(name) is not None
            for name in (
                "win32api",
                "pyaudiowpatch",
                "pycaw",
                "pyvda",
                "screen_brightness_control",
                "sherpa_onnx",
                "vosk",
                "tkinter",
            )
        },
        "note": "Presence checks only. Test microphone, desktop control, GPU encoding and HDR on Windows hardware before publishing.",
    }
