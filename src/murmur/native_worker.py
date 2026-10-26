"""Prefer the available Rust worker without importing inference libraries."""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import sys

from .commands import GREETINGS, POLITE_PREFIXES, POLITE_SUFFIXES, match_candidates, all_wake_variants
from .models import (ACCURATE_MODELS, DEFAULT_ACCURATE_MODEL, accurate_model_dir,
                     accurate_model_ready, default_model_path, denoiser_path, valid_model)

# The Rust worker loads Whisper when it hears a wake name and frees it after
# this long without a command; waiting for the wake phrase needs only Vosk.
WHISPER_IDLE_SECONDS = 60


def package_dir(name: str) -> Path | None:
    spec = importlib.util.find_spec(name)
    return Path(spec.origin).parent if spec and spec.origin else None


def binary_path() -> Path | None:
    packaged = Path(__file__).with_name("bin") / "jarvis-voice"
    checkout = Path(__file__).resolve().parents[2] / "native/voice-worker/target/release/jarvis-voice"
    return next((p for p in (packaged, checkout) if p.is_file() and os.access(p, os.X_OK)), None)


def prepare(config: dict, mode: str, grammar: dict) -> tuple[list[str], dict, dict]:
    """Prefer Rust on Linux; explicit environment overrides select either worker.

    Paths come from the installed packages and model discovery, never from a
    command string. Python supports Windows, PocketSphinx and source-only installs.
    """
    from .host import command, is_windows
    if is_windows():
        return command("voice"), {"config": config, "mode": mode, "grammar": grammar}, dict(os.environ, MURMUR_OWNER_PID=str(os.getpid()))
    selected = os.environ.get("MURMUR_VOICE_WORKER")
    payload = {"config": config, "mode": mode, "grammar": grammar}
    env = dict(os.environ, MURMUR_OWNER_PID=str(os.getpid()))
    if selected == "python":
        return [sys.executable, "-m", "murmur.voice"], payload, env
    if selected not in (None, "rust"):
        raise ValueError("MURMUR_VOICE_WORKER must be python or rust")
    binary = binary_path()
    settings = config["recognition"]
    if selected is None and (binary is None or settings["backend"] == "pocketsphinx"):
        return [sys.executable, "-m", "murmur.voice"], payload, env
    if binary is None:
        raise ValueError("Rust voice worker missing. Run scripts/build_voice_worker.py first.")
    if mode != "test" and settings["backend"] == "pocketsphinx":
        raise ValueError("The Rust worker requires Vosk; select auto/vosk or use the Python worker.")
    model = Path(settings.get("model_path") or default_model_path()).expanduser()
    vosk = package_dir("vosk")
    if mode != "test" and (not vosk or not (vosk / "libvosk.so").is_file() or not valid_model(model)):
        if selected is None:
            return [sys.executable, "-m", "murmur.voice"], payload, env
        raise ValueError("Rust voice worker requires the local Vosk package and model.")
    native = {"vosk_library": str(vosk / "libvosk.so") if vosk else "",
              "model_path": str(model), "whisper": None, "denoiser": None,
              "threads": max(1, min(8, (os.cpu_count() or 2) // 2)),
              "candidates": list(match_candidates(grammar).items()),
              "wake_forms": all_wake_variants(config["activation"]),
              "greetings": list(GREETINGS), "prefixes": list(POLITE_PREFIXES),
              "suffixes": list(POLITE_SUFFIXES), "whisper_idle_seconds": WHISPER_IDLE_SECONDS}
    if settings["accurate"] and accurate_model_ready():
        folder = accurate_model_dir()
        native["whisper"] = {k: str(folder / v) for k, v in ACCURATE_MODELS[DEFAULT_ACCURATE_MODEL]["files"].items()}
        if settings["noise_suppression"] and denoiser_path().is_file():
            native["denoiser"] = str(denoiser_path())
    payload["native"] = native
    return [str(binary)], payload, env
