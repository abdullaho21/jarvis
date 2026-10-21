"""Allocator selection and stream lifetime; real measurements use the benchmark."""
import ctypes
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import MagicMock, patch

from murmur import transcriber


class TranscriberMemoryTests(unittest.TestCase):
    def test_cpu_config_is_passed_without_changing_model_or_threads(self):
        recognizer = MagicMock()
        module = types.SimpleNamespace(OfflineRecognizer=types.SimpleNamespace(from_whisper=recognizer))
        with patch.dict(sys.modules, {"sherpa_onnx": module}), \
                patch.object(transcriber, "accurate_model_ready", return_value=True), \
                patch.object(transcriber, "accurate_model_dir", return_value=Path("/model")), \
                patch.object(transcriber, "release_unused_memory") as release:
            transcriber.Transcriber(threads=2)
            args, kwargs = recognizer.call_args
            self.assertEqual(kwargs["num_threads"], 2)
            self.assertEqual(kwargs["language"], "en")
            provider, config = kwargs["provider"].split(":", 1)
            self.assertEqual(provider, "cpu")
            self.assertIn("EnableCpuMemArena=0", Path(config).read_text())
            self.assertTrue(all(path.startswith("/model/") for path in args))
            release.assert_called_once()

    def test_stream_is_destroyed_before_allocator_trim(self):
        events = []
        class Stream:
            result = types.SimpleNamespace(text=" Open Steam. ")
            def accept_waveform(self, rate, samples):
                self.rate, self.samples = rate, samples
            def __del__(self):
                events.append("destroy")
        recognizer = types.SimpleNamespace(create_stream=Stream, decode_stream=lambda stream: events.append("decode"))
        engine = transcriber.Transcriber.__new__(transcriber.Transcriber)
        engine._recognizer, engine._denoiser, engine._memory_efficient = recognizer, None, True
        with patch.object(transcriber, "release_unused_memory", side_effect=lambda: events.append("trim")):
            self.assertEqual(engine.transcribe(bytes(16000)), "Open Steam.")
        self.assertEqual(events, ["decode", "destroy", "trim"])

    def test_non_glibc_allocator_is_supported(self):
        with patch.object(ctypes, "CDLL", return_value=object()):
            transcriber.release_unused_memory()


if __name__ == "__main__":
    unittest.main()
