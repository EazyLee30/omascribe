# Windows formal component acceptance

Updated 2026-10-27. This follows the requested order. A prepared test is not a
passed gate. The CPU base installer and the existing four-component/signature
pipeline remain the release design.

## 1. Integrate shared source and regress native behavior

Current application checkpoint: `44c6f21` merges shared `70a2d45`, preserving both
candidate channel selection and Windows's 404 state handling in a resolved
`components.rs` conflict. New signature fixtures retain exact bytes; the
publication fixture and workflow parser now handle Windows line endings without
weakening signature checks. Clean-source rebuilt CLI and native app: 19/19
real-model minutes, 100 core / 5 opt-in ignored, 55 Python and UI-test clippy pass.
The native main/companion/transcript language fixture also passes in candidate
selector mode. Source/CLI/app hashes and actual results:
[`candidate integration`](../evaluation/windows-candidate-integration-2026-10-27.json).

Shared sync `54e4be4` merges `4edafb0` and keeps the shared bounded-log signing
helper as the canonical implementation. It replaces the parallel Windows helper
`5d0f0b1`; that earlier checkpoint's 57 tests remain historical evidence. Current
Python has 56 passing tests. All four successful original native checkouts must
agree, and actual checkout/original head/current publishing source must have
matching native-input fingerprints; mutable run PR head/base values are not
used as trigger evidence. Original historical inputs can be read at their fixed
commit if a merge ref disappears. These delivery-only changes do not alter
application core/frontend/model or native SDK/package inputs from `44c6f21`, so
its 19-case/native/core results remain bound to the same application bytes.
Two completed Vulkan checkout trees and the now-completed Linux CUDA checkout
have also been independently observed to match original source `164371e`.
Windows CUDA and the original full catalog have now completed successfully.
The current shared delivery diagnostic reader is merged through `1f553c2`;
62 Python tests pass locally. First cloud signing attempt `40387149779` failed
before downloading/signing/publishing because its original checkout log reader
could not recognize the response. The same all-four reuse preflight passes
locally. Signature/source gates have not been bypassed.

The official run-log archive fallback from shared `0294f03` is now merged and
verified against all four real job logs. Unified signing run `40390695525` has
since succeeded and published the immutable **prerelease candidate**
`acceleration-ci-40367160891`; its tag still names build source `164371e`.
The downloaded public index matches the original payload byte for byte and
passes the application's normal embedded-key/version verifier. Stable
`acceleration-v1` was not promoted. Candidate publication is not a Settings or
recording acceptance result.

Earlier support checkpoint: `be5e16d` merges shared `e255dc7`, including the
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
That checkpoint's evidence is in
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
Linux CUDA has now completed build/package/upload, and both Linux archives have
been downloaded and checked against their outer ZIP digest, archive digest and
every member. Its original checkout agrees with the two Vulkan checkouts. These
are Linux artifact-integrity checks on Windows, not Linux GPU execution. Exact
three-component inventory and source proofs:
[`available candidate artifacts`](../evaluation/acceleration-candidate-integrity-2026-10-27.json).
Windows CUDA has now passed its seven-file integrity check and formal runtime
trial on NVIDIA 621.01: 29 layers offloaded, both bilingual facts/negations
correct, CUDA redistributables loaded from the component and nvcuda from the
system driver. SDK paths and overrides were excluded. Two fixed requests took
3688 ms including fresh-process startup, versus CPU 9594 ms; OS caches were
not controlled and this is not a broad meeting-performance claim. Exact proof:
[`formal CUDA runtime`](../evaluation/windows-cuda-formal-2026-10-27.json).
All four archives (56 members) and the unsigned original catalog now match;
normal-key signed publication and actual Settings acceptance remain pending.

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

The stable `acceleration-v1` catalog is unpublished. The isolated signed
candidate selector now supplies the real download prerequisite without changing
that stable pointer. Actual Settings download/activation results are still
pending execution. Do not add an unsigned-directory shortcut or change the
production trust root to satisfy QA.

The isolated native fixture `tests/native-windows-gpu.js` is prepared for real
Settings download/test, restart, Use CPU, memory-admission and owned-worker-fault
phases. It uses the existing `ui-test` hook, a signed candidate and fictional
transcript seed; actual backend/child cleanup must also be audited by its runner.
It has only passed a syntax check so far and is not end-to-end evidence yet.

Windows also exercised two 92-second ASR-only/CPU-concurrent synthetic pairs,
before and after shared ASR fixed-200ms decoding. Both concurrent arms showed
live translation/minutes, complete matching PCM and exact citations. Both pairs
remain **failed** under the existing 2500 ms gate: ASR-only startup peaks were
3264/3136 ms; after 10 seconds maximum lag was 384/456 ms. The new worker's two
ASR text-unit outputs match. No threshold was relaxed and these CPU observations
do not establish GPU coexistence or physical microphone acceptance. Worker
hashes, static-CRT build/tests and setup limitations are retained in
[`Windows replay diagnostics`](../evaluation/windows-recording-contention-2026-10-27.json).

A unified signed candidate can be tested without updating the global pointer.
The shared selector now accepts the fixed repository version through
`OMASCRIBE_ACCELERATION_CANDIDATE_VERSION=acceleration-ci-40367160891` in an
isolated test workspace. It uses the real Settings fetch/download path and a
separate version-specific cache, preserving the normal release key, exact
selected version, runtime revision and archive/member verification. A stable
cache cannot satisfy a candidate request. Do not seed installation markers.
Candidate creation/signing uses the shared pipeline's one-shot reuse label only
after the original complete matrix and current application/Windows gates pass;
Windows does not publish a second installer/channel. See
[`candidate staging`](acceleration-candidates.md).

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
