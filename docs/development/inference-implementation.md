# Core inference implementation status

Objective: implement the audio efficiency research plan, with optional hardware-specific acceleration components downloaded after local hardware detection. CUDA/HIP/other optional GPU dependencies must not enter the base installer.

Reference: `docs/research/audio-efficiency-2026-10-26/PLAN.zh-CN.md` and `REPORT.zh-CN.md`. Preserve unrelated private research and user data. No model/default promotion or release based only on compilation.

## Work and evidence

| Work | State | Required evidence |
| --- | --- | --- |
| P0 task traces and process/resource measurement | In progress | Bounded metadata-only traces; cold/warm baseline; missing measurements stay null |
| P0 evaluation datasets and metrics | Pending | Existing suites retained; mixed-language MER, domain anti-cases, long audio and speaker scoring |
| P1 worker leases and bounded keep-alive | In progress | Reuse, timeout, config/run isolation, cancellation, pressure and shutdown tests; real runtime comparison |
| P1 fair translation queue and cache validity | In progress | Oldest/newest reservation implemented and tested; token batching and expanded cache identity still pending |
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
