"""Optional short local tones, played only on recording state changes."""
import math
import shutil
import struct
import subprocess


def play(state: str):
    player = shutil.which("pw-play")
    if not player:
        return
    frequency = {"recording": 880, "idle": 660, "error": 220}.get(state, 440)
    frames = 2400  # 100 ms at 24 kHz; generated feedback, never microphone data
    pcm = b"".join(struct.pack("<h", int(1800 * math.sin(2 * math.pi * frequency * i / 24000)
                                       * min(1, i / 120, (frames - i) / 120))) for i in range(frames))
    try:
        subprocess.run([player, "--raw", "--rate", "24000", "--channels", "1", "--format", "s16", "-"],
                       input=pcm, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
    except (OSError, subprocess.TimeoutExpired):
        pass  # optional feedback cannot break capture/finalization
