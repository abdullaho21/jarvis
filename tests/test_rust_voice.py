"""Rust/Python parity and owned capture lifetimes; no real microphone or actions."""
import copy
import asyncio
import json
import os
from pathlib import Path
import random
import selectors
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import AsyncMock, MagicMock, patch

from murmur.commands import Parser, POLITE_PREFIXES, POLITE_SUFFIXES, closest_match, match_candidates
from murmur.config import DEFAULTS
from murmur.native_worker import binary_path, prepare

APPS = ["firefox", "dolphin", "discord", "deadlock", "steam", "stremio", "streamio", "tekken eight", "calculator"]


def native(config=None, mode="once", grammar=None):
    with patch.dict(os.environ, MURMUR_VOICE_WORKER="rust"):
        return prepare(config or copy.deepcopy(DEFAULTS), mode, grammar or Parser().grammar(APPS))


def event(process, timeout=10):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise AssertionError("Worker did not respond")
    line = process.stdout.readline()
    if not line:
        raise AssertionError(f"Worker exited: {process.poll()}")
    return json.loads(line)


class PreparationTests(unittest.TestCase):
    def test_python_remains_available_without_rust_or_model_imports(self):
        with patch.dict(os.environ, MURMUR_VOICE_WORKER="python"), patch("murmur.native_worker.binary_path", side_effect=AssertionError):
            argv, payload, env = prepare(copy.deepcopy(DEFAULTS), "test", {})
        self.assertEqual(argv, [sys.executable, "-m", "murmur.voice"])
        self.assertNotIn("native", payload)
        self.assertEqual(env["MURMUR_OWNER_PID"], str(os.getpid()))

    def test_explicit_rust_requires_a_built_worker_and_rejects_unknown_selection(self):
        for selection, error in (("rust", "missing"), ("typo", "must be")):
            with patch.dict(os.environ, MURMUR_VOICE_WORKER=selection), patch("murmur.native_worker.binary_path", return_value=None):
                with self.assertRaisesRegex(ValueError, error):
                    prepare(copy.deepcopy(DEFAULTS), "test", {})

    def test_rust_whisper_preparation_does_not_require_sherpa(self):
        config = copy.deepcopy(DEFAULTS)
        config["recognition"]["accurate"] = True
        with patch.dict(os.environ, MURMUR_VOICE_WORKER="rust"), \
                patch("murmur.native_worker.binary_path", return_value=Path("/built/jarvis-voice")), \
                patch("murmur.native_worker.package_dir", return_value=None) as packages, \
                patch("murmur.native_worker.accurate_model_ready", return_value=True):
            _, payload, _ = prepare(config, "test", Parser().grammar([]))
        self.assertIsNotNone(payload["native"]["whisper"])
        self.assertEqual(payload["native"]["whisper_idle_seconds"], 60)
        self.assertEqual([c.args[0] for c in packages.call_args_list], ["vosk"])


