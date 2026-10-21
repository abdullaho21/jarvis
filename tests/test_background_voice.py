"""Capture deadlines and worker delivery; no microphone, GUI or app launch."""
import asyncio
import copy
import io
import json
import unittest
from unittest.mock import AsyncMock, MagicMock, patch

from murmur.config import DEFAULTS
from murmur.daemon import Service
from murmur.speech import Recognition
from murmur.state import Activation
from murmur.voice import run


class CaptureDeadlineTests(unittest.TestCase):
    def setUp(self):
        self.now = 100.0
        self.activation = Activation(clock=lambda: self.now)
        self.activation.listen(deadline=108.0)

    def test_deadline_flush_can_finish_decoding_after_capture_ends(self):
        self.now = 109.0
        self.assertTrue(self.activation.accept("open steam", 0.9, captured_at=108.0))
        self.assertEqual(self.activation.state, "processing")
        self.assertFalse(self.activation.accept("open steam", 0.9, captured_at=108.0))

    def test_late_unstamped_result_still_fails(self):
        self.now = 109.0
        self.assertFalse(self.activation.accept("open steam", 0.9))

    def test_invalid_capture_times_and_stale_results_do_not_run(self):
        self.now = 109.0
        for timestamp in (99, 108.01, 110, float("nan"), float("inf"), True, "108"):
            with self.subTest(timestamp=timestamp):
                self.assertFalse(self.activation.accept("open steam", 0.9, captured_at=timestamp))
        self.now = 108 + Activation.MAX_RESULT_DELAY + 0.01
        self.assertFalse(self.activation.accept("open steam", 0.9, captured_at=108))

    def test_cancel_and_confidence_checks_still_apply_to_captured_results(self):
        self.now = 109.0
        self.assertFalse(self.activation.accept("open steam", 0.1, captured_at=108))
        self.activation.pause()
        self.assertFalse(self.activation.accept("open steam", 0.9, captured_at=108))


class WorkerDeadlineTests(unittest.TestCase):
    def run_worker(self, engine, clock):
        config = copy.deepcopy(DEFAULTS)
        config["privacy"]["onboarded"] = True
        selector = MagicMock()
        def frame(_timeout):
            clock[0] += 1.0
            return [(None, None)]
        selector.select.side_effect = frame
        capture = MagicMock()
        capture.stderr = io.BytesIO()
        with patch("murmur.voice._parent_death"), patch("murmur.voice.signal.signal"), \
                patch("murmur.voice.make_engine", return_value=engine), \
                patch("murmur.voice.subprocess.Popen", return_value=capture), \
                patch("murmur.voice.selectors.DefaultSelector", return_value=selector), \
                patch("murmur.voice.os.read", return_value=bytes(960)), \
                patch("murmur.voice.time.monotonic", side_effect=lambda: clock[0]), \
                patch("murmur.voice.emit") as emit:
            run(config, "once", [])
        messages = [call.kwargs for call in emit.call_args_list]
        self.assertFalse(any(message.get("event") == "error" for message in messages))
        return messages

    def test_worker_stamps_deadline_flush_before_slow_decode(self):
        clock = [0.0]
        engine = MagicMock(frame_bytes=960, unsupported_phrases=[], name="test", wants_raw_audio=True)
        engine.feed.return_value = None
        def flush():
            clock[0] += 1.0
            return Recognition("command", "open steam", 0.9)
        engine.flush.side_effect = flush
        messages = self.run_worker(engine, clock)
        commands = [message for message in messages if message.get("event") == "command"]
        self.assertEqual(len(commands), 1)
        self.assertEqual(commands[0]["captured_at"], DEFAULTS["activation"]["command_timeout"])
        self.assertGreater(clock[0], commands[0]["captured_at"])

    def test_slow_decode_is_not_reported_as_a_stalled_microphone(self):
        clock, calls = [0.0], []
        engine = MagicMock(frame_bytes=960, unsupported_phrases=[], name="test", wants_raw_audio=True)
        def feed(*args, **kwargs):
            calls.append(True)
            clock[0] += 4.0
            return Recognition("command", "open steam", 0.9) if len(calls) == 2 else None
        engine.feed.side_effect = feed
        messages = self.run_worker(engine, clock)
        self.assertEqual(len(calls), 2)
        self.assertTrue(any(message.get("event") == "command" for message in messages))


class BackgroundDeliveryTests(unittest.IsolatedAsyncioTestCase):
    async def test_service_dispatches_bounded_late_result_without_a_ui_watcher(self):
        with patch("murmur.daemon.AppRegistry", return_value=MagicMock(index={})):
            service = Service({"privacy": {"onboarded": True}})
        service.activation.clock = lambda: 109.0
        worker = MagicMock()
        worker.stdout = asyncio.StreamReader()
        worker.wait = AsyncMock(return_value=0)
        service.worker = worker
        for event in ({"event": "wake", "deadline": 108.0},
                      {"event": "command", "text": "open steam", "confidence": 0.9, "captured_at": 108.0}):
            worker.stdout.feed_data((json.dumps(event) + "\n").encode())
        worker.stdout.feed_eof()
        self.assertFalse(service.watchers)
        with patch.object(service, "execute", new_callable=AsyncMock) as execute:
            await service.read_voice(worker, "wake_word")
        execute.assert_awaited_once_with("open steam")


if __name__ == "__main__":
    unittest.main()
