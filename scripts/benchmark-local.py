#!/usr/bin/env python3
"""Measure a local llama.cpp server with synthetic text; no meeting data or outbound requests.

Example: python3 scripts/benchmark-local.py --server /path/llama-server \
  --model /path/Qwen3-1.7B-Q4_K_M.gguf --output docs/benchmarks/my-machine.json
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import re
import socket
import statistics
import subprocess
import threading
import time
import urllib.request
import uuid

PROMPT = """Summarize the following fictional project meeting. Keep names and numbers exact.
The team reviewed Project Cedar, a synthetic benchmark with no personal information.
The prototype budget was 37000 test credits and was revised to 42000 test credits.
Mina will run the accessibility review on October 30. The localization owner is still unknown.
The launch date is not decided. The team will first review the test results.
Write concise meeting notes in Chinese. Describe decisions, action items, and unresolved questions.
会议记录：这里只使用人工编造的公开测试文本，不读取任何用户会议、笔记或录音。
输出时区分已经决定与尚待确认的事项，不能把可能负责写成确定负责。
Summary:
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-id", default="qwen3-1.7b")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--backends", nargs="+", choices=("cpu", "metal"), default=["cpu", "metal"])
    args = parser.parse_args()
    if not 1 <= args.repetitions <= 10:
        raise SystemExit("Use 1–10 repetitions")
    with args.model.open("rb") as f:
        model_hash = hashlib.file_digest(f, "sha256").hexdigest()
    with args.server.open("rb") as f:
        server_hash = hashlib.file_digest(f, "sha256").hexdigest()
    repo = Path(__file__).resolve().parents[1]
    revision = json.loads((repo / "models.lock.json").read_text())["llama_cpp"]["revision"]
    def sysctl(key):
        return subprocess.check_output(["sysctl", "-n", key], text=True).strip()
    report = {
        "schema": 1, "id": args.output.stem, "measured_at": datetime.now(timezone.utc).isoformat(),
        "hardware": {"chip": sysctl("machdep.cpu.brand_string"), "memory_gib": int(sysctl("hw.memsize")) / 2**30,
                     "os": "macOS " + platform.mac_ver()[0], "arch": platform.machine()},
        "method": {"kind": "llama-server-completion", "prompt_sha256": hashlib.sha256(PROMPT.encode()).hexdigest(),
                   "server_sha256": server_hash, "llama_cpp_revision": revision,
                   "app_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(),
                   "app_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True)),
                   "generation_tokens_requested": 128, "repetitions": args.repetitions,
                   "cache": "prompt cache disabled; model warm after one unmeasured 16-token request",
                   "rss_sampling_interval_ms": 100, "synthetic_input": True},
        "runs": [], "summary": [], "backend_evidence": [],
        "limitations": ["Single machine; not a prediction for other M-series chips.",
                        "Generation speed is not transcription accuracy or meeting-summary quality.",
                        "RSS is a sampled process-resident-memory peak, not total unified/GPU allocation.",
                        "Power draw and thermal throttling were not measured.",
                        "128 output tokens are forced for comparability; this is a short synthetic benchmark."],
    }
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for backend in args.backends:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]
        token = str(uuid.uuid4()); base = f"http://127.0.0.1:{port}"
        command = [str(args.server.resolve()), "-m", str(args.model.resolve()), "--host", "127.0.0.1", "--port", str(port),
                   "--api-key", token, "--ctx-size", "2048", "--threads", "2", "--threads-batch", "2", "--parallel", "1",
                   "--n-gpu-layers", "99" if backend == "metal" else "0", "--batch-size", "128", "--ubatch-size", "128",
                   "--prio", "-1", "--poll", "0", "--reasoning", "off", "--cache-ram", "0", "--no-webui", "--no-warmup",
                   "--offline", "--log-verbosity", "5"]
        log_path = args.output.with_suffix(f".{backend}.log")
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with log_path.open("wb") as log:
            launched = time.perf_counter(); child = subprocess.Popen(command, stdout=log, stderr=log)
            try:
                def request(path, data=None):
                    req = urllib.request.Request(base + path, data=None if data is None else json.dumps(data).encode(),
                                                 headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
                    return opener.open(req, timeout=120)
                deadline = time.monotonic() + 120
                while True:
                    if child.poll() is not None: raise RuntimeError(f"Server exited; inspect {log_path}")
                    try:
                        with request("/health") as r: json.load(r)
                        break
                    except OSError:
                        if time.monotonic() > deadline: raise RuntimeError("Server startup timed out")
                        time.sleep(0.1)
                startup_ms = (time.perf_counter() - launched) * 1000
                with request("/completion", {"prompt": "Warmup.", "n_predict": 16, "temperature": 0, "cache_prompt": False}) as r:
                    json.load(r)
                for iteration in range(args.repetitions):
                    stopped = threading.Event(); peaks = []
                    def sample():
                        while not stopped.is_set():
                            value = subprocess.run(["ps", "-o", "rss=", "-p", str(child.pid)], capture_output=True, text=True)
                            if value.stdout.strip(): peaks.append(int(value.stdout.strip()) / 1024)
                            stopped.wait(0.1)
                    monitor = threading.Thread(target=sample); monitor.start()
                    started = time.perf_counter(); first = None; final = None
                    try:
                        with request("/completion", {"prompt": PROMPT, "n_predict": 128, "temperature": 0, "seed": 42,
                                                    "cache_prompt": False, "ignore_eos": True, "stream": True}) as response:
                            for line in response:
                                if not line.startswith(b"data: "): continue
                                chunk = json.loads(line[6:])
                                if chunk.get("content") and first is None: first = time.perf_counter()
                                if chunk.get("stop"): final = chunk
                    finally:
                        elapsed_ms = (time.perf_counter() - started) * 1000
                        stopped.set(); monitor.join()
                    if final is None or first is None: raise RuntimeError("Incomplete generation")
                    timings = final["timings"]
                    report["runs"].append({"backend": backend, "model_id": args.model_id, "model_sha256": model_hash,
                        "quantization": "Q4_K_M", "context_size": 2048, "threads": 2, "batch_size": 128, "ubatch_size": 128,
                        "repeat": iteration + 1, "prompt_tokens": timings["prompt_n"], "generated_tokens": timings["predicted_n"],
                        "prompt_tps": timings["prompt_per_second"], "generation_tps": timings["predicted_per_second"],
                        "ttft_ms": (first - started) * 1000, "elapsed_ms": elapsed_ms, "peak_rss_mib": max(peaks, default=0),
                        "model_startup_ms": startup_ms})
                    print(backend, iteration + 1, round(timings["predicted_per_second"], 2), "tokens/s", flush=True)
            finally:
                child.terminate()
                try: child.wait(timeout=10)
                except subprocess.TimeoutExpired: child.kill(); child.wait()
        # Require actual offload evidence, not a requested flag. Keep concise,
        # path-free evidence alongside the measurements; raw logs remain local.
        offloads = re.findall(r"offloaded ([0-9]+)/([0-9]+) layers to GPU", log_path.read_text())
        if not offloads or (backend == "metal") != (int(offloads[-1][0]) > 0):
            raise RuntimeError("Backend offload evidence does not match; refusing to publish results")
        report["backend_evidence"].append({"backend": backend, "gpu_layers": int(offloads[-1][0]),
            "total_layers": int(offloads[-1][1]), "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest()})
        rows = [r for r in report["runs"] if r["backend"] == backend]
        report["summary"].append({"backend": backend, "model_id": args.model_id,
            "median_generation_tps": statistics.median(r["generation_tps"] for r in rows),
            "median_ttft_ms": statistics.median(r["ttft_ms"] for r in rows),
            "max_peak_rss_mib": max(r["peak_rss_mib"] for r in rows)})
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
