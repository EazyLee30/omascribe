import copy
import json
import pathlib
import platform
import shutil
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from benchmark_keep_alive import (IDLE_SECONDS, ORDER, clone_asset, comparison_summary, file_identity, idle_summary,
                                  incurred_inference_costs, inference_summary,
                                  resource_summary, rss_integral, snapshot_summary, writer_resource_summary)
import benchmark_keep_alive
from harness_support import sha256


class KeepAliveTests(unittest.TestCase):
    def snapshot(self):
        segments = [dict(id="public-source-id-0001", meeting_id="meeting", run=2, seq=1,
                         start_ms=0, end_ms=1000, text="预算3000元尚未批准。", revision=3),
                    dict(id="public-source-id-0002", meeting_id="meeting", run=2, seq=2,
                         start_ms=1000, end_ms=2000, text="Morgan will send the report on Thursday.", revision=1)]
        return dict(meeting=dict(id="meeting", run=2, auto_notes=True, duration_ms=2000, notes_cursor=2),
            segments=segments, translation_mode="bilingual",
            translations={segment["id"]: dict(segment_id=segment["id"], revision=segment["revision"],
                target="en" if index == 0 else "zh", model="qwen3-1.7b", text="translated", cache_key="verified")
                for index, segment in enumerate(segments)},
            suggestions=[dict(meeting_id="meeting", stale=False, text="A public-fixture summary.",
                sources=[dict(segment_id=segment["id"], revision=segment["revision"], start_ms=segment["start_ms"],
                              end_ms=segment["end_ms"], quote=segment["text"]) for segment in segments])],
            runtime=dict(error=None, recording=False, transcribing=False, notes_busy=False, translation_busy=False))

    def events(self):
        rows = []
        for task in ("translation", "minutes"):
            def row(phase, metrics):
                return dict(schema=1, job=task+"-job", task=task, phase=phase, metrics=metrics)
            rows.extend([row("started", {}),
                         row("worker", dict(backend="metal", startup_ms=420, reused=task == "minutes")),
                         row("request_started", dict(attempt=1, endpoint="chat")),
                         row("generation", dict(attempt=1, completed=True, request_ms=200, first_token_ms=20,
                             usage=dict(input_tokens=30, output_tokens=10, cached_tokens=0, prefill_ms=15., decode_ms=150.))),
                         row("finished", dict(committed=True, items=1))])
        return rows

    def idle(self, ttl=10, idle_age_ms=0, zero_reason="batch_end"):
        remaining = ttl*1000-idle_age_ms
        def worker(at):
            released = bool(ttl and at >= remaining)
            life = dict(schema=1, releases=2+int(released) if ttl else 3,
                        ttl_releases=2+int(released) if ttl else 0,
                        batch_releases=0 if ttl or zero_reason == "lease_end" else 3,
                        lease_releases=3 if not ttl and zero_reason == "lease_end" else 0,
                        other_releases=0, resource_stops=0, admission_refusals=0, retention_refusals=0)
            life["last_release"] = dict(sequence=life["releases"], pid=400 if released or not ttl else 399,
                reason="ttl_expired" if ttl else zero_reason, ttl_ms=ttl*1000,
                idle_ms=ttl*1000 if ttl else 2000 if zero_reason == "batch_grace_expired" else None,
                exit_confirmed=True)
            resident = bool(ttl and not released)
            return dict(leased=False, starts=3, reuses=2, idle_pid=400 if resident else None,
                        idle_for_ms=idle_age_ms+at if resident else None,
                        expires_in_ms=remaining-at if resident else None, lifecycle=life)
        observation = dict(requested_seconds=IDLE_SECONDS, elapsed_ms=IDLE_SECONDS*1000,
            started_wall_ms=4000, started_unix_ms=4000, completed=True, queue_unchanged=True,
            worker_at_start=worker(0), worker_at_end=worker(IDLE_SECONDS*1000),
            worker_stopped_after_ms=remaining if ttl else None)
        rows = [dict(phase="capture", unix_ms=1000, idle_elapsed_ms=None, worker=worker(0)),
                dict(phase="settle", unix_ms=3000, idle_elapsed_ms=None, worker=worker(0))]
        rows += [dict(phase="idle", unix_ms=4000+at, idle_elapsed_ms=at,
                      notes_busy=False, translation_busy=False, worker=worker(at))
                 for at in range(0, IDLE_SECONDS*1000+1, 1000)]
        return dict(idle_observation=observation), rows

    def resources(self):
        return dict(schema=1, interval_seconds=.25, sampler_stopped=True, processes_exit_confirmed=True,
            alive_after_finish=[], exit_check_errors=0, sample_errors=0, timeline_truncated=False,
            cpu_seconds_sampled=4., peak_tree_rss_mib=100., peak_tree_pss_mib=None, processes_seen=2,
            observed_processes=[dict(pid=400, create_time=123., last_seen_elapsed_seconds=15.)],
            timeline=[dict(elapsed_seconds=float(at), unix_ms=at*1000, tree_rss_mib=100.,
                           tree_pss_mib=None, cpu_seconds_sampled=at/10, processes_observed=2)
                      for at in range(38)])

    def test_complete_coverage_requires_exact_source_ids_revisions_run_and_times(self):
        original = self.snapshot()
        checked = snapshot_summary(original, 2)
        self.assertEqual((checked["translations"], checked["cited_segments"]), (2, 2))
        changes = [lambda s: s["segments"][0].update(run=1),
                   lambda s: s["segments"][0].update(meeting_id="another"),
                   lambda s: s["segments"][0].update(end_ms=2500),
                   lambda s: s["translations"].pop("public-source-id-0002"),
                   lambda s: s["translations"]["public-source-id-0001"].update(target="zh"),
                   lambda s: s["translations"]["public-source-id-0001"].update(revision=1),
                   lambda s: s["suggestions"][0]["sources"][0].update(segment_id="public-source-id-000"),
                   lambda s: s["suggestions"][0]["sources"][0].update(start_ms=1),
                   lambda s: s["suggestions"][0]["sources"][0].update(quote="invented"),
                   lambda s: s["suggestions"][0].update(sources=[]),
                   lambda s: s["suggestions"][0].update(stale=True),
                   lambda s: s["meeting"].update(notes_cursor=1)]
        for change in changes:
            bad = copy.deepcopy(original)
            change(bad)
            with self.assertRaises(ValueError):
                snapshot_summary(bad, 2)

    def test_full_review_does_not_require_citing_every_redundant_segment(self):
        snapshot = self.snapshot()
        snapshot["suggestions"][0]["sources"].pop()
        checked = snapshot_summary(snapshot, 2)
        self.assertEqual(checked["notes_cursor"], 2)
        self.assertEqual(checked["cited_segment_ratio"], .5)
        self.assertEqual(checked["cited_segment_ids"], ["public-source-id-0001"])
        self.assertEqual(len(checked["source_segment_ids"]), 2)

    def test_actual_committed_both_tasks_are_required_not_workers_or_literals(self):
        events = self.events()
        self.assertEqual(inference_summary(events)["attempted_requests"], 2)
        for predicate in [lambda row: row["phase"] == "worker",
                          lambda row: row["task"] == "translation",
                          lambda row: row["phase"] != "request_started",
                          lambda row: row["phase"] != "generation"]:
            with self.assertRaises(ValueError):
                inference_summary([row for row in events if predicate(row)])
        fallback = copy.deepcopy(events)
        next(row for row in fallback if row["phase"] == "worker")["metrics"]["backend"] = "cpu"
        with self.assertRaises(ValueError):
            inference_summary(fallback)
        uncommitted = copy.deepcopy(events)
        next(row for row in uncommitted if row["phase"] == "finished")["metrics"]["committed"] = False
        with self.assertRaises(ValueError):
            inference_summary(uncommitted)

    def test_failed_requests_repair_and_unknown_tokens_remain_in_cost(self):
        events = self.events()
        index = next(i for i, row in enumerate(events) if row["task"] == "minutes" and row["phase"] == "generation")
        events[index]["metrics"].update(completed=False, usage=None)
        events[index+1:index+1] = [
            dict(schema=1, job="minutes-job", task="minutes", phase="validation_failed", metrics={}),
            dict(schema=1, job="minutes-job", task="minutes", phase="request_started", metrics=dict(attempt=2, endpoint="chat")),
            dict(schema=1, job="minutes-job", task="minutes", phase="generation",
                 metrics=dict(attempt=2, completed=True, request_ms=300, usage=None))]
        summary = inference_summary(events)
        self.assertEqual(summary["attempted_requests"], 3)
        self.assertEqual(summary["tasks"]["minutes"]["repaired_requests"], 1)
        self.assertEqual(summary["tasks"]["minutes"]["request_ms"], 500)
        self.assertIsNone(summary["tasks"]["minutes"]["input_tokens"])
        costs = incurred_inference_costs(events)
        self.assertEqual((costs["request_ms"], costs["validation_failures"]), (700, 1))
        self.assertIsNone(costs["output_tokens"])
        self.assertIsNone(incurred_inference_costs([])["attempted_requests"])
        events.append(dict(schema=1, job="translation-job", task="translation", phase="request_started",
                           metrics=dict(attempt=2, endpoint="chat")))
        with self.assertRaises(ValueError):
            inference_summary(events)
        self.assertIsNone(incurred_inference_costs(events)["request_ms"])

    def test_malformed_trace_keeps_observed_cost_and_marks_totals_unknown(self):
        for bad in (None, [], dict(phase="generation", metrics=[]),
                    dict(phase="generation", metrics=dict(usage=[]))):
            with self.subTest(bad=bad):
                result = incurred_inference_costs(self.events()+[bad])
                self.assertGreater(result["trace_error_count"], 0)
                self.assertIsNone(result["attempted_requests"])
                self.assertIsNone(result["request_ms"])
                self.assertIsNone(result["input_tokens"])
                self.assertEqual(result["observed_attempted_requests"], 2)
                self.assertEqual(result["observed_request_ms"], 400)

    def test_final_evidence_failure_cannot_skip_model_link_cleanup(self):
        for failure in ("process", "trace", "cost"):
            with self.subTest(failure=failure):
                result = dict(errors=[])
                with patch("benchmark_keep_alive.read_json", side_effect=OSError("process fixture") if failure == "process" else None), \
                        patch("benchmark_keep_alive.trace_events", side_effect=ValueError("trace fixture") if failure == "trace" else None, return_value=[]), \
                        patch("benchmark_keep_alive.incurred_inference_costs", side_effect=RuntimeError("cost fixture") if failure == "cost" else None, return_value={}), \
                        patch("benchmark_keep_alive.cleanup_models", return_value=dict(released=True)) as cleanup:
                    benchmark_keep_alive.collect_final_evidence(pathlib.Path("owned"), pathlib.Path("arm"), result)
                cleanup.assert_called_once_with(pathlib.Path("owned"))
                self.assertTrue(result["model_cleanup"]["released"])
                self.assertTrue(result["errors"])
    def test_planned_source_stages_do_not_count_as_repairs_but_keep_all_costs(self):
        events = self.events()
        index = next(i for i, row in enumerate(events)
                     if row["task"] == "minutes" and row["phase"] == "request_started")
        events[index]["metrics"]["stage"] = "prepare_sources"
        request, generation = copy.deepcopy(events[index]), copy.deepcopy(events[index+1])
        request["metrics"].update(attempt=2, stage="compose_prepared_sources")
        generation["metrics"]["attempt"] = 2
        events[index+2:index+2] = [request, generation]
        result = inference_summary(events)
        minutes = result["tasks"]["minutes"]
        self.assertEqual((result["attempted_requests"], minutes["attempted_requests"], minutes["repaired_requests"]), (3, 2, 0))
        self.assertEqual((minutes["request_ms"], minutes["input_tokens"], minutes["output_tokens"]), (400, 60, 20))
        self.assertEqual(incurred_inference_costs(events)["request_ms"], 600)

    def test_request_stage_overrides_attempt_and_legacy_keeps_its_meaning(self):
        for stage, number, expected in (("prepare_sources", 2, 0), ("compose", 2, 0),
                                        ("compose_prepared_sources", 2, 0), ("repair", 1, 1),
                                        (None, 2, 1)):
            with self.subTest(stage=stage):
                events = self.events()
                index = next(i for i, row in enumerate(events)
                             if row["task"] == "minutes" and row["phase"] == "request_started")
                if number == 2:
                    # Lifecycle evidence requires a real sequential first call,
                    # even when the second call is planned rather than a repair.
                    request, generated = copy.deepcopy(events[index]), copy.deepcopy(events[index+1])
                    request["metrics"]["attempt"] = generated["metrics"]["attempt"] = 2
                    events[index+2:index+2] = [request, generated]
                else:
                    request = events[index]
                if stage is not None:
                    request["metrics"]["stage"] = stage
                result = inference_summary(events)["tasks"]["minutes"]
                self.assertEqual(result["repaired_requests"], expected)
                self.assertEqual((result["attempted_requests"], result["request_ms"]), (number, number*200))
        request["metrics"]["stage"] = "unknown_stage"
        with self.assertRaises(ValueError):
            inference_summary(events)

    def test_same_idle_horizon_reaps_all_ttls_and_refuses_latent_work(self):
        for ttl in (0, 10, 30):
            response, rows = self.idle(ttl)
            self.assertTrue(idle_summary(response, rows, ttl)["completed"])
        response, rows = self.idle()
        for change in [lambda r, t: r["idle_observation"].update(requested_seconds=12),
                       lambda r, t: r["idle_observation"].update(completed=False),
                       lambda r, t: r["idle_observation"].update(queue_unchanged=False),
                       lambda r, t: r["idle_observation"].update(worker_stopped_after_ms=None),
                       lambda r, t: r["idle_observation"]["worker_at_end"].update(idle_pid=400),
                       lambda r, t: t[-1]["worker"].update(starts=4),
                       lambda r, t: t[-1].update(notes_busy=True),
                       lambda r, t: t[-1].update(idle_elapsed_ms=None)]:
            bad, telemetry = copy.deepcopy(response), copy.deepcopy(rows)
            change(bad, telemetry)
            with self.assertRaises(ValueError):
                idle_summary(bad, telemetry, 10)

    def early_disappearance(self, ttl):
        response, rows = self.idle(ttl)
        response["idle_observation"]["worker_stopped_after_ms"] = 1000
        for row in rows:
            if row.get("phase") == "idle" and row["idle_elapsed_ms"] >= 1000:
                row["worker"]["idle_pid"] = None
                row["worker"]["idle_for_ms"] = row["worker"]["expires_in_ms"] = None
        return response, rows

    def test_ttl10_early_disappearance_is_not_ttl_proof(self):
        response, rows = self.early_disappearance(10)
        with self.assertRaises(ValueError):
            idle_summary(response, rows, 10)

    def test_ttl30_early_disappearance_is_not_ttl_proof(self):
        response, rows = self.early_disappearance(30)
        with self.assertRaises(ValueError):
            idle_summary(response, rows, 30)

    def test_unsuccessful_pre_request_job_cannot_hide_behind_successful_jobs(self):
        events = self.events() + [
            dict(schema=1, job="admission-refused", task="minutes", phase="started", metrics={}),
            dict(schema=1, job="admission-refused", task="minutes", phase="finished",
                 metrics=dict(committed=False, items=None))]
        with self.assertRaises(ValueError):
            inference_summary(events)

    def test_each_chat_job_requires_one_worker_before_its_first_request(self):
        for mutation in ("missing", "duplicate", "late"):
            with self.subTest(mutation=mutation):
                events = self.events()
                index = next(i for i, row in enumerate(events)
                             if row["task"] == "minutes" and row["phase"] == "worker")
                worker = events.pop(index)
                if mutation == "duplicate":
                    events[index:index] = [worker, copy.deepcopy(worker)]
                elif mutation == "late":
                    events.insert(index+1, worker)
                with self.assertRaises(ValueError):
                    inference_summary(events)

    def test_unknown_trace_phase_and_missing_request_sequence_cannot_pass(self):
        for mutation in ("unknown", "missing_first", "missing_middle", "overlap"):
            with self.subTest(mutation=mutation):
                events = self.events()
                index = next(i for i, row in enumerate(events)
                             if row["task"] == "minutes" and row["phase"] == "request_started")
                if mutation == "unknown":
                    events.insert(index, dict(schema=1, job="minutes-job", task="minutes",
                                              phase="worker_died_unknown", metrics={}))
                elif mutation == "missing_first":
                    events[index]["metrics"]["attempt"] = events[index+1]["metrics"]["attempt"] = 2
                else:
                    request, generation = copy.deepcopy(events[index]), copy.deepcopy(events[index+1])
                    request["metrics"]["attempt"] = generation["metrics"]["attempt"] = 3 if mutation == "missing_middle" else 2
                    insertion = index+2 if mutation == "missing_middle" else index+1
                    events[insertion:insertion] = [request, generation]
                with self.assertRaises(ValueError):
                    inference_summary(events)

    def test_earlier_completed_response_cannot_hide_a_failed_final_response(self):
        events = self.events()
        index = next(i for i, row in enumerate(events)
                     if row["task"] == "minutes" and row["phase"] == "generation")
        request, generation = copy.deepcopy(events[index-1]), copy.deepcopy(events[index])
        request["metrics"]["attempt"] = generation["metrics"]["attempt"] = 2
        generation["metrics"]["completed"] = False
        events[index+1:index+1] = [request, generation]
        with self.assertRaises(ValueError):
            inference_summary(events)
        self.assertEqual(incurred_inference_costs(events)["attempted_requests"], 3)

    def test_known_schema_one_metadata_phases_remain_accepted(self):
        events = self.events()
        for task, phases in (("minutes", ("retrieval", "article_state", "article_prompt", "article_repair")),
                             ("translation", ("chunk", "literal_caption", "validation_failed",
                                              "source_unit_repair", "source_week_repair", "batch"))):
            index = next(i for i, row in enumerate(events)
                         if row["task"] == task and row["phase"] == "request_started")
            events[index:index] = [dict(schema=1, job=task+"-job", task=task, phase=phase, metrics={})
                                   for phase in phases]
        self.assertEqual(inference_summary(events)["attempted_requests"], 2)

    def test_remaining_expiry_not_full_ttl_anchors_reap_time(self):
        response, rows = self.idle(10, idle_age_ms=9000)
        self.assertEqual(idle_summary(response, rows, 10)["worker_stopped_after_ms"], 1000)
        response["idle_observation"]["worker_stopped_after_ms"] = 10000
        with self.assertRaisesRegex(ValueError, "reaping"):
            idle_summary(response, rows, 10)

    def test_slot_disappearance_cannot_hide_a_late_confirmed_exit(self):
        response, rows = self.idle()
        previous_release = response["idle_observation"]["worker_at_start"]["lifecycle"]
        for row in rows:
            if row.get("phase") == "idle" and 10000 <= row["idle_elapsed_ms"] < 31000:
                row["worker"]["lifecycle"] = copy.deepcopy(previous_release)
        # The pool slot disappeared at its deadline, but the process was not
        # confirmed reaped until 31s. The existing 2s deadline still applies.
        with self.assertRaises(ValueError):
            idle_summary(response, rows, 10)

    def test_missing_unknown_and_inconsistent_lifecycle_evidence_cannot_pass(self):
        changes = [lambda w: w.pop("lifecycle"),
                   lambda w: w["lifecycle"].update(schema=True),
                   lambda w: w["lifecycle"].update(releases=None),
                   lambda w: w["lifecycle"].update(releases=99),
                   lambda w: w["lifecycle"].update(retention_refusals=None),
                   lambda w: w["lifecycle"]["last_release"].update(sequence=99),
                   lambda w: w["lifecycle"]["last_release"].update(exit_confirmed=None),
                   lambda w: w["lifecycle"]["last_release"].update(exit_confirmed=False),
                   lambda w: w["lifecycle"]["last_release"].update(reason="system_pressure"),
                   lambda w: w["lifecycle"]["last_release"].update(ttl_ms=30000),
                   lambda w: w["lifecycle"]["last_release"].update(idle_ms=9999),
                   lambda w: w["lifecycle"]["last_release"].pop("idle_ms"),
                   lambda w: w["lifecycle"]["last_release"].update(pid=401),
                   lambda w: w.pop("expires_in_ms")]
        for change in changes:
            with self.subTest(change=changes.index(change)):
                response, rows = self.idle()
                change(response["idle_observation"]["worker_at_end"])
                with self.assertRaises(ValueError):
                    idle_summary(response, rows, 10)

    def test_historical_releases_cannot_belong_to_a_different_fixed_policy(self):
        for ttl, counter in ((10, "batch_releases"), (10, "lease_releases"), (0, "ttl_releases")):
            with self.subTest(ttl=ttl, counter=counter):
                response, rows = self.idle(ttl)
                observation = response["idle_observation"]
                workers = [observation["worker_at_start"], observation["worker_at_end"]]
                workers += [row["worker"] for row in rows]
                for worker in workers:
                    life = worker["lifecycle"]
                    life[counter] += 1
                    life["releases"] += 1
                    life["last_release"]["sequence"] += 1
                with self.assertRaises(ValueError):
                    idle_summary(response, rows, ttl)

    def test_adverse_capture_or_settle_counters_cannot_be_hidden_by_clean_idle(self):
        for phase in ("capture", "settle"):
            for key in ("resource_stops", "admission_refusals", "retention_refusals", "other_releases"):
                with self.subTest(phase=phase, counter=key):
                    response, rows = self.idle()
                    row = next(row for row in rows if row["phase"] == phase)
                    row["worker"]["lifecycle"][key] = 1
                    with self.assertRaisesRegex(ValueError, "adverse"):
                        idle_summary(response, rows, 10)

    def test_counter_reset_identity_rewrite_and_additional_release_cannot_pass(self):
        for mutation in ("reset", "rewrite", "additional"):
            with self.subTest(mutation=mutation):
                response, rows = self.idle()
                if mutation == "reset":
                    life = response["idle_observation"]["worker_at_end"]["lifecycle"]
                    life["releases"] = life["ttl_releases"] = life["last_release"]["sequence"] = 1
                elif mutation == "rewrite":
                    rows[0]["worker"]["lifecycle"]["last_release"]["pid"] = 398
                else:
                    life = response["idle_observation"]["worker_at_end"]["lifecycle"]
                    life["releases"] += 1
                    life["ttl_releases"] += 1
                    life["last_release"]["sequence"] += 1
                with self.assertRaises(ValueError):
                    idle_summary(response, rows, 10)

    def test_zero_policy_allows_healthy_batch_lease_and_two_second_grace_releases(self):
        for reason in ("batch_end", "lease_end", "batch_grace_expired"):
            with self.subTest(reason=reason):
                response, rows = self.idle(0, zero_reason=reason)
                self.assertTrue(idle_summary(response, rows, 0)["completed"])
        response, rows = self.idle(0, zero_reason="batch_grace_expired")
        response["idle_observation"]["worker_at_start"]["lifecycle"]["last_release"]["idle_ms"] = 1999
        with self.assertRaises(ValueError):
            idle_summary(response, rows, 0)

    def test_zero_policy_requires_prior_release_and_no_idle_release(self):
        for mutation in ("no_prior", "new_release", "ttl_reason"):
            with self.subTest(mutation=mutation):
                response, rows = self.idle(0)
                if mutation == "no_prior":
                    for worker in [response["idle_observation"]["worker_at_start"], response["idle_observation"]["worker_at_end"], *(row["worker"] for row in rows)]:
                        worker["lifecycle"].update(releases=0, batch_releases=0, last_release=None)
                elif mutation == "new_release":
                    life = response["idle_observation"]["worker_at_end"]["lifecycle"]
                    life["releases"] = life["batch_releases"] = life["last_release"]["sequence"] = 4
                else:
                    response["idle_observation"]["worker_at_end"]["lifecycle"]["last_release"]["reason"] = "ttl_expired"
                with self.assertRaises(ValueError):
                    idle_summary(response, rows, 0)

    def test_inference_jobs_require_ordered_successful_terminal_evidence(self):
        for mutation in ("dangling", "abandoned", "no_start", "duplicate_finish", "after_finish", "missing_response", "response_first", "duplicate_attempt", "only_incomplete", "bad_items"):
            with self.subTest(mutation=mutation):
                events = self.events()
                if mutation == "dangling": events.pop()
                elif mutation == "abandoned": events[-1]["phase"] = "abandoned"
                elif mutation == "no_start": events.pop(0)
                elif mutation == "duplicate_finish": events.append(copy.deepcopy(events[-1]))
                elif mutation == "after_finish": events.append(dict(schema=1, job="minutes-job", task="minutes", phase="validation_failed", metrics={}))
                elif mutation == "missing_response": events.pop(-2)
                elif mutation == "response_first": events[-3], events[-2] = events[-2], events[-3]
                elif mutation == "duplicate_attempt": events.insert(-2, copy.deepcopy(events[-3]))
                elif mutation == "only_incomplete": events[-2]["metrics"]["completed"] = False
                else: events[-1]["metrics"]["items"] = True
                with self.assertRaises(ValueError):
                    inference_summary(events)
                # Invalid closure never erases incurred attempted work.
                self.assertIsNotNone(incurred_inference_costs(events)["attempted_requests"])

    def test_closed_successful_noop_job_is_allowed_alongside_actual_committed_work(self):
        events = self.events() + [dict(schema=1, job="noop", task="translation", phase="started", metrics={}),
                                  dict(schema=1, job="noop", task="translation", phase="finished", metrics=dict(committed=True, items=0))]
        self.assertEqual(inference_summary(events)["attempted_requests"], 2)

    def test_rejected_completed_attempt_can_be_repaired_within_successful_job(self):
        events = self.events()
        index = next(i for i, row in enumerate(events) if row["task"] == "minutes" and row["phase"] == "generation")
        repaired = copy.deepcopy(events[index])
        repaired["metrics"]["attempt"] = 2
        events[index+1:index+1] = [dict(schema=1, job="minutes-job", task="minutes", phase="validation_failed", metrics={}),
            dict(schema=1, job="minutes-job", task="minutes", phase="request_started", metrics=dict(attempt=2, endpoint="chat")), repaired]
        result = inference_summary(events)
        self.assertEqual(result["tasks"]["minutes"]["attempted_requests"], 2)
        self.assertEqual(result["tasks"]["minutes"]["validation_failures"], 1)
        self.assertEqual(result["tasks"]["minutes"]["repaired_requests"], 1)

    def test_writer_diagnostic_is_hashed_negative_screen_and_never_ttl_proof(self):
        good = dict(backend="metal", budget_mib=4096, available_before_mib=2048)
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder)/"last-writer-resource.json"
            path.write_text(json.dumps(good))
            self.assertEqual(writer_resource_summary(path)["sha256"], sha256(path))
            response, rows = self.early_disappearance(30)
            with self.assertRaises(ValueError):
                idle_summary(response, rows, 30)
            for bad in (None, {}, {**good, "reason":"system_pressure"}, {**good, "reason":"process_budget"},
                        {**good, "reason":None}, {**good, "available_before_mib":None},
                        {**good, "available_before_mib":False}, {**good, "backend":"cpu"}, {**good, "budget_mib":2048}):
                with self.subTest(diagnostic=bad):
                    path.write_text(json.dumps(bad))
                    with self.assertRaises(ValueError):
                        writer_resource_summary(path)
            path.write_text("invalid json")
            with self.assertRaises(ValueError):
                writer_resource_summary(path)
            path.unlink()
            with self.assertRaises(ValueError):
                writer_resource_summary(path)

    def test_rss_integral_clips_phase_boundaries_and_preserves_unknown_pss(self):
        timeline = [dict(unix_ms=0, tree_rss_mib=100.), dict(unix_ms=2000, tree_rss_mib=200.)]
        self.assertAlmostEqual(rss_integral(timeline, 500, 1500), 150.)
        self.assertIsNone(rss_integral(timeline, 500, 1500, "tree_pss_mib"))
        with self.assertRaises(ValueError):
            rss_integral(timeline, 0, 2500)
        response, rows = self.idle()
        summary = resource_summary(self.resources(), response, rows, 37.1)
        self.assertEqual(summary["phases"]["idle"]["rss_mib_seconds_sampled"], 3200.)
        self.assertEqual(summary["rss_mib_seconds_sampled"], 3700.)
        self.assertIsNone(summary["phases"]["idle"]["pss_mib_seconds_sampled"])

    def test_missing_sampler_cpu_rss_timing_and_surviving_children_cannot_pass(self):
        response, rows = self.idle()
        for change in [lambda r: r.update(schema=2), lambda r: r.update(cpu_seconds_sampled=None),
                       lambda r: r.update(processes_exit_confirmed=False),
                       lambda r: r.update(alive_after_finish=[dict(pid=400)]),
                       lambda r: r.update(timeline_truncated=True),
                       lambda r: r["timeline"][10].update(tree_rss_mib=None),
                       lambda r: r["timeline"][10].update(cpu_seconds_sampled=None),
                       lambda r: r["timeline"][10].update(unix_ms=500),
                       lambda r: r["observed_processes"][0].update(create_time=None),
                       lambda r: r.update(timeline=r["timeline"][:20])]:
            bad = self.resources()
            change(bad)
            with self.assertRaises(ValueError):
                resource_summary(bad, response, rows, 37.1)

    def test_bounded_unobserved_idle_tail_is_reported_without_zero_extrapolation(self):
        response, rows = self.idle()
        resources = self.resources()
        resources["timeline"] = [row for row in resources["timeline"] if row["unix_ms"] < 36000]
        resources["timeline"].append(dict(resources["timeline"][-1],
            elapsed_seconds=35.8, unix_ms=35800, cpu_seconds_sampled=3.58))
        checked = resource_summary(resources, response, rows, 36.1)
        self.assertAlmostEqual(checked["phases"]["idle"]["covered_seconds"], 31.8)
        self.assertAlmostEqual(checked["phases"]["idle"]["uncovered_tail_seconds"], .2)
        self.assertAlmostEqual(checked["phases"]["idle"]["rss_mib_seconds_sampled"], 3180.)

    def test_comparison_retains_failed_arm_and_distinguishes_request_count_changes(self):
        rows = [dict(trial=trial, ttl_seconds=ttl, passed=True, pcm_sha256="same", audio_samples=32000,
                     hypothesis_units=["same"], source_signature=[["same"]],
                     inference=dict(tasks={task: dict(attempted_requests=2) for task in ("translation", "minutes")}))
                for trial, order in enumerate(ORDER, 1) for ttl in order]
        self.assertTrue(comparison_summary(rows)["comparable_for_per_request_reuse"])
        rows[-1]["inference"]["tasks"]["minutes"]["attempted_requests"] = 3
        result = comparison_summary(rows)
        self.assertFalse(result["comparable_for_per_request_reuse"])
        self.assertEqual(result["errors"], [])
        rows[-1]["passed"] = False
        self.assertTrue(comparison_summary(rows)["errors"])
        self.assertTrue(comparison_summary(rows[:1])["errors"])
        self.assertTrue(comparison_summary(list(reversed(rows)))["errors"])

    def test_owned_model_clone_success_and_copy_fallback_preserve_original_inode_metadata(self):
        for clone_ok in (True, False):
            with self.subTest(clone_ok=clone_ok), tempfile.TemporaryDirectory() as folder:
                source, target = pathlib.Path(folder) / "source", pathlib.Path(folder) / "owned"
                source.write_bytes(b"tiny public-model stand-in")
                before = file_identity(source)
                def copy_command(argv, **options):
                    self.assertEqual(argv[:2], ["/bin/cp", "-c"])
                    if clone_ok:
                        shutil.copy2(argv[-2], argv[-1])
                    return SimpleNamespace(returncode=0 if clone_ok else 1)
                with patch("benchmark_keep_alive.platform.system", return_value="Darwin"), \
                        patch("benchmark_keep_alive.subprocess.run", side_effect=copy_command):
                    result = clone_asset(source, target, before["bytes"], sha256(source))
                self.assertEqual(result["method"], "apfs_cp_clone" if clone_ok else "copy2")
                self.assertEqual(file_identity(source), before)
                self.assertNotEqual(file_identity(target)["inode"], before["inode"])
                self.assertEqual(sha256(target), sha256(source))

    @unittest.skipUnless(platform.system() == "Darwin", "APFS clone is a macOS preparation path")
    def test_real_tiny_model_clone_never_changes_source_ctime_or_inode(self):
        with tempfile.TemporaryDirectory() as folder:
            source, target = pathlib.Path(folder) / "source", pathlib.Path(folder) / "owned"
            source.write_bytes(b"no real weights or model execution")
            before = file_identity(source)
            clone_asset(source, target, before["bytes"], sha256(source))
            self.assertEqual(file_identity(source), before)
            self.assertNotEqual(file_identity(target)["inode"], before["inode"])
            self.assertEqual(sha256(target), sha256(source))


if __name__ == "__main__":
    unittest.main()
