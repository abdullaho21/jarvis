import math
import unittest

from scripts.benchmark_memory_soak import analyze, verdict


def run(slope=0):
    # One-hour warm-up growth followed by a controllable 23-hour floor.
    return {"commands_identical": True, "elapsed_seconds": 86400,
            "commands": [{"text": "ten meters", "wake_elapsed_seconds": seconds,
                          "rss_at_wake_mib": min(seconds / 3600, 1) * 100 + max(0, seconds / 3600 - 1) * slope}
                         for seconds in range(0, 86401, 300)]}


class SoakGateTests(unittest.TestCase):
    def test_warmup_growth_then_plateau_passes_only_after_three_runs(self):
        result = analyze(run())
        self.assertEqual(result["status"], "stable")
        self.assertEqual(verdict([result] * 2), "insufficient")
        self.assertEqual(verdict([result] * 3), "pass")

    def test_persistent_growth_exceeding_tolerance_fails(self):
        result = analyze(run(1.1))
        self.assertEqual(result["status"], "growth")
        self.assertEqual(verdict([result] * 3), "fail")

    def test_mixed_windows_or_repeated_runs_do_not_pass(self):
        sample = run()
        for row in sample["commands"]:
            if row["wake_elapsed_seconds"] > 16 * 3600:
                row["rss_at_wake_mib"] += (row["wake_elapsed_seconds"] / 3600 - 16) * 10
        self.assertEqual(analyze(sample)["status"], "inconclusive")
        self.assertEqual(verdict([analyze(run()), analyze(run()), analyze(run(2))]), "inconclusive")

    def test_short_missing_bad_recognition_and_nonfinite_samples_never_pass(self):
        sample = run()
        sample["elapsed_seconds"] = 3600
        self.assertEqual(analyze(sample)["status"], "insufficient")
        sample = run()
        sample["commands"][5]["text"] = "stop recording"
        self.assertEqual(analyze(sample)["status"], "invalid")
        sample = run()
        sample["commands"][5]["wake_elapsed_seconds"] = math.nan
        self.assertEqual(analyze(sample)["status"], "invalid")
        sample = run()
        del sample["commands"][5]["wake_elapsed_seconds"]
        self.assertEqual(analyze(sample)["status"], "invalid")
        sample = run()
        sample["elapsed_seconds"] = math.nan
        self.assertEqual(analyze(sample)["status"], "invalid")


if __name__ == "__main__":
    unittest.main()