@unittest.skipUnless(binary_path(), "build the optional Rust worker")
class RustPolicyTests(unittest.TestCase):
    def test_matches_python_across_commands_typos_ambiguities_and_unicode(self):
        config = copy.deepcopy(DEFAULTS)
        # Preparation in meter-test mode does not require a Vosk model.
        argv, _payload, env = native(config, "test")
        grammar = Parser().grammar(APPS)
        candidates = match_candidates(grammar)
        corpus = ["open fire fox", "open stream io", "go to workspace tree", "stat recording", "open taken eight",
                  "switch to discourt", "open team", "could you open fire fox please", "recording", "open", "", "hey",
                  "start a cording", "Go to workspace 3.", "uh open firefox and the weather tomorrow", "OPEN FIREFOX!",
                  "please open ﬁrefox", "start recording " + "and the weather tomorrow will be cloudy " * 7,
                  "i want to eat pizza", "what time is it", "can sell", "the weather tomorrow will be mostly cloudy"]
        rng = random.Random(20261022)
        for text in rng.sample(list(candidates), 90):
            corpus.extend((text, "could you " + text + " please", text[:3] + text[4:], "uh " + text + " tomorrow"))
        requests = [{"heard": text, "candidates": list(candidates.items()), "prefixes": list(POLITE_PREFIXES), "suffixes": list(POLITE_SUFFIXES)} for text in corpus]
        out = subprocess.run(argv + ["--match"], input="".join(json.dumps(r) + "\n" for r in requests), env=env, capture_output=True, text=True, timeout=60, check=True)
        actual = [json.loads(line)["match"] for line in out.stdout.splitlines()]
        self.assertEqual(len(actual), len(corpus))
        for text, result in zip(corpus, actual):
            expected = closest_match(text, candidates)
            self.assertEqual(result, list(expected) if expected else None, text)

    def test_no_consent_cannot_spawn_capture(self):
        argv, payload, env = native(mode="test")
        env["PATH"] = "/not-a-real-directory"
        out = subprocess.run(argv, input=json.dumps(payload) + "\n", env=env, text=True, capture_output=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("consent", json.loads(out.stdout)["message"])

    def test_payload_bound_is_enforced_before_model_or_capture_load(self):
        argv, _payload, env = native(mode="test")
        out = subprocess.run(argv, input=b"x" * 1048577, env=env, capture_output=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("size limit", json.loads(out.stdout)["message"])

    def test_invalid_whisper_idle_duration_is_rejected_before_loading(self):
        argv, payload, env = native(mode="test")
        for value in (0, 0.5, -60, 86401):
            with self.subTest(value=value):
                payload["native"]["whisper_idle_seconds"] = value
                out = subprocess.run(argv, input=json.dumps(payload) + "\n", env=env, text=True,
                                     capture_output=True, timeout=10)
                self.assertNotEqual(out.returncode, 0)
                self.assertIn("Whisper idle duration", json.loads(out.stdout)["message"])


FIXTURE = os.environ.get("MURMUR_SPEECH_TEST_RAW")
MODEL = os.environ.get("MURMUR_VOSK_MODEL")
WHISPER = os.environ.get("MURMUR_WHISPER_MODEL")


def whisper_threads(pid):
    """Whisper's loader and inference pool threads exist only while it is loaded."""
    names = []
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            names.append((task / "comm").read_text().strip())
        except OSError:
            pass
    return sum(name.startswith("whisper-") for name in names)


def whisper_worker(idle, pcm):
    """Worker with real Vosk/Whisper, wake name "go forward" and a paced fake microphone."""
    config = copy.deepcopy(DEFAULTS)
    config["privacy"]["onboarded"] = True
    config["recognition"].update(model_path=MODEL, accurate=True)
    config["activation"]["wake_phrase"] = "go forward"
    argv, payload, env = native(config, "wake_word", Parser({"ten meters": "start recording"}).grammar(APPS))
    root = Path(WHISPER)
    payload["native"]["whisper"] = {key: str(root / f"small.en-{name}") for key, name in
                                    (("encoder", "encoder.int8.onnx"), ("decoder", "decoder.int8.onnx"), ("tokens", "tokens.txt"))}
    if idle is None:
        del payload["native"]["whisper_idle_seconds"]
    else:
        payload["native"]["whisper_idle_seconds"] = idle
    directory = tempfile.TemporaryDirectory()
    fake = Path(directory.name) / "pw-record"
    fake.write_text("#!/usr/bin/env python3\nimport os,time\nfrom pathlib import Path\naudio=bytes(19200)+Path(os.environ['PUBLIC_PCM']).read_bytes()\naudio+=bytes((-len(audio))%960)\nfor offset in range(0,len(audio),960):\n os.write(1,audio[offset:offset+960]);time.sleep(.03)\nwhile True: os.write(1,bytes(960));time.sleep(.03)\n")
    fake.chmod(0o700)
    env.update(PATH=directory.name + ":" + env.get("PATH", ""), PUBLIC_PCM=pcm)
    process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, bufsize=0)
    process.stdin.write((json.dumps(payload) + "\n").encode()); process.stdin.flush()
    while event(process, 30)["event"] != "ready":
        pass
    return process, directory


@unittest.skipUnless(binary_path() and MODEL and FIXTURE, "build Rust and supply the public fixture and Vosk model")
class RustEngineTests(unittest.TestCase):
    def test_service_selects_rust_and_accepts_its_command_without_dispatching_actions(self):
        from murmur.daemon import Service
        async def check():
            config = copy.deepcopy(DEFAULTS)
            config["privacy"]["onboarded"] = True
            config["activation"]["mode"] = "push_to_talk"
            config["recognition"].update(model_path=MODEL, accurate=False)
            config["commands"] = {"go forward ten meters": "start recording"}
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                capture = root / "pw-record"
                capture.write_text("#!/usr/bin/env python3\nimport os,time\nfrom pathlib import Path\naudio=Path(os.environ['PUBLIC_PCM']).read_bytes()+bytes(64000)\naudio+=bytes((-len(audio))%960)\nfor offset in range(0,len(audio),960):\n os.write(1,audio[offset:offset+960]);time.sleep(.03)\nwhile True: os.write(1,bytes(960));time.sleep(.03)\n")
                capture.chmod(0o700)
                with patch.dict(os.environ, MURMUR_VOICE_WORKER="rust", PATH=str(root) + ":" + os.environ.get("PATH", ""), PUBLIC_PCM=FIXTURE), \
                        patch("murmur.daemon.AppRegistry", return_value=MagicMock(index={})):
                    service = Service(config)
                    service.loop = asyncio.get_running_loop()
                    service.execute = AsyncMock()
                    try:
                        await service.start_worker("once")
                        self.assertEqual(Path(f"/proc/{service.worker.pid}/exe").resolve().name, "jarvis-voice")
                        await asyncio.wait_for(service.voice_task, 12)
                        service.execute.assert_awaited_once_with("go forward ten meters")
                        self.assertEqual(service.engine, "vosk")
                        self.assertIsNone(service.worker)
                    finally:
                        await service.stop_worker()
        asyncio.run(check())

    def test_real_vosk_commands_and_one_breath_wake_match_python(self):
        from murmur.vosk_engine import VoskEngine
        from murmur.voice import rms
        grammar = Parser({"go forward ten meters": "start recording", "ten meters": "start recording"}).grammar(APPS)
        config = copy.deepcopy(DEFAULTS)
        config["recognition"].update(model_path=MODEL, accurate=False)
        config["activation"]["wake_phrase"] = "go forward"
        argv, payload, env = native(config, grammar=grammar)
        audio = Path(FIXTURE).read_bytes()
        audio += bytes(((len(audio) + 64000 + 959) // 960) * 960 - len(audio))
        expected = []
        engine = VoskEngine(grammar, "go forward", model_path=MODEL)
        for session in ("command", "wake", "wake", "command"):
            engine.begin_command() if session == "command" else engine.begin_wake()
            results = []
            for offset in range(0, len(audio), 960):
                frame = audio[offset:offset + 960]
                result = engine.feed(frame, quiet=rms(frame) < .005)
                if result:
                    results.append(result.to_dict())
                    if result.kind == "wake" and (follow := engine.after_wake()):
                        results.append(follow.to_dict())
                    break
            expected.append(results)
        request = json.dumps(payload) + "\n" + "".join(json.dumps({"path": FIXTURE, "session": s}) + "\n" for s in ("command", "wake", "wake", "command"))
        out = subprocess.run(argv + ["--engine"], input=request, env=env, capture_output=True, text=True, timeout=30, check=True)
        actual = [json.loads(line) for line in out.stdout.splitlines()]
        self.assertEqual(actual[0]["event"], "ready")
        self.assertEqual([e["events"] for e in actual[1:]], expected)
        self.assertTrue(all(expected))

    def test_control_priority_and_capture_cleanup_with_a_fake_microphone(self):
        config = copy.deepcopy(DEFAULTS)
        config["privacy"]["onboarded"] = True
        config["recognition"].update(model_path=MODEL, accurate=False)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "pw-record"
            fake.write_text("#!/usr/bin/env python3\nimport os\nfrom pathlib import Path\nPath(os.environ['CAPTURE_PID']).write_text(str(os.getpid()))\nwhile True: os.write(1, bytes(960))\n")
            fake.chmod(0o700)
            argv, payload, env = native(config, "wake_word")
            env.update(PATH=str(root) + ":" + env.get("PATH", ""), CAPTURE_PID=str(root / "capture.pid"))
            process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, bufsize=0)
            try:
                process.stdin.write((json.dumps(payload) + "\n").encode()); process.stdin.flush()
                while event(process)["event"] != "ready":
                    pass
                deadline = time.monotonic() + 3
                while not (root / "capture.pid").exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue((root / "capture.pid").exists())
                process.stdin.write(b"listen\n"); process.stdin.flush()
                self.assertTrue(event(process)["listening"])
                process.stdin.write(b"cancel\n"); process.stdin.flush()
                self.assertFalse(event(process)["listening"])
                capture_pid = int((root / "capture.pid").read_text())
                process.terminate(); self.assertEqual(process.wait(timeout=5), 0)
                self.assertFalse(Path(f"/proc/{capture_pid}").exists(), "Capture outlived the Rust worker")
            finally:
                if process.poll() is None:
                    process.kill(); process.wait()
                process.stdin.close(); process.stdout.close()

    def close(self, process, directory):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
        process.stdin.close(); process.stdout.close()
        directory.cleanup()

    @unittest.skipUnless(WHISPER, "supply the Whisper model")
    def test_whisper_loads_on_wake_and_is_released_after_idle(self):
        process, directory = whisper_worker(1, FIXTURE)
        try:
            self.assertEqual(whisper_threads(process.pid), 0, "Whisper loaded before a wake name")
            events = [event(process, 15)]
            while events[-1]["event"] != "command":
                events.append(event(process, 15))
            self.assertEqual([e["event"] for e in events if e["event"] in ("wake", "command")], ["wake", "command"])
            self.assertEqual(events[-1]["text"], "ten meters")
            self.assertGreater(whisper_threads(process.pid), 0)
            deadline = time.monotonic() + 6
            while whisper_threads(process.pid) and time.monotonic() < deadline:
                time.sleep(.05)
            self.assertEqual(whisper_threads(process.pid), 0, "Whisper was not released after going idle")
            self.assertIsNone(process.poll())
        finally:
            self.close(process, directory)

    @unittest.skipUnless(WHISPER, "supply the Whisper model")
    def test_payload_without_idle_limit_keeps_whisper_resident(self):
        process, directory = whisper_worker(None, FIXTURE)
        try:
            self.assertGreater(whisper_threads(process.pid), 0, "Whisper was not loaded at startup")
        finally:
            self.close(process, directory)


if __name__ == "__main__":
    unittest.main()
