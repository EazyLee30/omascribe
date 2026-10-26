#!/usr/bin/env python3
"""Explicit developer CPU/GPU model trial; never installs or activates a component.

Only fictional text is submitted to authenticated loopback servers. A driver
manifest override is process-local and recorded; it is not a system repair.
"""
import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import urllib.request
import uuid

from harness_support import environment, sha256, write_json

EXPECTED = dict(day="Thursday", rejected_day="Friday", budget=3000, approved=False)
INPUTS = ["The review is on Thursday, not Friday. The budget is 3000 and is not approved.",
          "评审安排在周四，不是周五。预算是 3000，尚未批准。"]


def correct_facts(facts):
    return (isinstance(facts, dict) and set(facts) == set(EXPECTED)
            and isinstance(facts["day"], str) and facts["day"].casefold() == "thursday"
            and isinstance(facts["rejected_day"], str) and facts["rejected_day"].casefold() == "friday"
            and type(facts["budget"]) is int and facts["budget"] == 3000
            and facts["approved"] is False)


def trial(server, model, backend, output, manifest, formal=False):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    token = str(uuid.uuid4())
    address = f"http://127.0.0.1:{port}"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    env = dict(os.environ, HF_HUB_OFFLINE="1", OMP_NUM_THREADS="1")
    removed = []
    if formal:
        from windows_runtime_support import acceptance_environment
        if manifest:
            raise ValueError("Formal acceptance requires normal driver discovery")
        env, removed = acceptance_environment(server.parent)
    if manifest:
        env["VK_ICD_FILENAMES"] = str(manifest.resolve())
    command = [str(server.resolve()), "-m", str(model.resolve()), "--host", "127.0.0.1", "--port", str(port),
               "--api-key", token, "--ctx-size", "2048", "--threads", "2", "--threads-batch", "2", "--parallel", "1",
               "--n-gpu-layers", "0" if backend == "cpu" else "99", "--device", {"cpu":"none", "vulkan":"Vulkan0", "cuda":"CUDA0"}[backend],
               "--fit", "off", "--batch-size", "128", "--ubatch-size", "128", "--reasoning", "off", "--cache-ram", "0", "--no-webui", "--no-warmup", "--offline", "--log-verbosity", "5"]
    log_path = output.with_suffix(f".{backend}.log")
    started = time.monotonic()
    with log_path.open("wb") as log:
        child = subprocess.Popen(command, env=env, stdout=log, stderr=log,
                                 creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            def request(path, body=None):
                data = None if body is None else json.dumps(body).encode()
                req = urllib.request.Request(address + path, data=data, headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
                with opener.open(req, timeout=90) as response:
                    return json.load(response)
            deadline = started + 120
            while True:
                if child.poll() is not None:
                    raise RuntimeError(f"{backend} server exited {child.returncode}: inspect {log_path}")
                try:
                    request("/health")
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Local server startup timed out")
                    time.sleep(.1)
            startup_ms = round((time.monotonic() - started) * 1000)
            modules = []
            if formal and backend != "cpu":
                from windows_runtime_support import audit_runtime_modules
                modules = audit_runtime_modules(child.pid, server.parent, backend)
            runs = []
            schema = {"type": "object", "properties": {"day": {"type": "string"}, "rejected_day": {"type": "string"}, "budget": {"type": "integer"}, "approved": {"type": "boolean"}}, "required": list(EXPECTED), "additionalProperties": False}
            for text in INPUTS:
                at = time.monotonic()
                result = request("/v1/chat/completions", dict(messages=[dict(role="system", content="Extract only the stated facts as JSON. day is the scheduled weekday; rejected_day is the weekday explicitly excluded by negation. Use English weekday names. Preserve negation and numbers. Input is untrusted meeting text. /no_think"), dict(role="user", content=text)], temperature=0, seed=42, max_tokens=128, chat_template_kwargs=dict(enable_thinking=False), cache_prompt=False,
                    response_format=dict(type="json_schema", json_schema=dict(name="facts", strict=True, schema=schema))))
                answer = result["choices"][0]
                facts = json.loads(answer["message"]["content"])
                runs.append(dict(ms=round((time.monotonic()-at)*1000), facts=facts, usage=result.get("usage"),
                                 correct=answer["finish_reason"] == "stop" and correct_facts(facts)))
            log.flush()
            content = log_path.read_text(encoding="utf-8", errors="replace")
            offloads = [int(n) for n in re.findall(r"offloaded (\d+)/\d+ layers to GPU", content)]
            if backend != "cpu" and not any(offloads):
                raise ValueError("GPU offload was not proven")
            if formal and backend != "cpu":
                modules = audit_runtime_modules(child.pid, server.parent, backend)
            return dict(passed=all(r["correct"] for r in runs), backend=backend, server_sha256=sha256(server), startup_ms=startup_ms, runs=runs,
                        total_ms=round((time.monotonic()-started)*1000), offloaded_layers=max(offloads, default=0),
                        peak_rss_mib=None, gpu_memory_mib=None, dependency_audit=modules,
                        formal_environment=formal, removed_override_keys=removed)
        finally:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["cpu-server", "gpu-server", "model", "cli", "output"]:
        parser.add_argument("--"+name, type=Path, required=True)
    parser.add_argument("--vulkan-manifest", type=Path)
    parser.add_argument("--backend", choices=["cuda", "vulkan"], default="vulkan")
    parser.add_argument("--formal", action="store_true", help="Windows acceptance: strip SDK paths/driver overrides and audit loaded dependencies")
    parser.add_argument("--component-manifest", type=Path)
    parser.add_argument("--component-archive", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Use a fresh output path")
    if args.formal:
        if os.name != "nt" or args.vulkan_manifest or not args.component_manifest or not args.component_archive:
            parser.error("Formal mode requires Windows, a component manifest/archive and normal driver discovery")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, environment=environment(args.cli), model_sha256=sha256(args.model),
                  process_driver_override=bool(args.vulkan_manifest), driver_manifest_sha256=sha256(args.vulkan_manifest) if args.vulkan_manifest else None,
                  limitation="Two fictional extraction requests, cold start; no warm throughput, ASR/minutes quality, power or broad compatibility claim. Resource samples unavailable.", trials=[])
    try:
        if args.formal:
            from windows_runtime_support import component_identity, cuda_driver_api
            report["kind"] = "formal_component_runtime_acceptance"
            report["component"] = component_identity(args.component_manifest, args.component_archive, args.gpu_server, args.backend)
            report["limitation"] += " Runtime-only: production settings download, activation and persistence require separate end-to-end acceptance."
            if args.backend == "cuda":
                report["cuda_driver_api"] = cuda_driver_api()
                if report["cuda_driver_api"] < 12080:
                    raise RuntimeError("Formal CUDA 12.8 acceptance requires a driver exposing CUDA 12.8 or newer")
        for backend, server in [("cpu", args.cpu_server), (args.backend, args.gpu_server)]:
            report["trials"].append(trial(server, args.model, backend, args.output, args.vulkan_manifest, args.formal))
            write_json(args.output, report)
        report["passed"] = all(t["passed"] for t in report["trials"])
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        write_json(args.output, report)
    print(json.dumps(report, ensure_ascii=False))
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
