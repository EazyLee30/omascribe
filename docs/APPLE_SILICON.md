# Apple Silicon inference and measured performance

Implemented for 0.3.0. Physical-device validation: Apple M5 / 24 GiB / macOS 27.0.1 on 2026-10-24. Release artifacts are separately built and checked by the Native release workflow. MLX remains an optional experimental backend.

## Delivered paths

- **Automatic / Metal:** Apple Silicon native packaging and developer setup build the pinned llama.cpp revision with Metal, Accelerate and an embedded Metal library. Existing Qwen GGUF models are reused. A runtime build manifest distinguishes these binaries from old CPU-only installations.
- **CPU:** retained on all desktop platforms. A legacy runtime without a Metal build manifest remains a CPU runtime; explicitly selecting an unavailable backend produces an actionable error.
- **MLX (experimental):** a separate, explicitly installed developer runtime supports the catalog's pinned `qwen3-1.7b-mlx-4bit` directory. It is **not bundled in the installer**. There is no hidden pip install or runtime download when starting a meeting or generating notes.

Model download, inference runtime setup, and inference are distinct operations. The application model manager requires the device's model-download permission, checks file size and SHA256, stages incomplete files, and only exposes a complete verified model directory. The developer command below downloads Python packages and is a deliberate setup operation:

```sh
python3 scripts/setup-mlx.py --runtime-dir "/path/to/OmaScribe/runtime"
```

MLX package versions are pinned at the top level and installed package versions are recorded locally. Transitive wheels, Python distribution, relocatability, signing and a packaged Python runtime still need a release qualification process; the development environment is not a reproducible distributable MLX installer.

## Resource behavior

Presets are starting points, not measured recommendations for every machine:

| Preset | LLM | ASR | Threads | Context | Automatic LLM budget |
| --- | --- | --- | ---: | ---: | --- |
| Eco | Qwen3 0.6B GGUF | bundled small Zipformer | 1 | 2048 | total RAM / 4, clamped to 1–2 GiB |
| Balanced | Qwen3 1.7B GGUF | bundled small Zipformer | 2 | 2048 | total RAM / 3, clamped to 1.5–4 GiB |
| Quality | Qwen3 4B GGUF | bilingual Zipformer | 4 | 4096 | total RAM / 2, clamped to 2–8 GiB |

The automatic hardware recommendation chooses Balanced at 8 GiB or above and Eco below that; it does not infer fast performance from a chip name. Users can override the budget from 1024 to 32768 MiB. Model/context admission checks run before loading. During generation, a watchdog samples process RSS and system memory headroom every 500 ms and stops the task if RSS exceeds the budget or available memory falls below 512 MiB. Quality mode waits until recording has ended. Sustained transcription lag cancels note generation, and every task unloads its inference process on completion, cancellation or failure.

These controls are **not an OS-enforced memory limit**, a GPU allocation limit, or a power cap. RSS does not account for all unified/GPU memory. MLX's memory/cache limits are allocator guidance; its wired-memory behavior and native allocations are additional reasons to keep the external watchdog. Actual power draw, battery life and concurrent long-meeting load have not been benchmarked here.

## Offline inference boundary

llama.cpp receives only a local model path, `--offline`, an authenticated ephemeral loopback endpoint, one worker, bounded context and generation length. The core HTTP client disables environment proxies and redirects. Build scripts disable curl/OpenSSL network support. No imported profile can supply a command, model path, model URL or network permission.

The MLX adapter uses the fixed managed model directory and `local_files_only=True`, `trust_remote_code=False`, `HF_HUB_OFFLINE=1`, `TRANSFORMERS_OFFLINE=1`; telemetry and implicit Hub tokens are disabled. It exposes only health, tokenization and one chat-completion route on `127.0.0.1`, requires a per-task secret, rejects browser Origins, bounds request size, and times out incomplete requests. Python audit checks reject external connect/sendto/sendmsg/name-resolution calls. This is defense in depth for Python APIs, **not an OS network sandbox for native dependencies**.

MLX currently prompts for the output schema instead of constrained decoding. The same Rust parsing, source checks and evidence-guided repair apply. Failed output is not committed. A synthetic three-passage meeting with two named speakers successfully completed generation, validation and persistence with both network permissions disabled. An earlier fragmented transcript failed format/source validation and remained uncommitted. The successful MLX output also used stronger wording than the source in places; a systematic factuality/attribution evaluation remains necessary. MLX is therefore experimental and not the default recommendation.

## Reproducing the CPU / Metal benchmark

```sh
python3 scripts/benchmark-local.py \
  --server /path/to/llama-server \
  --model /path/to/Qwen3-1.7B-Q4_K_M.gguf \
  --output docs/benchmarks/my-machine.json
```

This script uses a synthetic fixed prompt and never reads meeting data. The same Metal-capable binary is run with 0 and 99 requested GPU layers. The report records the actual layer-offload evidence, model/server hashes, pinned runtime revision, application commit and dirty state, hardware, context, threads, sample counts, warmup policy, prompt hash, actual input/output tokens, TTFT, timing and sampled RSS. It refuses to publish a result if the log does not prove the requested backend was used.

The [M5 report](benchmarks/apple-m5-24gb-2026-10-24.json) contains three measured runs per backend, each with 155 input tokens and 128 forced output tokens, Q4_K_M, context 2048 and two threads:

| Backend | Median generation tokens/s | Median first-token latency | Maximum sampled RSS |
| --- | ---: | ---: | ---: |
| CPU, 0/29 layers offloaded | 13.52 | 2213 ms | 2426 MiB |
| Metal, 29/29 layers offloaded | 78.32 | 102 ms | 1431 MiB |

These are warm-model short generation measurements. They exclude application retrieval, document validation, ASR and complete-meeting latency; the forced length is not an accuracy test. RSS is not total memory consumption. Only this M5 configuration was measured; no M1/M2/M3/M4/M5 Pro/Max, Windows or Linux speeds are extrapolated. MLX smoke-test generation counters are not published as an equivalent benchmark because the prompt, schema and generation lengths differ.

## Primary references

- [llama.cpp build documentation](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md)
- [llama-bench method and limitations](https://github.com/ggml-org/llama.cpp/blob/master/tools/llama-bench/README.md)
- [MLX memory-limit semantics](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.set_memory_limit.html), [cache limit](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.set_cache_limit.html), [wired limit](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.set_wired_limit.html)
- [MLX-LM loading implementation](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/utils.py), [generation implementation](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/generate.py)
- [Hugging Face offline environment variables](https://huggingface.co/docs/huggingface_hub/package_reference/environment_variables)
- [Pinned MLX model source](https://huggingface.co/mlx-community/Qwen3-1.7B-4bit/tree/3b1b1768f8f8cf8351c712464f906e86c2b8269e)
