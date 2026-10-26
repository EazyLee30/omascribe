import copy
import unittest

from benchmark_recording_contention import paired_errors, telemetry_summary


class RecordingContentionTests(unittest.TestCase):
    def rows(self):
        return [dict(schema=1, wall_ms=t+100, captured_ms=t, processed_ms=max(0, t-100) if t < 2000 else t,
            has_error=False, recording=t < 2000, transcribing=t < 2000, asr_rtf=.1,
            translations=1 if t >= 1000 else 0, notes_cursor=1 if t >= 1500 else 0,
            worker=dict(leased=500 <= t < 1500, starts=1, reuses=0)) for t in range(0, 2001, 500)]

    def test_live_work_and_complete_paced_capture_are_required(self):
        rows = self.rows()
        result = telemetry_summary(rows, 2, True)
        self.assertEqual(result["lag_max_ms"], 100)
        self.assertEqual(result["live_worker_samples"], 2)
        settled = rows + [dict(rows[-1], wall_ms=3100)]
        self.assertEqual(telemetry_summary(settled, 2, True)["capture_wall_overhead_ms"], 100)
        for change in [lambda r: r[-1].update(processed_ms=1900),
                       lambda r: r[-1].update(recording=True),
                       lambda r: r[2].update(has_error=True),
                       lambda r: r[2].update(processed_ms=1001),
                       lambda r: r[3].update(wall_ms=100),
                       lambda r: r[3].update(captured_ms=1),
                       lambda r: r[3].update(translations=0, notes_cursor=0)]:
            bad = copy.deepcopy(rows)
            change(bad)
            with self.assertRaises(ValueError): telemetry_summary(bad, 2, True)

    def test_missing_llm_load_is_not_a_successful_contention_measurement(self):
        rows = self.rows()
        for row in rows: row["worker"]["leased"] = False
        telemetry_summary(rows, 2, False)
        with self.assertRaises(ValueError): telemetry_summary(rows, 2, True)
        rows = self.rows()
        for row in rows: row["wall_ms"] *= 4
        with self.assertRaises(ValueError): telemetry_summary(rows, 2, True)

    def test_failed_costs_pcm_and_output_must_stay_in_comparison(self):
        baseline = dict(arm="asr-only", passed=True, pcm_sha256="a", audio_samples=32000, hypothesis_units=["hello"])
        loaded = dict(baseline, arm="metal-concurrent")
        self.assertEqual(paired_errors([baseline, loaded]), [])
        for change in [dict(passed=False), dict(pcm_sha256="other"), dict(audio_samples=None), dict(hypothesis_units=["different"])]:
            self.assertTrue(paired_errors([baseline, dict(loaded, **change)]))
        self.assertTrue(paired_errors([baseline]))
        arms = ["legacy-concurrent", "catchup-concurrent"]
        pair = [dict(baseline, arm=name) for name in arms]
        self.assertEqual(paired_errors(pair, arms), [])
        self.assertTrue(paired_errors(pair))


if __name__ == "__main__": unittest.main()
