# Windows formal component acceptance

Updated 2026-10-27. This follows the requested order. A prepared test is not a
passed gate. The CPU base installer and the existing four-component/signature
pipeline remain the release design.

## 1. Integrate shared source and regress native behavior

Current support checkpoint: `be5e16d` merges shared `e255dc7`, including the
public ASR fixture preparation/offline reuse change. Python now has 43 passing
tests. The full 19-case rerun passed, with clean inference sources; core 99 / 5
opt-in ignored, ASR 3, default-app and UI-test clippy also pass. The reused CLI's
compile identity remains `164371e` and its core source hash is unchanged; the
report separately records current suite revision `be5e16d`. This merge does not change the
production core, frontend, model inputs or GPU component build inputs compared
with candidate source `164371e`; it does not require another component build.

At `164371e`, shared `ac6cdfa` had already brought the repository layout and Metal
fallback work. The Windows icon path and HTML/Markdown LF attributes were
corrected. Rebuilt frontend and Windows application, clean-source 19/19 real-model
minutes, 99 core tests / 5 opt-in ignored, 42 Python tests and native UI-test
feature clippy pass. Earlier checkpoints below are retained as history.

Merged PR #1 `8dbdf3b` into Windows commit `5cd3bae`, preserving both Metal and
Windows harness tests. Production core/frontend source is unchanged from the
previous Windows application source `911d786`. The newly compiled CLI identifies
`5cd3bae` and the same core source hash. Clean-source real-model minutes: 19/19
passed. Rust core: 95 passed / 5 opt-in ignored. Python: 42 passed after adding
the formal-environment tools. Source CI for the merge is green.

The earlier native UI attempt was blocked. Automatic approval rejected the
acceptance-app launch command containing a WebView2 debug port with only
`blocked by policy`. Existing-window accessibility reads work, but WGC screenshot
capture returned `FrameArrived timed out`; input geometry is unavailable and
keyboard results could not be reliably verified. The user confirmed an unlocked,
visible desktop. These tool failures were resolved for functional acceptance by
using the repository's existing native `ui-test` startup hook and fixed DOM
probes, without a debugging port. Open native main/companion/transcript windows
passed five language changes and expansion checks; detection did not activate
GPU inference. The probes are compiled out of production. User-supplied app
screenshots are normal; automated desktop screenshot capture remains a separate
tool limitation. See the earlier
[`step1 evidence`](../evaluation/windows-acceptance-2026-10-27-step1.json).
Current evidence is in
[`layout acceptance`](../evaluation/windows-acceptance-2026-10-27-layout.json).

## 2. Accept the formal native payloads

The previous NVIDIA 522.06 driver could not be used for formal CUDA 12.8 acceptance.
NVIDIA's official GTX 1650 notebook / Windows 10 lookup returned 621.01 WHQL.
The installer is downloaded, its size/SHA-256 recorded, and its NVIDIA
Authenticode signature verified. The user approved launch and handled the
installation. The installer summary reports a failure while all selected rows
report installed; its aggregate failure reason has not been established. The
system driver is now 621.01, device problem code is zero, SetupAPI records the
display-driver operation as SUCCESS, and CUDA initialization/device enumeration
succeed. No restart was performed by the agent. See
[`driver evidence`](../evaluation/windows-driver-preparation-2026-10-27.json).

The Windows Vulkan artifact from full-matrix run `40367160891` has now passed
normal driver discovery, 29/29 layer offload and both fictional fact/negation
requests. Its archive and every member are hash-checked, SDK/driver override
paths excluded, and the system Vulkan loader observed in the child. This is
runtime-only acceptance, not signed-settings activation. Exact results and limits:
[`formal Vulkan runtime`](../evaluation/windows-vulkan-formal-2026-10-27.json).
The two CUDA build jobs are still pending; no CUDA 12.8 result is claimed yet.

Use candidate artifacts from the existing Windows/Linux × CUDA/Vulkan matrix.
Retain the workflow run, exact source SHA, CUDA 12.8.1 build log/toolchain version,
immutable component version, archive SHA-256 and every unpacked file hash. The
old CUDA 11.5 developer build and the old process-local Vulkan ICD override are
not substitutes. A fresh normal-discovery test must clear driver overrides.

`tests/benchmark_windows_gpu.py --formal` requires the actual component metadata
and archive. It verifies the archive, exact unpacked members, pinned runtime
revision and platform/backend before execution. For CUDA it requires a driver
exposing CUDA 12.8 or newer for this acceptance lane; that stricter test condition
does not change the application's existing CUDA 12 minor-compatibility policy.

