"""Event-driven service. GTK and speech libraries are never imported here."""
from __future__ import annotations

import asyncio
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import threading

from . import config as configuration
from .actions import Dispatcher
from .apps import AppRegistry
from .client import socket_path
from .commands import REFERENCE, CommandError, Parser, normalize
from .hyprland import Hyprland
from .recorder import Recorder, audio_sources
from .state import Activation
from .desktop_process import desktop_argv


class Service:
    def __init__(self, config=None):
        self.config = configuration.validate(config) if config is not None else configuration.load()
        self.apps = AppRegistry(self.config["aliases"])
        self.parser = Parser(self.config["commands"])
        a = self.config["activation"]
        self.activation = Activation(a["command_timeout"], a["confidence"])
        self.desktop = Hyprland()
        self.recorder = Recorder(self.config, self.recording_changed)
        self.muted = not self.config["privacy"]["onboarded"]
        self.message = "Complete first-run microphone consent in Settings" if self.muted else "Starting voice control"
        self.worker = self.voice_task = None
        self.worker_mode = None
        self.watchers = set()
        self.loop = None
        self.level = 0
        self.unsupported = []
        self.last_recording_state = "idle"
        self.action_lock = asyncio.Lock()
        self.worker_lock = asyncio.Lock()
        self.stopping = asyncio.Event()
        self.tasks: set[asyncio.Task] = set()
        self.voice_available = (importlib.util.find_spec("pocketsphinx") is not None
                                or importlib.util.find_spec("vosk") is not None)
        self.engine = None
        self.gate = None
        self.dispatcher = Dispatcher(self.apps, self.desktop, self.recorder, self.pause_from_action, self.open_ui)

    def status(self):
        return {"state": self.activation.state, "message": self.message, "muted": self.muted,
                "recording": self.recorder.status(), "level": self.level,
                "voice_available": self.voice_available,
                "unsupported_phrases": self.unsupported, "engine": self.engine, "gate": self.gate,
                "dry_run": os.environ.get("MURMUR_DRY_RUN") == "1",
                "microphone_open": self.worker is not None,
                "mic_test": self.worker is not None and self.worker_mode == "test"}

    def publish(self):
        message = {"ok": True, "status": self.status()}
        for queue in tuple(self.watchers):
            if queue.full():
                queue.get_nowait()
            queue.put_nowait(message)

    def voice_grammar(self) -> dict:
        """Command grammar plus which desktop entry each spoken app name opens."""
        grammar = self.parser.grammar(list(self.apps.index))
        app_ids = {}
        for name in self.apps.index:
            try:
                app_ids[name] = self.apps.resolve(name).id
            except CommandError:
                app_ids[name] = name
        grammar["app_ids"] = app_ids
        return grammar

    def background(self, coroutine, what: str) -> asyncio.Task:
        """Run a coroutine without awaiting it, but keep a reference (asyncio only
        holds weak references to tasks) and surface its failure in the status."""
        task = asyncio.create_task(coroutine)
        self.tasks.add(task)
        def finished(done: asyncio.Task) -> None:
            self.tasks.discard(done)
            if done.cancelled() or done.exception() is None:
                return
            exc = done.exception()
            print(f"murmur: {what} failed: {exc!r}", file=sys.stderr, flush=True)
            if what == "voice restart":
                self.activation.error()
            self.message = f"{what.capitalize()} failed: {exc}"
            self.publish()
        task.add_done_callback(finished)
        return task

    def recording_changed(self, _status):
        if self.loop:
            self.loop.call_soon_threadsafe(self.on_recording_changed, _status)

    def on_recording_changed(self, status):
        state = status["state"]
        if state != self.last_recording_state and state in ("recording", "idle", "error") and self.config["recording"].get("sound_feedback", False):
            from .feedback import play
            self.background(asyncio.to_thread(play, state), "recording sound")
        self.last_recording_state = state
        self.publish()

    def pause_from_action(self):
        self.loop.call_soon_threadsafe(lambda: self.background(self.set_mute(True), "mute"))

    def open_ui(self, page):
        # Separate process and no UI import retained by the service.
        proc = subprocess.Popen(desktop_argv([sys.executable, "-m", "murmur", "ui", "--page", page]),
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
        threading.Thread(target=proc.wait, daemon=True, name="murmur-ui-reaper").start()

    async def stop_worker(self):
        async with self.worker_lock:
            await self._stop_worker_locked()

    async def _stop_worker_locked(self):
        proc = self.worker
        self.worker = None
        if proc and proc.returncode is None:
            proc.terminate()
            try:
                await asyncio.wait_for(proc.wait(), 3)
            except TimeoutError:
                proc.kill()
                await proc.wait()
        task = self.voice_task
        self.voice_task = None
        if task and task is not asyncio.current_task():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        self.level = 0

    async def start_worker(self, mode):
        if self.muted or not self.config["privacy"]["onboarded"]:
            raise CommandError("Microphone is muted. Complete consent and unmute in Settings.")
        async with self.worker_lock:
            if self.muted or not self.config["privacy"]["onboarded"]:
                raise CommandError("Microphone is muted. Complete consent and unmute in Settings.")
            if mode == "wake_word" and self.config["activation"]["mode"] != "wake_word":
                return  # a queued resume must not outlive a switch to PTT
            await self._stop_worker_locked()
            self.message = "Opening microphone…"
            if mode == "once":
                self.activation.arm()
            payload = {"config": self.config, "mode": mode, "grammar": self.voice_grammar()}
            encoded = (json.dumps(payload) + "\n").encode()
            if len(encoded) > 1048576:
                raise CommandError("Voice grammar exceeds the 1 MiB worker limit")
            self.worker = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "murmur.voice", stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL, limit=65536,
                env=dict(os.environ, MURMUR_OWNER_PID=str(os.getpid())))
            self.worker_mode = mode
            # Mute may arrive while subprocess creation yields. Never leave a stale worker.
            if self.muted or not self.config["privacy"]["onboarded"] or (mode == "wake_word" and self.config["activation"]["mode"] != "wake_word"):
                await self._stop_worker_locked()
                return
            self.worker.stdin.write(encoded)
            await self.worker.stdin.drain()
            self.worker.stdin.close()
            self.voice_task = asyncio.create_task(self.read_voice(self.worker, mode))
            self.publish()

    async def read_voice(self, proc, mode):
        try:
            while line := await proc.stdout.readline():
                event = json.loads(line)
                if self.worker is not proc:
                    return
                kind = event["event"]
                if kind == "loading":
                    self.message = event["message"]
                elif kind == "ready":
                    self.unsupported = event.get("unsupported", [])
                    self.engine = event.get("engine")
                    self.activation.arm()
                    if event.get("listening"):
                        self.activation.listen(deadline=event.get("deadline"))
                    self.message = "Say a command" if event.get("listening") else ("Microphone test · ends in 15 seconds" if mode == "test" else "Waiting for “" + self.config["activation"]["wake_phrase"] + "”")
                elif kind == "wake":
                    self.activation.listen(deadline=event.get("deadline"))
                    self.message = "Listening · say one command"
                elif kind == "command":
                    accepted = self.activation.accept(event["text"], event.get("confidence"),
                                                      captured_at=event.get("captured_at"))
                    if accepted:
                        self.message = "Processing command…"
                        self.publish()
                        await self.execute(event["text"])
                        if event.get("closest_match") and event.get("heard"):
                            # Say what was actually heard, so a wrong guess is visible.
                            self.message = f"Heard “{event['heard']}” → {event['text']}. {self.message}"
                    else:
                        self.message = "No confident match. Try again or adjust sensitivity."
                    if not self.muted:
                        self.activation.arm() if mode == "wake_word" else self.activation.pause()
                elif kind == "heard":
                    self.message = f"Heard “{event.get('text', '')}”, which isn't a command I know. Try again."
                elif kind == "timeout":
                    self.message = "No command heard. Try again."
                    self.activation.arm() if mode == "wake_word" else self.activation.pause()
                elif kind == "level":
                    self.level = event["level"]
                    self.gate = event.get("gate")
                elif kind == "error":
                    self.activation.error()
                    self.message = event["message"]
                self.publish()
            await proc.wait()
            if self.worker is proc:
                self.worker = None
                self.level = 0
                if mode in ("test", "once") and not self.muted and self.activation.state != "error" and self.config["activation"]["mode"] == "wake_word":
                    self.background(self.start_worker("wake_word"), "voice restart")
                elif self.activation.state != "error":
                    if mode == "wake_word" and not self.muted:
                        self.activation.error()
                        self.message = "Voice worker stopped unexpectedly. Check the microphone and choose Resume."
                    else:
                        self.activation.pause()
                self.publish()
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            await self.stop_worker()
            self.activation.error()
            self.message = "Voice worker failed: " + str(exc)
            self.publish()

    async def set_mute(self, muted):
        if type(muted) is not bool:
            raise CommandError("Mute must be true or false")
        if not muted and not self.config["privacy"]["onboarded"]:
            raise CommandError("Complete first-run microphone consent in Settings")
        self.muted = muted
        if muted:
            # Mark closed before awaiting termination so no queued result can dispatch.
            self.activation.pause()
            await self.stop_worker()
            self.message = "Microphone muted · audio capture closed"
        elif self.config["activation"]["mode"] == "wake_word":
            await self.start_worker("wake_word")
        else:
            await self.stop_worker()
            self.activation.pause()
            self.message = "Push-to-talk ready · microphone closed"
        self.publish()

    async def execute(self, phrase):
        async with self.action_lock:
            try:
                intent = self.parser.parse(phrase)
                if os.environ.get("MURMUR_DRY_RUN") == "1":
                    self.message = "Dry run: " + intent.action
                else:
                    self.message = await asyncio.to_thread(self.dispatcher.execute, intent)
            except (CommandError, RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
                self.message = str(exc)
                self.publish()
                return {"ok": False, "error": str(exc)}
            self.publish()
            return {"ok": True, "message": self.message}

    async def handle(self, payload):
        op = payload.get("op")
        if op == "status":
            return {"ok": True, "status": self.status()}
        if op == "config":
            return {"ok": True, "config": self.config}
        if op == "apps":
            # Pick up apps installed since the last scan (the UI asks on open/Reconnect).
            if await asyncio.to_thread(self.apps.rescan) and self.worker and not self.muted \
                    and self.config["activation"]["mode"] == "wake_word":
                self.background(self.start_worker("wake_word"), "voice restart")
            entries = self.apps.list()
            if self.worker:
                excluded = set(self.unsupported)
                for app in entries:
                    app["speakable"] = any("open " + normalize(n) not in excluded for n in [app["name"], *app["aliases"]] if len(n) <= 256)
            return {"ok": True, "apps": entries}
        if op == "commands":
            return {"ok": True, "commands": REFERENCE}
        if op == "devices":
            return {"ok": True, "devices": await asyncio.to_thread(devices)}
        if op == "recording_devices":
            return {"ok": True, "devices": [{"id": d["name"], "name": d["description"], "monitor": d["monitor"]} for d in await asyncio.to_thread(audio_sources)]}
        if op == "monitors":
            return {"ok": True, "monitors": await asyncio.to_thread(self.recorder.sources)}
        if op == "mute":
            await self.set_mute(payload["muted"])
        elif op == "listen":
            await self.start_worker("once")
        elif op == "cancel":
            await self.stop_worker()
            self.activation.pause()
            if not self.muted and self.config["activation"]["mode"] == "wake_word":
                await self.start_worker("wake_word")
            else:
                self.message = "Cancelled · microphone closed"
        elif op == "mic_test":
            if payload.get("enabled"):
                await self.start_worker("test")
            elif self.worker is not None and self.worker_mode == "test":   # a finished test must not reload wake capture
                await self.stop_worker()
                if not self.muted and self.config["activation"]["mode"] == "wake_word":
                    await self.start_worker("wake_word")
                else:
                    self.activation.pause()
        elif op == "save_config":
            c = configuration.validate(payload["config"])
            Parser(c["commands"])
            if self.recorder.status()["state"] in ("recording", "stopping") and c["recording"] != self.config["recording"]:
                raise CommandError("Stop recording before changing capture settings")
            was_onboarded = self.config["privacy"]["onboarded"]
            self.config = await asyncio.to_thread(configuration.save, c)
            self.parser = Parser(c["commands"])
            self.apps = await asyncio.to_thread(AppRegistry, c["aliases"])
            self.dispatcher.apps = self.apps
            self.recorder.config = self.config
            a = c["activation"]
            self.activation.timeout, self.activation.confidence = a["command_timeout"], a["confidence"]
            if not c["privacy"]["onboarded"]:
                await self.set_mute(True)
            else:
                await self.set_mute(self.muted if was_onboarded else False)
            return {"ok": True, "config": self.config}
        elif op in ("record_start", "record_stop"):
            return await self.execute("start recording" if op == "record_start" else "stop recording")
        elif op == "execute":
            # Trusted local IPC/CLI can test deterministic parsing, never arbitrary shell.
            return await self.execute(payload["phrase"])
        elif op == "open_ui":
            self.open_ui("recording" if payload.get("page") == "recording" else "dashboard")
        elif op == "set_startup":
            enabled = payload.get("enabled")
            if type(enabled) is not bool:
                raise CommandError("Startup must be true or false")
            r = await asyncio.to_thread(subprocess.run, ["systemctl", "--user", "enable" if enabled else "disable", "murmur.service"], capture_output=True, text=True, timeout=5)
            if r.returncode:
                raise CommandError("Install the optional user service first (see README). " + r.stderr.strip()[:300])
            self.config["startup"] = enabled
            await asyncio.to_thread(configuration.save, self.config)
        else:
            raise CommandError("Unsupported operation")
        self.publish()
        return {"ok": True, "status": self.status()}


def devices():
    result = subprocess.run(["pw-dump"], capture_output=True, text=True, timeout=3)
    if result.returncode:
        raise CommandError("Cannot reach PipeWire; check the graphical session")
    nodes = json.loads(result.stdout)
    return [{"id": "", "name": "System default input"}] + [
        {"id": p["node.name"], "name": p.get("node.description", p["node.name"])}
        for n in nodes if (p := n.get("info", {}).get("props", {})).get("media.class") == "Audio/Source" and "node.name" in p]


async def serve():
    service = Service()
    service.loop = asyncio.get_running_loop()
    path = Path(socket_path())
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Do not use a world-writable pre-existing IPC directory.
    info = path.parent.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError("IPC directory must be owned by you with mode 0700")
    lock = (path.parent / "service.lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        raise RuntimeError("Murmur service is already running") from exc
    path.unlink(missing_ok=True)
    async def connection(reader, writer):
        queue = None
        try:
            peer = writer.get_extra_info("socket")
            _, uid, _ = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            if uid != os.getuid():
                return
            line = await asyncio.wait_for(reader.readline(), 5)
            if len(line) > 65536:
                raise ValueError("Request exceeds 64 KiB")
            payload = json.loads(line)
            if not isinstance(payload, dict):
                raise ValueError("Request must be an object")
            if payload.get("op") == "watch":
                queue = asyncio.Queue(maxsize=8)
                service.watchers.add(queue)
                queue.put_nowait({"ok": True, "status": service.status()})
                # Detect disconnect while status is unchanged, without timer polling.
                eof = asyncio.create_task(reader.read(1))
                try:
                    while not eof.done():
                        event = asyncio.create_task(queue.get())
                        done, _ = await asyncio.wait([eof, event], return_when=asyncio.FIRST_COMPLETED)
                        if eof in done:
                            event.cancel()
                            await asyncio.gather(event, return_exceptions=True)
                            break
                        writer.write((json.dumps(event.result()) + "\n").encode())
                        await asyncio.wait_for(writer.drain(), 2)
                finally:
                    eof.cancel()
            else:
                result = await service.handle(payload)
                writer.write((json.dumps(result) + "\n").encode())
                await writer.drain()
        except (ValueError, KeyError, RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
            try:
                writer.write((json.dumps({"ok": False, "error": str(exc)}) + "\n").encode())
                await writer.drain()
            except (OSError, ConnectionError):
                pass
        finally:
            if queue:
                service.watchers.discard(queue)
            writer.close()
            await writer.wait_closed()
    server = await asyncio.start_unix_server(connection, str(path), limit=65536)
    path.chmod(0o600)
    for sig in (signal.SIGINT, signal.SIGTERM):
        service.loop.add_signal_handler(sig, service.stopping.set)
    try:
        try:
            await asyncio.to_thread(service.recorder.inspect_previous)
        except OSError as exc:
            print(f"murmur: could not inspect earlier recordings: {exc}", file=sys.stderr)
        if not service.muted:
            # A voice failure must not take the whole service down (and into a
            # systemd restart loop); show it and wait for the user instead.
            try:
                await service.set_mute(False)
            except (CommandError, RuntimeError, OSError) as exc:
                service.activation.error()
                service.message = f"Voice control could not start: {exc}"
        async with server:
            await service.stopping.wait()
    finally:
        await service.set_mute(True)
        try:
            await asyncio.to_thread(service.recorder.close)
        finally:
            path.unlink(missing_ok=True)
            lock.close()


def main():
    asyncio.run(serve())
