"""Deterministic process telemetry tests; these never launch an inference model."""
import json
import pathlib
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

import harness_support
import process_resources as resources


MIB = 1024**2


class FakeError(Exception):
    pass


class FakeNoSuchProcess(FakeError):
    pass


class FakeAccessDenied(FakeError):
    pass


class FakeClock:
    def __init__(self):
        self.elapsed = 0.
        self.sleeps = []

    def monotonic(self):
        return 100. + self.elapsed

    def time(self):
        return 1700000000. + self.elapsed

    def sleep(self, duration):
        self.sleeps.append(duration)
        self.elapsed += duration


class FakeProcess:
    def __init__(self, pid=10, created=1000., rss=2*MIB, pss=MIB, cpu=0.):
        self.pid, self.created = pid, created
        self.rss, self.pss, self.cpu = rss, pss, cpu
        self.descendants = []
        self.running = True
        self.process_status = "running"
        self.identity_error = None
        self.children_error = None
        self.rss_error = None
        self.pss_error = None
        self.status_error = None

    def create_time(self):
        if self.identity_error:
            raise self.identity_error
        return self.created

    def children(self, recursive):
        if self.children_error:
            raise self.children_error
        return self.descendants

    def cpu_times(self):
        return types.SimpleNamespace(user=self.cpu, system=0.)

    def memory_info(self):
        if self.rss_error:
            raise self.rss_error
        return types.SimpleNamespace(rss=self.rss)

    def memory_full_info(self):
        if self.pss_error:
            raise self.pss_error
        return types.SimpleNamespace() if self.pss is None else types.SimpleNamespace(pss=self.pss)

    def is_running(self):
        return self.running

    def status(self):
        if self.status_error:
            raise self.status_error
        return self.process_status


class FakePsutil:
    Error = FakeError
    NoSuchProcess = FakeNoSuchProcess
    STATUS_ZOMBIE = "zombie"
    STATUS_DEAD = "dead"
    __version__ = "fake-1"

    def __init__(self, *processes):
        self.processes = {process.pid: process for process in processes}
        self.lookups = 0

    def Process(self, pid):
        self.lookups += 1
        value = self.processes.get(pid)
        if isinstance(value, Exception):
            raise value
        if value is None:
            raise FakeNoSuchProcess()
        return value


