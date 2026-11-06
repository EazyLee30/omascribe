"""Gate failure-path tests. Only tiny Python stand-ins run; no model/SDK/Cargo."""
import copy
import json
import os
from pathlib import Path
import queue
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

import evaluate_hotword_dynamic_safety as gate
from harness_support import read_json, write_json


def config(mode="stream", rejected=False):
    return dict(hotword_update_mode="stream_experiment" if mode == "stream" else "recognizer",
        threads=1, hotword_count=2, decoder="modified_beam_search",
        default_hotword_buffer_present=mode == "legacy", model_load_attempts=1, stream_updates=0,
        fallback="previous context retained" if rejected else None,
        hotword_input=dict(status="rejected" if rejected else "accepted", previous_context_retained=rejected,
                          error={"code": "invalid_words"} if rejected else None))


class DeadlineAndEvidenceTests(unittest.TestCase):
    def test_global_reserve_prevents_another_launch_and_caps_long_arm(self):
        with patch.object(gate.time, "monotonic", side_effect=[100., 101., 269., 270.]):
            budget = gate.Budget(seconds=180, reserve=10)
            self.assertEqual(budget.remaining(500), 169.)
            self.assertEqual(budget.remaining(), 1.)
            with self.assertRaises(TimeoutError):
                budget.remaining()

    def test_inherited_modes_models_and_thread_overrides_do_not_change_trial(self):
        with patch.dict(os.environ, {"OMASCRIBE_ASR_STREAM_HOTWORDS": "1", "OMASCRIBE_MODEL": "private",
                                   "LLAMA_ARG_MODEL": "private", "OMP_NUM_THREADS": "99"}):
            for mode, flag in [("legacy", "0"), ("stream", "1")]:
                env = gate.environment(mode)
                self.assertEqual(env["OMASCRIBE_ASR_STREAM_HOTWORDS"], flag)
                self.assertEqual(env["OMP_NUM_THREADS"], "1")
                self.assertNotIn("OMASCRIBE_MODEL", env)
                self.assertNotIn("LLAMA_ARG_MODEL", env)

    def test_rejected_diagnostic_cannot_pass_without_actual_retained_context(self):
        expected = config(rejected=True)
        gate.check_config(expected, "stream", 2, 1, 0, "invalid_words")
        for field, value in [("hotword_count", 0), ("model_load_attempts", 2),
                             ("stream_updates", 1), ("default_hotword_buffer_present", True)]:
            with self.subTest(field=field):
                broken = copy.deepcopy(expected)
                broken[field] = value
                with self.assertRaises(RuntimeError):
                    gate.check_config(broken, "stream", 2, 1, 0, "invalid_words")
        broken = copy.deepcopy(expected)
        broken["hotword_input"]["previous_context_retained"] = False
        with self.assertRaises(RuntimeError):
            gate.check_config(broken, "stream", 2, 1, 0, "invalid_words")

    def test_recovery_cannot_pass_with_a_stale_rejection(self):
        with self.assertRaises(RuntimeError):
            gate.check_config(config(rejected=True), "stream", 2, 1, 0)
        gate.check_config(config(), "stream", 2, 1, 0)

    def test_first_protocol_failure_stops_before_other_mode_or_protocol(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(gate, "lifecycle", side_effect=RuntimeError("witness")) as first, \
                patch.object(gate, "boundary") as later:
            report = dict(protocols={}, processes=[])
            with self.assertRaisesRegex(RuntimeError, "witness"):
                gate.run_protocols(None, Path(folder), gate.Budget(), report)
            self.assertEqual(first.call_count, 1)
            later.assert_not_called()

    def test_cross_mode_pcm_mismatch_stops_before_rejection_and_timing_trials(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(gate, "lifecycle", return_value={}), \
                patch.object(gate, "boundary", side_effect=[dict(final={"pcm_sha256": "a"}), dict(final={"pcm_sha256": "b"})]), \
                patch.object(gate, "rejected_updates") as later:
            with self.assertRaisesRegex(RuntimeError, "PCM/text/tail differs"):
                gate.run_protocols(None, Path(folder), gate.Budget(), dict(protocols={}, processes=[]))
            later.assert_not_called()


@unittest.skipUnless(os.name == "posix", "POSIX process-group safety gate")
class ProcessCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name) / "process"
        self.records = []

    def worker(self, script, budget=None):
        return gate.worker([sys.executable, "-c", script], self.directory, "stream",
                           budget or gate.Budget(seconds=5, reserve=.5), self.records)

    def test_success_requires_normal_exit_and_preserves_raw_stdout(self):
        script = 'import json,time; time.sleep(.1); print(json.dumps({"event":"ready"}),flush=True); time.sleep(.1)'
        with self.worker(script) as native:
            native.until(lambda e: e["event"] == "ready")
        record = read_json(self.directory / "process.json")
        self.assertEqual(record["code"], 0)
        self.assertTrue(record["exit_confirmed"])
        self.assertTrue(record["group_absent"])
        self.assertFalse(record["killed"])
        self.assertEqual(json.loads((self.directory / "stdout.jsonl").read_text())["event"], "ready")

    def test_stalled_worker_is_killed_reaped_and_failure_evidence_remains(self):
        script = 'import json,time; print(json.dumps({"event":"ready"}),flush=True); time.sleep(60)'
        started = time.monotonic()
        with self.assertRaises((queue.Empty, TimeoutError)):
            with self.worker(script, gate.Budget(seconds=1, reserve=.25)) as native:
                native.until(lambda e: e["event"] == "ready")
                native.until(lambda e: e.get("event") == "done")
        self.assertLess(time.monotonic() - started, 5)
        record = read_json(self.directory / "process.json")
        self.assertTrue(record["killed"])
        self.assertTrue(record["exit_confirmed"])
        self.assertTrue(gate.group_absent(record["pid"]))
        self.assertNotEqual(record["code"], 0)

    def test_zero_exit_without_sampler_exit_evidence_is_not_accepted(self):
        sampler = Mock()
        sampler.start.return_value = sampler
        sampler.finish.return_value = dict(processes_exit_confirmed=False)
        with patch.object(gate, "ProcessTreeSampler", return_value=sampler):
            with self.assertRaisesRegex(RuntimeError, "complete exit evidence"):
                with self.worker('import time; time.sleep(.1)'):
                    pass
        record = read_json(self.directory / "process.json")
        self.assertEqual(record["code"], 0)
        self.assertFalse(record["exit_confirmed"])

    def test_malformed_worker_json_stops_and_reaps(self):
        with self.assertRaisesRegex(RuntimeError, "Worker reported an error"):
            with self.worker('import time; print("not json",flush=True); time.sleep(60)') as native:
                native.until(lambda e: False)
        self.assertTrue(self.records[0]["killed"])
        self.assertTrue(self.records[0]["exit_confirmed"])


class AssetOwnershipTests(unittest.TestCase):
    def make_assets(self, root):
        assets = gate.Assets(Mock(worker=Path("omascribe-asr")), root)
        assets.worker.parent.mkdir(parents=True)
        assets.model.mkdir()
        write_json(root / "owner.json", {"owner": assets.owner})
        for path in [assets.worker, assets.model / "tokens.txt"]:
            path.write_bytes(b"owned fixture")
            assets.copied[str(path)] = gate.file_identity(path)
        return assets

    def test_unknown_exit_or_member_prevents_any_clone_deletion(self):
        for reason in ("exit", "extra", "owner", "mutation", "symlink"):
            with self.subTest(reason=reason), tempfile.TemporaryDirectory() as folder:
                root = Path(folder) / "owned"
                assets = self.make_assets(root)
                if reason == "extra":
                    (root / "foreign.txt").write_text("retain")
                elif reason == "owner":
                    write_json(root / "owner.json", {"owner": "someone else"})
                elif reason == "mutation":
                    assets.worker.write_bytes(b"changed")
                elif reason == "symlink":
                    (root / "unexpected").symlink_to(assets.worker)
                with self.assertRaises(RuntimeError):
                    assets.cleanup(reason != "exit")
                self.assertTrue(assets.worker.exists())
                self.assertTrue((assets.model / "tokens.txt").exists())

    def test_only_verified_owned_copies_removed_original_sibling_is_retained(self):
        with tempfile.TemporaryDirectory() as folder:
            original = Path(folder) / "installed-model"
            original.write_bytes(b"original weights")
            assets = self.make_assets(Path(folder) / "owned")
            result = assets.cleanup(True)
            self.assertTrue(result["released"])
            self.assertEqual(len(result["files_before_cleanup"]), 2)
            self.assertFalse(assets.root.exists())
            self.assertEqual(original.read_bytes(), b"original weights")


if __name__ == "__main__":
    unittest.main()
