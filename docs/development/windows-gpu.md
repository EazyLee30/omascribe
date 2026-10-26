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

Final local verification after merging Mac commit `9c7c4b4`: Windows desktop and
static-CRT ASR worker compile; strict clippy passed for all three workspace
packages (ASR uses its own static-CRT target cache). Core: 94 passed / 4 opt-in
ignored; ASR: 3 passed; Python: 24 passed. The real
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

## Reproduce on Windows

Install VS 2022 C++ Build Tools, Node 22, Python 3.11+ and Rust 1.99.0.
`scripts/dev-windows.ps1 -Python <python.exe> -Run` builds the desktop app and
the pinned static-CRT ASR worker with a separate app identifier and data folder.
It reuses installed `OmaScribe/native` models/runtime read-only; `-Assets` selects
another prepared asset folder. `-SkipAsr` reuses an installed worker for UI work.
The script does not install a GPU toolkit or enable a component.

The proposed `build-windows-vulkan` label compiles only the Windows Vulkan
component. Its workflow patch is posted in PR #2 for the Mac side to apply:
the Windows OAuth credential lacks the GitHub `workflow` scope. The targeted
path cannot assemble or publish a partial catalog. The existing
`build-acceleration-components` label and manual release dispatch retain the full
matrix and every signing gate. Remove the targeted label after the trial to keep
ordinary pushes on source checks.

## References

- [NVIDIA CUDA 12.8 driver compatibility](https://docs.nvidia.com/cuda/archive/12.8.0/cuda-toolkit-release-notes/index.html)
- [CMake real/virtual CUDA architecture selection](https://cmake.org/cmake/help/latest/prop_tgt/CUDA_ARCHITECTURES.html)
- [Khronos Windows driver discovery](https://github.com/KhronosGroup/Vulkan-Loader/blob/main/docs/LoaderDriverInterface.md#driver-discovery-on-windows)