class ProcessResourceTests(unittest.TestCase):
    def setUp(self):
        self.clock = FakeClock()
        self.root = FakeProcess()
        self.psutil = FakePsutil(self.root)
        self.psutil_patch = patch.object(resources, "_psutil", self.psutil)
        self.time_patch = patch.object(resources, "time", self.clock)
        self.psutil_patch.start()
        self.time_patch.start()
        self.addCleanup(self.psutil_patch.stop)
        self.addCleanup(self.time_patch.stop)

    def sampler(self, record_samples=True):
        sampler = resources.ProcessTreeSampler(self.root.pid, record_samples=record_samples)
        sampler._root = self.root
        sampler._started_monotonic = self.clock.monotonic()
        return sampler

    def finish_dead(self, sampler):
        self.psutil.processes.clear()
        return sampler.finish()

    def test_timeline_uses_monotonic_offsets_and_cumulative_cpu_identity_maxima(self):
        sampler = self.sampler()
        self.root.cpu = 3.
        sampler._sample()
        self.clock.elapsed = 2.
        self.root.cpu = 2.  # A later lower reading cannot decrease cumulative CPU.
        self.root.rss = 4*MIB
        sampler._sample()
        self.clock.elapsed = 3.
        replacement = FakeProcess(pid=10, created=2000., cpu=1., rss=6*MIB)
        sampler._root = replacement
        sampler._sample()
        result = self.finish_dead(sampler)
        self.assertEqual([sample["elapsed_seconds"] for sample in result["timeline"]], [0., 2., 3.])
        self.assertEqual([sample["unix_ms"] for sample in result["timeline"]],
                         [1700000000000, 1700000002000, 1700000003000])
        self.assertEqual([sample["cpu_seconds_sampled"] for sample in result["timeline"]], [3., 3., 4.])
        self.assertEqual(result["cpu_seconds_sampled"], 4.)
        self.assertEqual(result["processes_seen"], 2)
        self.assertEqual(result["tree_rss_mib_seconds_sampled"], 11.)
        self.assertEqual(result["observed_processes"], [
            dict(pid=10, create_time=1000., last_seen_elapsed_seconds=2.),
            dict(pid=10, create_time=2000., last_seen_elapsed_seconds=3.)])
        self.assertTrue(result["processes_exit_confirmed"])
        self.assertEqual(result["alive_after_finish"], [])

    def test_unknown_pss_and_missing_sample_are_null_and_do_not_become_zero_endpoints(self):
        sampler = self.sampler()
        self.root.pss = None
        sampler._sample()
        self.clock.elapsed = 1.
        self.root.children_error = FakeNoSuchProcess()
        sampler._sample()
        self.clock.elapsed = 3.
        self.root.children_error = None
        self.root.rss = 8*MIB
        self.root.pss_error = FakeAccessDenied()
        sampler._sample()
        result = self.finish_dead(sampler)
        self.assertIsNone(result["peak_tree_pss_mib"])
        self.assertIsNone(result["tree_rss_mib_seconds_sampled"])
        self.assertEqual([sample["tree_rss_mib"] for sample in result["timeline"]], [2., None, None])
        self.assertEqual([sample["tree_pss_mib"] for sample in result["timeline"]], [None, None, None])
        self.assertEqual([sample["processes_observed"] for sample in result["timeline"]], [1, 0, 1])
        self.assertGreater(result["sample_errors"], 0)

    def test_readable_parent_and_inaccessible_child_make_memory_unknown_without_integration(self):
        for error in (FakeAccessDenied(), OSError("fixture failure")):
            with self.subTest(error=type(error).__name__):
                self.clock.elapsed = 0.
                self.root.descendants = []
                sampler = self.sampler()
                sampler._sample()
                self.clock.elapsed = 1.
                child = FakeProcess(pid=11, created=1100.)
                child.rss_error = error
                self.root.descendants = [child]
                sampler._sample()
                self.clock.elapsed = 2.
                self.root.descendants = []
                sampler._sample()
                with patch.object(resources, "_MAX_TIMELINE_SAMPLES", 3):
                    self.clock.elapsed = 3.
                    sampler._sample()
                result = self.finish_dead(sampler)
                self.assertGreater(result["sample_errors"], 0)
                self.assertEqual([sample["tree_rss_mib"] for sample in result["timeline"]], [2., None, 2.])
                self.assertEqual([sample["tree_pss_mib"] for sample in result["timeline"]], [1., None, 1.])
                self.assertIsNone(result["tree_rss_mib_seconds_sampled"])
                self.assertTrue(result["timeline_truncated"])
                self.assertEqual(result["timeline_dropped_samples"], 1)
                self.assertEqual(result["timeline"][1]["cpu_seconds_sampled"], 0.)

    def test_known_zero_is_preserved_and_no_samples_stays_unknown(self):
        sampler = self.sampler()
        empty = self.finish_dead(sampler)
        for field in ("cpu_seconds_sampled", "peak_tree_rss_mib", "peak_tree_pss_mib", "tree_rss_mib_seconds_sampled"):
            self.assertIsNone(empty[field])
        self.assertFalse(empty["processes_exit_confirmed"])
        self.root.rss = self.root.pss = self.root.cpu = 0.
        sampler._sample()
        self.clock.elapsed = 1.
        sampler._sample()
        result = self.finish_dead(sampler)
        for field in ("cpu_seconds_sampled", "peak_tree_rss_mib", "peak_tree_pss_mib", "tree_rss_mib_seconds_sampled"):
            self.assertEqual(result[field], 0.)
        self.assertEqual(result["timeline"][0]["cpu_seconds_sampled"], 0.)

    def test_all_observed_descendants_are_checked_and_polling_is_bounded_without_killing(self):
        child = FakeProcess(pid=11, created=1100.)
        self.root.descendants = [child]
        self.psutil.processes[child.pid] = child
        sampler = self.sampler()
        sampler._sample()
        del self.psutil.processes[self.root.pid]
        result = sampler.finish()
        self.assertFalse(result["processes_exit_confirmed"])
        self.assertEqual(result["alive_after_finish"], [dict(pid=11, create_time=1100.)])
        self.assertEqual(result["exit_check_errors"], 0)
        self.assertAlmostEqual(self.clock.elapsed, 2.)
        self.assertLessEqual(len(self.clock.sleeps), 41)
        self.assertTrue(all(0 < duration <= .05 for duration in self.clock.sleeps))

    def test_pid_reuse_confirms_old_identity_exited_without_waiting_for_replacement(self):
        sampler = self.sampler()
        sampler._sample()
        self.psutil.processes[10] = FakeProcess(pid=10, created=2000.)
        result = sampler.finish()
        self.assertTrue(result["processes_exit_confirmed"])
        self.assertEqual(result["alive_after_finish"], [])
        self.assertEqual(self.clock.sleeps, [])

    def test_permission_errors_are_unknown_and_cannot_confirm_exit(self):
        for failure_point in ("lookup", "identity", "status"):
            with self.subTest(failure_point=failure_point):
                self.clock.elapsed = 0.
                self.clock.sleeps.clear()
                self.psutil.processes[10] = self.root
                self.root.identity_error = self.root.status_error = None
                sampler = self.sampler()
                sampler._sample()
                if failure_point == "lookup":
                    self.psutil.processes[10] = FakeAccessDenied()
                elif failure_point == "identity":
                    self.root.identity_error = FakeAccessDenied()
                else:
                    self.root.status_error = FakeAccessDenied()
                result = sampler.finish()
                self.assertFalse(result["processes_exit_confirmed"])
                self.assertGreater(result["exit_check_errors"], 0)
                self.assertEqual(result["alive_after_finish"], [])
                self.assertAlmostEqual(self.clock.elapsed, 2.)

    def test_zombie_and_no_longer_running_identities_are_confirmed_exited(self):
        for status, running in (("zombie", True), ("dead", True), ("running", False)):
            with self.subTest(status=status, running=running):
                sampler = self.sampler()
                sampler._sample()
                self.root.process_status, self.root.running = status, running
                self.assertTrue(sampler.finish()["processes_exit_confirmed"])
                self.assertEqual(self.clock.sleeps, [])

    def test_timeline_cap_reports_dropped_samples_but_aggregate_and_identities_continue(self):
        sampler = self.sampler()
        with patch.object(resources, "_MAX_TIMELINE_SAMPLES", 2):
            for elapsed in range(5):
                self.clock.elapsed = elapsed
                self.root.cpu = elapsed
                self.root.rss = (elapsed + 1)*MIB
                sampler._sample()
        result = self.finish_dead(sampler)
        self.assertEqual(len(result["timeline"]), 2)
        self.assertEqual(result["samples"], 5)
        self.assertTrue(result["timeline_truncated"])
        self.assertEqual(result["timeline_dropped_samples"], 3)
        self.assertEqual(result["peak_tree_rss_mib"], 5.)
        self.assertEqual(result["cpu_seconds_sampled"], 4.)
        self.assertEqual(result["tree_rss_mib_seconds_sampled"], 1.5)
        self.assertEqual(result["observed_processes"][0]["last_seen_elapsed_seconds"], 4.)

    def test_default_compatibility_has_no_timeline_or_exit_polling(self):
        sampler = self.sampler(record_samples=False)
        sampler._sample()
        with patch.object(sampler, "_check_process_exits", side_effect=AssertionError("opt-in only")):
            result = sampler.finish()
        self.assertEqual(result["samples"], 1)
        self.assertEqual(result["cpu_seconds_sampled"], 0.)
        self.assertNotIn("timeline", result)
        self.assertNotIn("processes_exit_confirmed", result)
        self.assertEqual(self.clock.sleeps, [])

    def test_psutil_unavailable_is_unknown_and_starts_no_thread(self):
        with patch.object(resources, "_psutil", None):
            sampler = resources.ProcessTreeSampler(10, record_samples=True).start()
            result = sampler.finish()
        self.assertEqual(result["timeline"], [])
        self.assertIsNone(result["cpu_seconds_sampled"])
        self.assertFalse(result["processes_exit_confirmed"])
        self.assertTrue(result["sampler_stopped"])


