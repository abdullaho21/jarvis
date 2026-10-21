"""Private Unix-socket JSON IPC. No network listeners."""
import json
import os
from pathlib import Path
import socket


def socket_path() -> str:
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    base = Path(runtime) / "murmur" if runtime else Path(f"/tmp/murmur-{os.getuid()}")
    return os.environ.get("MURMUR_SOCKET", str(base / "control.sock"))


# Operations that can legitimately take longer than a status query. Stopping a
# recording waits up to ~16 s for wf-recorder, then validates the file with ffprobe.
TIMEOUTS = {"record_stop": 45, "record_start": 20, "execute": 45, "save_config": 30,
            "set_startup": 15, "apps": 20}


def request(op: str, timeout: float | None = None, **kwargs) -> dict:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(timeout or TIMEOUTS.get(op, 12))
        sock.connect(socket_path())
        sock.sendall((json.dumps({"op": op, **kwargs}) + "\n").encode())
        with sock.makefile("rb") as f:
            line = f.readline(1048577)
        if not line or len(line) > 1048576:
            raise ConnectionError("Invalid response from Murmur service")
        return json.loads(line)


def watch(callback, stop_event):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.connect(socket_path())
        sock.sendall(b'{"op":"watch"}\n')
        with sock.makefile("rb") as f:
            while not stop_event.is_set():
                line = f.readline(65537)
                if not line or len(line) > 65536:
                    break
                callback(json.loads(line))
