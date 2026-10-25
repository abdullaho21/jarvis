"""Explicit per-user startup and background launch; never needs elevation."""

import os
from pathlib import Path
import subprocess
import sys
from ..host import app_dir, child_options, command, data_dir


def startup_argv():
    if getattr(sys, "frozen", False):
        return [str(app_dir() / "Jarvis.exe"), "--background"]
    python = Path(sys.executable).with_name("pythonw.exe")
    return [
        str(python if python.exists() else Path(sys.executable)),
        "-m",
        "murmur",
        "--background",
    ]


def set_startup(enabled):
    import winreg

    with winreg.CreateKey(
        winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run"
    ) as key:
        if enabled:
            winreg.SetValueEx(
                key, "Jarvis", 0, winreg.REG_SZ, subprocess.list2cmdline(startup_argv())
            )
        else:
            try:
                winreg.DeleteValue(key, "Jarvis")
            except FileNotFoundError:
                pass


def start_service():
    from ..client import request

    try:
        if request("status", timeout=1).get("ok"):
            return
    except (OSError, ValueError, RuntimeError):
        pass
    folder = data_dir() / "logs"
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / "daemon.log"
    if path.exists() and path.stat().st_size > 1024 * 1024:
        os.replace(path, path.with_suffix(".log.1"))
    with path.open("ab", buffering=0) as output:
        subprocess.Popen(
            command("daemon"),
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=output,
            **child_options(independent=True),
        )


def stop_service():
    """Wait for the service process to exit before an installer replaces files."""
    from ..client import request

    try:
        result = request("shutdown", timeout=3)
    except (FileNotFoundError, ConnectionRefusedError):
        return {"ok": True, "message": "Jarvis service is already stopped"}
    if not result.get("ok"):
        raise RuntimeError(result.get("error", "Jarvis could not stop"))
    import pywintypes
    import win32api
    import win32con
    import win32event

    try:
        process = win32api.OpenProcess(win32con.SYNCHRONIZE, False, result["pid"])
    except pywintypes.error as exc:
        if exc.winerror == 87:  # process already exited
            return result
        raise
    try:
        if win32event.WaitForSingleObject(process, 45000) != win32event.WAIT_OBJECT_0:
            raise RuntimeError(
                "Jarvis is still finishing capture; wait before updating or uninstalling"
            )
    finally:
        process.Close()
    return result