class HarnessResourceTests(unittest.TestCase):
    def test_opt_in_writes_raw_timeline_once_and_checks_only_after_process_wait(self):
        waited = []
        process = types.SimpleNamespace(pid=123, wait=lambda timeout: waited.append(True) or 0)
        full = dict(schema=1, timeline=[dict(elapsed_seconds=0., unix_ms=1700000000000)],
                    observed_processes=[dict(pid=123, create_time=1000., last_seen_elapsed_seconds=0.)],
                    processes_exit_confirmed=True, alive_after_finish=[], exit_check_errors=0)
        sampler = Mock()
        sampler.start.return_value = sampler

        def finish():
            self.assertEqual(waited, [True])
            return full

        sampler.finish.side_effect = finish

        def popen(*args, **kwargs):
            kwargs["stdout"].write('{"ok":true}')
            return process

        with tempfile.TemporaryDirectory() as folder, patch.object(harness_support.subprocess, "Popen", popen), \
                patch.object(harness_support, "ProcessTreeSampler", return_value=sampler) as constructor:
            directory = pathlib.Path(folder)
            result = harness_support.command(["fictional-cli"], {}, directory, resource_samples=True)
            self.assertEqual(result, dict(ok=True))
            constructor.assert_called_once_with(123, record_samples=True)
            self.assertEqual(json.loads((directory / "resources.json").read_text()), full)
            summary = json.loads((directory / "process.json").read_text())["resources"]
            self.assertEqual(summary["timeline_file"], "resources.json")
            self.assertTrue(summary["processes_exit_confirmed"])
            self.assertNotIn("timeline", summary)
            self.assertNotIn("observed_processes", summary)

    def test_default_does_not_request_or_write_samples(self):
        process = types.SimpleNamespace(pid=123, wait=lambda timeout: 0)
        sampler = Mock()
        sampler.start.return_value = sampler
        sampler.finish.return_value = dict(schema=1, samples=0)

        def popen(*args, **kwargs):
            kwargs["stdout"].write('{}')
            return process

        with tempfile.TemporaryDirectory() as folder, patch.object(harness_support.subprocess, "Popen", popen), \
                patch.object(harness_support, "ProcessTreeSampler", return_value=sampler) as constructor:
            directory = pathlib.Path(folder)
            harness_support.command(["fictional-cli"], {}, directory)
            constructor.assert_called_once_with(123)
            self.assertFalse((directory / "resources.json").exists())
            self.assertEqual(json.loads((directory / "process.json").read_text())["resources"], dict(schema=1, samples=0))


if __name__ == "__main__":
    unittest.main()
