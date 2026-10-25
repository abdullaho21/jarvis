"""Policy equivalence and lifecycle regressions; no live desktop or microphone."""
import io
import json
import threading
import unittest
from unittest.mock import MagicMock, patch

from murmur import client, config
from murmur.apps import App
from murmur.commands import CommandError, Intent, Parser
from murmur.hyprland import Hyprland
from murmur.phrases import number
from murmur.recorder import RecorderError
from murmur.replay import Replay


class ExactBrightnessTests(unittest.TestCase):
    def test_every_percentage_and_wording_resolves_to_the_exact_monitor(self):
        parser = Parser.from_config(config.validate({"brightness": {
            "monitors": {"screen-1": {"name": "screen one", "aliases": ["primary"]},
                         "screen-2": {"name": "screen", "aliases": []}},
            "default": "screen-1"}}))
        for name, identity in [("screen one", "screen-1"), ("primary", "screen-1"),
                               ("main", "screen-1"), ("screen", "screen-2")]:
            for n in range(101):
                for value in (str(n), number(n)):
                    for form in (f"set {name} brightness to {value}",
                                 f"brighten the {name} to {value}", f"dim {name} to {value}"):
                        for text in (form, "could you " + form + " percent please"):
                            with self.subTest(text=text):
                                self.assertEqual(parser.parse(text), Intent("brightness_set", {
                                    "monitor": identity, "percent": n}))
        self.assertEqual(parser.parse("set brightness to 100").target["percent"], 100)
        self.assertEqual(parser.parse("dim"), Intent("brightness_down", {"monitor": "screen-1"}))

    def test_partial_numbers_and_extra_words_never_become_percentages(self):
        parser = Parser.from_config(config.validate({"brightness": {
            "monitors": {"screen-1": {"name": "screen", "aliases": []}}}}))
        for value in ("", "one hundred one", "twenty zero", "two one", "zero hundred",
                      "twenty one one", "one hundred zero", "sixty seven percent percent",
                      "101", "-1", "+67", "6.7", "67 tomorrow"):
            with self.subTest(value=value), self.assertRaises(CommandError):
                parser.parse("set screen brightness to " + value)
        for phrase in ("dim", "brighten", "set brightness to 67", "dim unknown", "dim the"):
            with self.subTest(phrase=phrase), self.assertRaises(CommandError):
                parser.parse(phrase)


class SettingsOwnershipTests(unittest.TestCase):
    def test_nested_patches_preserve_siblings_and_do_not_mutate_defaults(self):
        base = {"nested": {"old": [1], "changed": {"a": 1}}}
        update = {"nested": {"new": [2], "changed": {"b": [3]}}, "extra": {"list": []}}
        merged = config.merged(base, update)
        self.assertEqual(merged, {"nested": {"old": [1], "changed": {"a": 1, "b": [3]},
                                            "new": [2]}, "extra": {"list": []}})
        merged["nested"]["old"].append(4)
        self.assertEqual(base, {"nested": {"old": [1], "changed": {"a": 1}}})
        self.assertEqual(update, {"nested": {"new": [2], "changed": {"b": [3]}}, "extra": {"list": []}})


class WindowMatchingTests(unittest.TestCase):
    def test_current_and_initial_classes_preserve_focus_order_and_unicode_folding(self):
        app = App("strasse.desktop", "Straße", "/fixture/strasse.desktop", "strasse")
        windows = [{"class": "other", "initialClass": "STRASSE", "focusHistoryID": 2},
                   {"class": "Straße", "focusHistoryID": 1},
                   {"class": "unrelated", "focusHistoryID": 0}]
        desktop = Hyprland()
        with patch.object(desktop, "query", return_value=windows):
            self.assertEqual(desktop.running(app), [windows[1], windows[0]])


class ResponseValidationTests(unittest.TestCase):
    def test_non_object_service_responses_are_connection_errors(self):
        for value in ([], None, "status", 1):
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value.__enter__.return_value.readline.return_value = json.dumps(value).encode() + b"\n"
            with self.subTest(value=value), patch.object(client.socket, "socket", return_value=connection):
                with self.assertRaises(ConnectionError):
                    client.request("status")

    def test_non_object_replay_responses_are_actionable_errors(self):
        connection = MagicMock()
        connection.__enter__.return_value = connection
        connection.makefile.return_value.__enter__.return_value.readline.return_value = b"[]\n"
        with patch("murmur.replay.socket.socket", return_value=connection):
            with self.assertRaisesRegex(RecorderError, "invalid response"):
                Replay(config.validate({}))._request("save-replay")

    def test_replay_stop_handles_a_child_that_exits_before_signalling(self):
        replay = Replay(config.validate({}))
        process = MagicMock(stderr=io.BytesIO(), **{"wait.return_value": 0})
        replay._process = process
        replay._status["state"] = "on"
        replay._done.clear()
        watcher = threading.Thread(target=replay._watch, args=(process,))
        def exit_before_signal(_signal):
            watcher.start()
            raise ProcessLookupError
        process.send_signal.side_effect = exit_before_signal
        try:
            self.assertEqual(replay.stop()["state"], "off")
        finally:
            watcher.join(timeout=2)
        self.assertFalse(watcher.is_alive())
        self.assertTrue(process.stderr.closed)


if __name__ == "__main__":
    unittest.main()
