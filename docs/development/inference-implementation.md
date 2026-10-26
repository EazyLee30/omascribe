# Core inference implementation status

Objective: implement the audio efficiency research plan, with optional hardware-specific acceleration components downloaded after local hardware detection. CUDA/HIP/other optional GPU dependencies must not enter the base installer.

Reference: `docs/research/audio-efficiency-2026-10-26/PLAN.zh-CN.md` and `REPORT.zh-CN.md`. Preserve unrelated private research and user data. No model/default promotion or release based only on compilation.

## Work and evidence

| Work | State | Required evidence |
| --- | --- | --- |
| P0 task traces and process/resource measurement | In progress | Bounded metadata-only traces; cold/warm baseline; missing measurements stay null |
| P0 evaluation datasets and metrics | Pending | Existing suites retained; mixed-language MER, domain anti-cases, long audio and speaker scoring |
| P1 worker leases and bounded keep-alive | In progress | Reuse, timeout, config/run isolation, cancellation, pressure and shutdown tests; real runtime comparison |
| P1 fair translation queue and cache validity | In progress | Oldest/newest reservation and expanded execution/source cache identity implemented and tested; token batching and long-passage handling pending |
| P1 incremental terminology | In progress | Revisioned SQLite queue, bounded 128-row reads, transactional acknowledgement and v5 migration tested; wider meeting workloads pending |
| P2 hotwords and stable text | Pending | SDK capability audit, accuracy/cost sweeps, quiet speech and partial/final regressions |
| P2 selective second-pass ASR and VAD experiments | Pending | Compatible adapters; local A/B quality and cost; original audio and human edits preserved |
| P2 incremental minutes and small-model comparison | Pending | Facts/negation/source/ownership regressions and total cost including repairs |
| P3 bounded diarization and stable identities | Pending | Global ID association; bounded memory; DER, unknown coverage and manual anchors |
| G1 local hardware and runtime detection | In progress | Isolated native DXGI / Linux PCI probe, multi-vendor Linux fixtures; native Windows/Linux execution pending |
| G1 optional acceleration component delivery | In progress | Signed/hash-checked manifests, isolated atomic downloads, CUDA/Vulkan build/package workflow and base inventory gate implemented; actual CI packages/catalog publication pending |
| G1 per-task backend verification and CPU fallback | In progress | Local CPU/GPU fact/negation/offload/timing gate and one CPU retry implemented; real CPU gate passed; GPU/driver/OOM scenarios pending |
| G2 HIP, ONNX GPU, Apple comparisons | Pending | Exact compatibility matrix and real-device evidence; unverified paths remain experimental |
| Updater separation and delta experiment | Pending | Model/component reuse, interrupted update/signature/rollback/full-package fallback |

## Completion rule

The full objective remains active until implementation and the plan's quality/resource/platform gates have been audited. Compilation or fixture-only GPU detection is not proof of real hardware acceleration. Record missing hardware evidence explicitly while continuing independent implementation work.

## Evidence recorded 2026-10-26

- `cargo clippy --workspace --all-targets --locked -- -D warnings` and `npm run build` passed on the local Apple M5. Python packaging/release tests: 14 passed. Core suite: 80 passed, 3 opt-in tests ignored (the real CPU gate was run explicitly and passed separately).
- Real Qwen3-1.7B Q4_K_M CPU gate: two English/Chinese synthetic extraction requests preserved Thursday/Friday, budget 3000 and not-approved. 1,838 ms startup, 6,921 / 8,252 ms request time, 17,013 ms total; sampled worker peak RSS 2,486 MiB. `artifacts/benchmarks/acceleration-cpu-gate.json` is ignored and local. GPU memory unavailable, recorded as null. This is not an ASR/minutes quality benchmark or a Windows GPU result.
- Previous four-request M5 worker comparison: 3 starts / 1 reuse; reused job had 0 ms startup, 52/53 cached prompt tokens and 169 ms request time. `artifacts/benchmarks/worker-reuse-m5.json`; limited synthetic evidence only.
- User can test Windows 10 / GTX 1650 later. No real Windows NVIDIA, Windows AMD or Linux GPU performance/driver results yet.
- Hardware/model/component/config identity binds the local acceleration certificate. Unknown driver versions bind to one app session. Linux kernel drivers without a module version bind to kernel release and boot ID. Explicit CPU is preserved. A changed configuration requires another local test.
- Automatic GPU selection remains opt-in through **Test & enable**, requires auto backend and a faster, correct local test. No global default promotion has been made. HIP/ASR GPU remain pending experiments.
- Draft PR #1 / commit `a3951a51f89f14aedb801906071f0b55f90fbf2c`: Windows core test, clippy and native hardware probe steps passed in CI. The GPU component jobs are separate; Vulkan SDK layout failures are under repair and CUDA builds are still pending. This does not establish real GPU acceleration.
- A clean-source CPU repeat on that commit passed: 1,437 ms startup, 6,311 / 7,961 ms requests, 15,710 ms inference total and sampled peak RSS 2,502 MiB. The benchmark artifact now includes source, executable and model hashes; hashing/report overhead is excluded from inference timing. The earlier 17,013 ms sample is retained here as evidence of ordinary run variability.

### Incremental terminology and translation cache follow-up

- Core regression suite: 85 passed, 3 opt-in tests ignored; workspace clippy passed. Added a persistent revision queue instead of scanning all previous passages on every final ASR event. Reads are bounded to 128 records, acknowledgement cannot remove newer work, and the v5 migration backfills existing current-run passages. Human text remains protected. Disabling a term or hiding/restoring a linked note reevaluates automatic corrections against the archived original.
- Translation cache keys now include meeting/run, complete source revision/text and speaker bindings, target, model catalog hashes/file metadata, runtime binary hash, optional component binding, prompt/schema and execution configuration/permissions. Older cache rows recompute once. Runtime hashes use a bounded metadata cache; large model weights are not rehashed by each UI snapshot. Deleted meetings clear these derived caches transactionally while preserving recoverable source/notes.
- Real installed Qwen3-1.7B translation harness passed two invented Chinese/English passages (negation and weekday), unchanged originals and four display modes. Repeating the same request added no worker trace. This is a narrow regression check, not overall ASR/translation throughput evidence.
- `tests/benchmark_terminology.py` compares two actual CLI binaries in disposable, network-disabled workspaces: 5,000 unchanged fictional passages plus eight appended passages, two trials in reversed order. Baseline is `615861de409d1a5eaac43c6fec2729f72a792159`; candidate source/executable hashes and compile-time build identity are recorded in ignored `artifacts/benchmarks/terminology-queue-full-cost.json`. Candidate was built from the working changes before their commit.
- Baseline append-correction medians: 65.02 / 65.04 ms; candidate: 6.72 / 6.50 ms. Initial correction increased from 69.36 / 65.56 to 105.19 / 103.44 ms; seeding 5,000 passages increased from 19.78 / 17.68 to 99.41 / 99.51 ms due to queue maintenance. Including workspace setup, all source writes, initial correction and eight updates, totals were 678.32 / 659.88 ms versus 314.26 / 305.09 ms. Result validation/hashing are excluded; CPU/RSS unavailable. No model runs, so this does not measure ASR or LLM speed.
- CLI `build-info` reports the compiled source digest, revision at compilation, compiler, target and profile before opening any user workspace. Its source digest is authoritative for uncommitted builds; revision at compilation alone is insufficient. Smoke-tested with a data path that is an ordinary file, confirming no Engine/workspace opening.
