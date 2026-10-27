# Windows development and GPU validation

Windows work is maintained in `codex/windows-gpu-adaptation`, stacked on PR #1
(`codex/inference-acceleration`). Before each push, fetch both upstream branches,
review new commits and integrate the shared branch when needed. Never force-push
over the parallel Mac work. Shared worker/component mechanisms remain in PR #1;
this branch owns Windows acceptance and the reported UI/terminology regressions.

## 2026-10-26 local evidence

- Windows 10 x64, GTX 1650 / 4096 MiB, NVIDIA driver 522.06, CUDA toolkit 11.5.
  Rust 1.99.0 installed to match CI. Node 22.13.1; test Python 3.12.14.
- Frontend TypeScript/Vite build passed. Core regression suite: 89 passed,
  3 explicit hardware/model tests ignored. Python suite: 21 passed after fixing
  Windows subprocess timeout normalization. Native UI acceptance passed in real
  WebView2 windows: EN / simplified / traditional language changes reached both
  the open companion and floating transcript; expanded state was retained.
- Existing CUDA components use CUDA 12.8.1 and cannot run on this installed
  CUDA-11-compatible driver. Hardware probing now queries the system CUDA driver
  API without loading a model or requiring a development toolkit. CUDA choices
  are disabled for a known incompatible driver; the execution test also checks.
- CUDA packaging previously emitted PTX only for compute 7.5/8.0/9.0. The build
  now requests native machine code for those GPUs so they need not JIT PTX from
  the newer SDK. This source fix still requires a fresh SDK component build.
- Downloaded Windows Vulkan CI artifact from run `40304633065`. Its archive and
  all 11 members matched the recorded hashes. Default `--list-devices` found no
  GPU: Vulkan loader 1.3.204 reported missing registry ICD manifests. No activation
  was performed. A developer-only process override to the installed 522.06
  NVIDIA DriverStore manifest lists `Vulkan0: NVIDIA GeForce GTX 1650` with
  3581 MiB free. This does not fix system discovery or validate model inference.
- The actual component fails model loading with `0xc0000409` in `llama.dll`,
  including a CPU-only control using that same component. `--fit off` did not
  prevent the crash. The base installer CPU runtime loads the same pinned model
  successfully. Windows component builds now use a self-contained static runtime
  instead of static-CRT objects crossing server/common/llama DLL boundaries;
  a fresh targeted Vulkan build is required to validate this correction.
- The local admission test returned semantically correct lowercase weekday names.
  Weekday case is now normalized while incorrect weekdays, amount and approval
  remain rejected. The prompt defines the scheduled/excluded weekday fields.
- Actual Qwen3-1.7B CPU evaluation of the new `zh_windows_gpu_keyword_lines`
  fixture passed: OmaScribe, gtx1650, nvidia and the unfinished optimization status
  were retained. This is a bounded synthetic regression, not general ASR accuracy.
- Signed `acceleration-v1` is not yet published. The UI reports that state and
  keeps CPU selected; CI artifacts are not treated as signed production downloads.

## Reported regressions

Local verification after merging Mac commit `bca8671`: Windows desktop and
static-CRT ASR worker compile; strict clippy passed for all three workspace
packages (ASR uses its own static-CRT target cache). Core: 95 passed / 5 opt-in
ignored; ASR: 3 passed; Python: 36 passed. The real
CPU admission gate passed both English/Chinese fact-and-negation requests. The
native three-language UI check passed again against the rebuilt app and worker.
Two seconds of synthetic silence completed without invented segments in both
legacy and opt-in stream-hotword modes; both loaded all three note terms with
no fallback. This confirms native model startup and decoder configuration, not
speech accuracy. The new desktop app lives in ignored `artifacts/windows-app/`
and uses a separate development workspace; the installed application is untouched.

The language preference must update already-open companion and transcript
windows, including static controls, ARIA labels and translation options, while
retaining expanded state. `tests/windows-ui.mjs` checks the real native windows
through an explicitly enabled development WebView2 CDP endpoint. Use a separate
Tauri identifier and `OMASCRIBE_DATA_DIR`; never attach it to a personal workspace.

Standalone lowercase keyword lines now supply ASR spelling hints. Writer hints
contain only names supported by recognizable transcript text. A draft that drops
one of these names must repair before committing. Notes alone do not prove that
an unintelligible fragment such as “M二” names a particular product. Raw ASR
history and human edits continue to be preserved.

Ordinary Latin words collected from mixed-language notes remain ASR hints but
are not mandatory literal names in minutes. Full-corpus testing exposed the
distinction in `bilingual_scope`: `offline transcription` must be allowed to
translate. A language-only repair now translates the existing draft against the
original transcript instead of recomposing it and losing qualifiers. The repaired
fixture retains offline transcription, Friday, microphone-only scope and deferred
system audio. Factual repairs still rebuild from evidence, and every translation
passes the same name, fact and source checks before committing.
The final clean-source Qwen3-1.7B Windows run passed all 19 fixtures, including
language repairs, note-only names, long context, corrections and human-paragraph
protection. The functional run and exact input/build hashes are recorded in
[`windows-minutes-2026-10-26.json`](../evaluation/windows-minutes-2026-10-26.json).
It overlapped UI checks/compilation and is not a performance benchmark.

## Reproduce on Windows

