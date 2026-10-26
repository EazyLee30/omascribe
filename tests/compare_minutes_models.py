#!/usr/bin/env python3
"""Paired offline Qwen 0.6B/1.7B minutes regressions, including rejected attempts.

Assets must already be installed. This script never downloads, switches product
defaults, or runs two models concurrently. Its pattern checks are targeted gates,
not proof of general meeting comprehension or a production fallback policy.
"""
import argparse
import json
import math
import os
import pathlib
import platform
import re
import subprocess
import sys
import time

from harness_support import REPO, environment, read_json, sha256, write_json

MODELS = ("qwen3-1.7b", "qwen3-0.6b")
PREFERENCES = dict(inference_backend="cpu", threads=1, context_size=2048, resource_mode="balanced",
                   memory_budget_mib=4096, llm_keep_alive_seconds=0, auto_notes=False, translation_mode="off",
                   diarization=False, use_memory=False, use_knowledge=False)


def measured_sum(values):
    return sum(values) if all(isinstance(v, (int, float)) and math.isfinite(v) and v >= 0 for v in values) else None


def summarize(report):
    runs = [run for case in report["cases"] for run in case["runs"]]
    attempts = [attempt for run in runs for attempt in (run.get("generation") or {}).get("attempts", [])]
    processes = [run.get("process") or {} for run in runs]
    resources = [process.get("resources") or {} for process in processes]
    sampled_cpu = [r.get("cpu_seconds_sampled") for r in resources]
    peaks = [r.get("peak_tree_rss_mib") for r in resources]
    pss = [r.get("peak_tree_pss_mib") for r in resources]
    usage = [attempt.get("usage") or {} for attempt in attempts]
    timings = [u.get("timings") or {} for u in usage]
    return dict(cases=len(report["cases"]), passed_cases=sum(c["passed"] is True for c in report["cases"]),
                case_seconds=measured_sum([c["seconds"] for c in report["cases"]]),
                generation_commands=len(runs), process_seconds=measured_sum([p.get("seconds") for p in processes]),
                attempts=len(attempts), repairs=sum(a.get("attempt", 1) > 1 for a in attempts),
                rejected_attempts=sum(bool(a.get("validation_error") or a.get("failure")) for a in attempts),
                attempt_seconds=measured_sum([a.get("elapsed_ms") / 1000 if a.get("elapsed_ms") is not None else None for a in attempts]),
                input_tokens=measured_sum([u.get("prompt_tokens") for u in usage]),
                output_tokens=measured_sum([u.get("completion_tokens") for u in usage]),
                prompt_ms=measured_sum([t.get("prompt_ms") for t in timings]),
                decode_ms=measured_sum([t.get("predicted_ms") for t in timings]),
                sampled_cpu_seconds=measured_sum(sampled_cpu),
                sampled_cpu_known_seconds=sum(v for v in sampled_cpu if v is not None),
                commands_missing_cpu=sum(v is None for v in sampled_cpu),
                peak_tree_rss_mib=max((v for v in peaks if v is not None), default=None),
                commands_missing_rss=sum(v is None for v in peaks),
                peak_tree_pss_mib=max((v for v in pss if v is not None), default=None),
                commands_missing_pss=sum(v is None for v in pss), gpu_memory_mib=None, energy_joules=None)


def validate_pair(results, expected_cases, reference_environment=None, variants=None):
    assert expected_cases and len(set(expected_cases)) == len(expected_cases)
    variants = variants or [(model, "cpu") for model in MODELS]
    assert len(results) == 2 and {(r["model"], r.get("backend", "cpu")) for r in results} == set(variants)
    first = results[0]["report"]
    reference_environment = reference_environment or first["environment"]
    for result in results:
        report = result["report"]
        assert report["schema"] == 3
        assert report["fixture_sha256"] == first["fixture_sha256"]
        assert report["fixture_cases"] == expected_cases
        assert [case["case"] for case in report["cases"]] == expected_cases
        for key in ["cli_sha256", "cli_build", "core_and_harness_source_sha256"]:
            assert report["environment"][key] == reference_environment[key]
        backend = result.get("backend", "cpu")
        expected = {**PREFERENCES, "llm_model": result["model"], "inference_backend": backend}
        assert report["preferences"] == expected
        assert report["passed"] == all(case["passed"] is True for case in report["cases"])
        assert (result["exit_code"] == 0) == report["passed"]
        for case in report["cases"]:
            for run in case.get("runs", []):
                generation = run.get("generation")
                if generation:
                    assert generation.get("model") == result["model"]
                    if not (generation.get("reason") == "non_substantive_only" and generation.get("attempt") == 0):
                        assert generation.get("backend", "cpu") == backend