The formal child environment reconstructs PATH from the component and Windows
system directories, removes SDK/LLAMA/GGML/Vulkan override variables, and audits
loaded DLLs at readiness and after inference. Observed CUDA redistributables
must come from the component; the NVIDIA driver/system Vulkan loader must come
from the Windows driver installation. This machine already has a development
toolkit; report it as excluded from the tested process, not uninstalled or absent.
A separate clean-machine run remains useful corroboration.

Example after the driver and current candidate artifact are ready:

```powershell
python tests/benchmark_windows_gpu.py --formal --backend cuda `
  --cpu-server <CPU-runtime/llama-server.exe> `
  --gpu-server <verified-component/llama-server.exe> `
  --component-manifest <cuda-windows-x86_64.component.json> `
  --component-archive <candidate.tar.bz2> `
  --model <pinned-Qwen3-1.7B-Q4_K_M.gguf> `
  --cli target/debug/omascribe-cli.exe --output <fresh-result.json>
```

Use `--backend vulkan` for normal driver discovery, with no ICD override. These
two fictional fact/negation requests are runtime/dependency acceptance only;
they do not establish full-meeting quality, activation or a universal speedup.

## 3. Exercise the actual settings lifecycle

Pending the preceding gates and a signed candidate catalog:

| Case | Required observation |
| --- | --- |
| Download/install | Settings fetches a catalog verified with the existing release key; the immutable archive is downloaded, checked and atomically installed |
| Test/enable | Correct bilingual facts, positive offload and the existing CPU/GPU comparison gate; a slower candidate stays on CPU |
| Application restart | The installed component and hardware/model/runtime-bound certificate are revalidated; the next generation proves the effective backend |
| Switch to CPU | The GPU worker is reaped, selection/certificate is cleared and subsequent generation uses CPU |
| Insufficient VRAM | Admission rejects insufficient memory without losing pending source; separately record any real allocation-failure fallback test |
| Worker failure | Kill only the owned inference child; verify certificate invalidation, one CPU retry, preserved source/citations and no retry loop |
| Recording plus generation | Record ASR backlog and continuity, generation/fallback status, cancellation and child cleanup; label synthetic PCM injection separately from physical microphone capture |

The current `acceleration-v1` catalog endpoint returns 404. Real download and
activation are not passed while that prerequisite is missing. Do not add an
unsigned-directory shortcut or change the production trust root to satisfy QA.

A unified signed candidate can be tested without updating the global pointer:
provide its signed `index.json` and immutable candidate release/archive URLs,
then place that envelope in an isolated acceptance workspace's existing
`acceleration-index.json` cache. The production verifier still checks the normal
release key, version, runtime revision and asset URLs on every use. A 404 pointer
refresh retains a valid cached candidate. Candidate creation/staging belongs to
the shared pipeline owner; Windows must not publish a second installer/channel.

## 4. Hand off the evidence

Submit the accepted application/source revision, native runtime revision,
workflow/toolchain/driver versions, full archive/member/model hashes and each
case's actual result and limits. Keep blocked, simulated and failed cases visible.
The unified pipeline requires all four components, checks their exact bytes,
signs the directory and updates the shared catalog only after release gates.
Uploading a standalone Windows executable does not satisfy that contract.

## Native functional test without a debugger port

Build the existing `ui-test` feature with a separate application identifier and
protocol scheme, and use a fresh data/report directory:

```powershell
$env:TAURI_CONFIG='{"identifier":"io.omarchy.omascribe.nativeqa","plugins":{"deep-link":{"desktop":{"schemes":["omascribe-nativeqa"]}}}}'
cargo +1.99.0 build -p omascribe --features ui-test --locked
$env:OMASCRIBE_DATA_DIR='<fresh-acceptance-data-directory>'
$env:OMASCRIBE_MODELS_DIR='<prepared-model-directory>'
$env:OMASCRIBE_RUNTIME_DIR='<CPU-runtime-directory>'
$env:OMASCRIBE_UI_TEST_SCRIPT=(Resolve-Path tests/native-windows-acceptance.js).Path
$env:OMASCRIBE_UI_TEST_REPORT='<absolute-new-report.json>'
& target/debug/omascribe.exe
```

The native app exercises its real frontend/IPC/windows, writes a result and exits.
The fixture supports physical Escape cancellation. Fixed probes inspect only
localization/control state and can expand the companion; they do not expose an
arbitrary script-evaluation endpoint. Reports from the first fixture run retain
a test-navigation timing failure; the fixture now waits for the settings page
after a language re-render. Do not distribute the UI-test binary as a release.