Install VS 2022 C++ Build Tools, Node 22, Python 3.11+ and Rust 1.99.0.
`scripts/dev-windows.ps1 -Python <python.exe> -Run` builds the desktop app and
the pinned static-CRT ASR worker with a separate app identifier and data folder.
It reuses installed `OmaScribe/native` models/runtime read-only; `-Assets` selects
another prepared asset folder. `-SkipAsr` reuses an installed worker for UI work.
The script does not install a GPU toolkit or enable a component.

The `build-windows-vulkan` label compiles only the Windows Vulkan component.
Mac commit `12e9441` supplied the shared workflow. The Windows account's workflow
permission was subsequently restored and the branches were merged using the Mac
version. This targeted path cannot assemble or publish a partial catalog. The existing
`build-acceleration-components` label and manual release dispatch retain the full
matrix and every signing gate. Remove the targeted label after the trial to keep
ordinary pushes on source checks.

`scripts/build-windows-gpu.ps1 -Backend cuda -CudaToolkit D:\CUDA
-CudaArchitectures 75-real -LegacyCudaHost` reproduces the current local GTX 1650
experiment using the installed CUDA 11.5.119 and MSVC 19.44. It builds the pinned
source statically with FlashAttention off and records a developer-only manifest.
The explicit legacy-host switch permits an SDK/compiler pairing outside NVIDIA's
supported host matrix; this is not a formal SDK/component compatibility claim.
No component is installed or activated. A matching Vulkan SDK can be selected
with `-Backend vulkan -VulkanSdk <folder>`.

The ignored `live_developer_gpu_model_gate` test accepts
`OMASCRIBE_TEST_GPU_RUNTIME=<runtime bin folder>` and
`OMASCRIBE_TEST_GPU_BACKEND=cuda|vulkan`. It runs the production worker's resource
and bilingual fact/negation gate, requires proven offload, and saves hashes and
the result in ignored artifacts. Production component signature/activation rules
are unchanged.

The local CUDA build now loads the pinned Qwen3-1.7B model and proves 29/29 layers
offloaded on the GTX 1650. Both fictional English/Chinese extraction requests
preserve weekdays, amount and negation. One ordered CPU/CUDA pair at two threads
took 14,328/5,735 ms including startup. This small cold-start sample does not
establish general throughput or meeting quality. The production worker's own
one-thread admission trial also passed (5,565 ms total, sampled child RSS 2,093
MiB, runtime GPU buffers 1,300.43 MiB). The reported buffers are not physical peak
VRAM. Exact hashes, configuration and limits are in
[`windows-gpu-2026-10-26.json`](../evaluation/windows-gpu-2026-10-26.json).
Repeating the one-thread admission on clean source took 9,670 ms, so the initial
two-thread ordered pair must not be treated as a stable speedup estimate.

The pinned runtime hides model offload metadata at its default verbosity 3;
optional GPU workers now request verbosity 4 so the admission test can prove
offload. Verbosity 5 logs request contents and is not enabled in production.

Two targeted static Vulkan CI builds compiled successfully, then failed
`--version` with `0xc0000135` on their GPU-less runners. The copy-only development
SDK did not supply a runtime loader. The build probe now uses the separately
published, SHA-256 pinned LunarG runtime ZIP in a build-only directory, selects
only its x64 loader, and adds it to that process's PATH. It never installs a
system runtime or copies the loader into the downloadable component. Run
[`40345201430`](https://github.com/EazyLee30/omascribe/actions/runs/40345201430)
now passes Windows source checks, static compilation, version verification and
component packaging; its catalog correctly skips. Direct CLI downloads hit
TLS/EOF errors; the GitHub connector subsequently retrieved the artifact. Its
archive and all three members passed checksum verification. The static candidate
now loads Qwen3-1.7B, offloads 29/29 layers and passes the production worker's
English/Chinese fact/negation trial without the old `0xc0000409` failure. This
trial uses the explicitly recorded process-local NVIDIA ICD manifest override:
the system's default loader still lists no devices. No system driver or registry
was changed, and this does not establish default-environment compatibility.
The trial took 13,315 ms including startup, with sampled child RSS 1,480 MiB and
runtime GPU buffers 1,287.93 MiB; these are not a throughput comparison or peak
physical VRAM. The build label was removed to avoid redundant SDK builds on
subsequent source pushes. Signed production activation remains a separate gate.

A diagnostic with one local Microsoft Zira synthesized English sentence still
misrecognized OmaScribe and NVIDIA, both without terms and with the default 1.5
hotword score. Uppercasing vocabulary did not resolve it. This narrow failure
is retained in ignored `artifacts/asr-tts/`; no general speech-accuracy or default
score promotion is claimed from this synthetic voice.

## References

- [NVIDIA CUDA 12.8 driver compatibility](https://docs.nvidia.com/cuda/archive/12.8.0/cuda-toolkit-release-notes/index.html)
- [CMake real/virtual CUDA architecture selection](https://cmake.org/cmake/help/latest/prop_tgt/CUDA_ARCHITECTURES.html)
- [Khronos Windows driver discovery](https://github.com/KhronosGroup/Vulkan-Loader/blob/main/docs/LoaderDriverInterface.md#driver-discovery-on-windows)
- [LunarG runtime archive hashes and non-installer description](https://vulkan.lunarg.com/sdk/files.json)