def backend_evidence(report, artifacts, backend):
    """Bind each model command's actual layer offload to its saved diagnostics.

    A non-substantive shortcut has no model call. Missing diagnostics on an
    attempted generation cannot count as proof, even if its final text passes.
    No prompt text or runtime log contents are copied into the summary.
    """
    evidence, errors = [], []
    for case in report["cases"]:
        for run in case["runs"]:
            generation = run.get("generation") or {}
            if generation.get("reason") == "non_substantive_only" and generation.get("attempt") == 0:
                continue
            path = artifacts / case["case"] / f'phase-{run["phase"]}' / f'batch-{run["batch"]}' / "llm-runtime.log"
            log = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
            layers = [tuple(map(int, pair)) for pair in re.findall(r"offloaded (\d+)/(\d+) layers to GPU", log)]
            positive = any(0 < used <= total for used, total in layers)
            entry = dict(case=case["case"], phase=run["phase"], batch=run["batch"],
                         requested_backend=backend, actual_backend=generation.get("backend"),
                         offloaded_layers=layers, positive_offload=positive,
                         log_sha256=sha256(path) if path.is_file() else None)
            evidence.append(entry)
            if generation.get("backend") != backend or not log:
                errors.append(f'{case["case"]}: missing {backend} generation/log evidence')
            if (backend == "metal") != positive:
                errors.append(f'{case["case"]}: unexpected GPU offload for {backend}')
    if not evidence:
        errors.append("No actual model commands; a shortcut cannot admit a backend")
    return dict(commands=evidence, errors=errors, passed=not errors)


def runtime_inventory(runtime):
    return {str(path.relative_to(runtime / "llama")): dict(bytes=path.stat().st_size, sha256=sha256(path))
            for path in sorted((runtime / "llama").rglob("*")) if path.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli")
    parser.add_argument("--model-store", type=pathlib.Path, required=True)
    parser.add_argument("--runtime", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--trials", type=int, choices=[1, 2], default=2)
    parser.add_argument("--case", help="One diagnostic case only; never admits a model")
    parser.add_argument("--apple-backends", action="store_true", help="Compare CPU/Metal using the same 1.7B weights on Apple Silicon")
    args = parser.parse_args()
    if args.apple_backends and (platform.system() != "Darwin" or platform.machine() != "arm64"):
        parser.error("Apple backend comparison requires native arm64 macOS")
    variants = [("qwen3-1.7b", backend) for backend in ["cpu", "metal"]] if args.apple_backends else [(model, "cpu") for model in MODELS]
    output, cli = args.output.resolve(), args.cli.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists():
        parser.error("Use fresh output and artifact paths")
    artifacts.mkdir(parents=True)
    cases = read_json(REPO / "tests/fixtures/document-cases.json")
    if args.case:
        cases = [case for case in cases if case["name"] == args.case]
    if not cases:
        parser.error("Unknown case")
    for case in cases:
        assert not set(case.get("preferences", {})) & set(PREFERENCES | {"llm_model": ""}), "Fixture overrides comparison settings"
    catalog = read_json(REPO / "model-catalog.json")["models"]
    assets = {}
    for model in dict.fromkeys(model for model, _ in variants):
        spec = next(item for item in catalog if item["id"] == model)
        files = []
        for item in spec["files"]:
            path = args.model_store.resolve() / model / item["name"]
            assert path.stat().st_size == item["bytes"] and sha256(path) == item["sha256"], f"Model mismatch: {model}"
            files.append({key: item[key] for key in ["name", "sha256", "bytes", "url"]})
        assets[model] = files
    runtime = args.runtime.resolve()
    runtime_files = runtime_inventory(runtime)
    assert runtime_files, "No local llama runtime"
    report = dict(schema=1, kind="paired-apple-backends" if args.apple_backends else "paired-minutes-models", environment=environment(cli), assets=assets,
                  runtime_files=runtime_files, expected_cases=[case["name"] for case in cases],
                  variants=variants, runtime_log_verbosity=4 if args.apple_backends else None,
                  trials=[], errors=[], comparison_complete=False, passed=False, default_promotion=False,
                  limitation="Targeted fictional pattern/citation/coverage/human-edit checks. Each generation command opens a CLI; worker retention across commands is not measured. CPU is sampled lower-bound, RSS may double-count shared pages; PSS/GPU/energy may be unknown. Includes all attempts and failed cases. No automatic model promotion or cross-model fallback simulation.")
    if args.apple_backends:
        listing = subprocess.run([str(runtime / "llama/llama-server"), "--list-devices"],
                                 capture_output=True, text=True, timeout=15, check=True)
        assert "MTL" in listing.stdout, "The pinned runtime exposes no Metal device"
        report["apple_host"] = dict(chip=subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
                                    physical_memory_bytes=int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True)),
                                    runtime_device_listing=listing.stdout,
                                    note="Runtime working-set/free estimates are unified-memory figures, not independent VRAM or system available memory.")
        report["backend_policy"] = "Untuned production balanced policy: CPU batch/ubatch 64, Metal 128; both one thread, same context and weights. This is not an equal-batch kernel microbenchmark. Case time includes startup; separate startup is not measured."
    for trial in range(args.trials):
        results = []
        order = variants if trial == 0 else list(reversed(variants))
        for model, backend in order:
            folder = artifacts / f"{trial+1}-{model}-{backend}"
            folder.mkdir()
            preferences = folder / "preferences.json"
            write_json(preferences, {**PREFERENCES, "llm_model": model, "inference_backend": backend})
            destination = folder / "evaluation.json"
            argv = [sys.executable, str(REPO / "tests/evaluate.py"), "--cli", str(cli), "--preferences", str(preferences), "--output", str(destination)]
            if args.case:
                argv += ["--case", args.case]
            env = dict(os.environ, OMASCRIBE_TEST_MODEL_STORE=str(args.model_store.resolve()),
                       OMASCRIBE_MODELS_DIR=str(folder / "no-bundled-models"), OMASCRIBE_RUNTIME_DIR=str(runtime),
                       OMASCRIBE_RETRIEVAL_CACHE="0")
            if args.apple_backends:
                # Diagnostics only, with fictional input. The pinned runtime
                # hides actual layer offload at its default verbosity of 3.
                env["LLAMA_ARG_LOG_VERBOSITY"] = "4"
            print(f"Trial {trial+1} {model}/{backend}: {len(cases)} cases", flush=True)
            started = time.monotonic()
            # evaluate.py records each failed case and continues. Nonzero exit is
            # quality evidence, not grounds to discard its expensive failed work.
            process = subprocess.run(argv, env=env, check=False)
            result = dict(model=model, backend=backend, exit_code=process.returncode, wall_seconds=time.monotonic()-started,
                          report=read_json(destination))
            if result["report"]:
                result["summary"] = summarize(result["report"])
                if args.apple_backends:
                    result["backend_evidence"] = backend_evidence(result["report"], destination.with_suffix(".artifacts"), backend)
            results.append(result)
            write_json(output, {**report, "in_progress": dict(trial=trial+1, results=results)})
        try:
            validate_pair(results, report["expected_cases"], report["environment"], variants)
            if args.apple_backends:
                assert all(r["backend_evidence"]["passed"] for r in results), "Backend evidence missing/mismatched"
            report["trials"].append(dict(trial=trial+1, results=results, comparable=True))
        except (AssertionError, KeyError, TypeError) as error:
            report["errors"].append(f"Trial {trial+1} is not comparable: {type(error).__name__}: {error}")
            report["trials"].append(dict(trial=trial+1, results=results, comparable=False))
        write_json(output, report)
    try:
        assert runtime_inventory(runtime) == runtime_files, "Runtime changed during comparison"
        assert sha256(cli) == report["environment"]["cli_sha256"], "CLI changed during comparison"
        final_environment = environment(cli)
        for key in ["core_and_harness_source_sha256", "model_catalog_sha256", "models_lock_sha256"]:
            assert final_environment[key] == report["environment"][key], "Source changed during comparison"
        for model, files in assets.items():
            for item in files:
                path = args.model_store.resolve() / model / item["name"]
                assert path.stat().st_size == item["bytes"] and sha256(path) == item["sha256"], "Model changed during comparison"
    except (AssertionError, OSError) as error:
        report["errors"].append(f"Asset revalidation: {error}")
    report["comparison_complete"] = not report["errors"] and len(report["trials"]) == args.trials
    report["passed"] = report["comparison_complete"] and all(r["report"]["passed"] for t in report["trials"] for r in t["results"])
    write_json(output, report)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
