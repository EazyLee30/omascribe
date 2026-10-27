# Windows formal component acceptance

Updated 2026-10-27. Actual Settings download, test-enable, restart, Use CPU and
an owned CUDA process fault followed by exactly one CPU retry have Windows
evidence. Corrected paced-file replay now passes the original backlog gate;
automatic post-stop completion and release quality remain unaccepted. CPU stays in the base installer;
CUDA/Vulkan use the existing four-component build and signed catalog pipeline.

## Replay pacing correction and current CUDA evidence

Frozen application `83d3287`, shared base `9fb00e5`, changes only paced-file
replay in `audio.rs`: `-re -readrate_initial_burst 0.001` prevents FFmpeg's initial
read burst from advancing the supplied audio clock. Physical microphone capture,
ordinary file import, ASR thread policy and the2500ms admission gate are unchanged.
See [FFmpeg's input pacing options](https://www.ffmpeg.org/ffmpeg-all.html#Advanced-options).
QA requires FFmpeg with `readrate_initial_burst`; this run uses private FFmpeg9.0.2,
without installing it globally or adding it to the app package.

The PCM/timing integration test actually ran with that FFmpeg: core129 passed,
6 opt-in ignored, and core all-targets clippy passed. Exact-head Linux40423110492
and Optional40423110503 passed; prepare/Windows-core succeeded, SDK build/catalog
were skipped. Six empty-audio Release ASR processes showed approximately1.33–1.43s
median readiness across1/2/4 threads. Preverification warmed model pages; this
diagnostic supports no production thread-policy or broad speed claim.

Two reversed-order15s ASR-only pairs preserve all PCM bytes and identical ASR
units. Old CLI3286081 leads wall time by494/521ms with peak backlogs2880/2112ms;
new CLI83d3287 leads by19/18ms with1792ms backlog in both arms. Both new arms pass
the unchanged2500ms gate. OS caches, frequency and thermal state were uncontrolled.

The normal signed-candidate CPU/GPU test enabled CUDA again. A single92s fictional
Zira replay with live translation/minutes preserves the exact expected PCM,
observes14 CUDA worker events and passes the original backlog gate at1984ms
(p95:448ms; p50:232ms). CUDA DLLs come from the installed component and nvcuda
from Windows, with SDK PATH/overrides excluded. This is operational acceptance
of the synthetic replay, not a paired old/new cost study or microphone test.

At stop, minutes cover8/16 segments and translations15/16. Explicit Translate
finishes16/16. One explicit Generate commits through13/16, failing the harness's
single-call full-cursor assertion; a second bounded Generate reaches16/16 with
unchanged source and exact citations. Keep that first failure. This is **not**
unattended post-stop completion or semantic summary acceptance. The original
failed92s CUDA/CPU measurements below retain their source identities and are not
retroactively passed. No release approval, new SDK build or stable catalog change.

Exact CLI/ASR/FFmpeg/model hashes, all four short arms, initial diagnostics,
normal activation, complete CUDA module audits, metadata traces and both
completion attempts: [replay pacing evidence](../evaluation/windows-replay-pacing-2026-10-27.json).

## Approval-negation checkpoint

Application `3286081` merges shared `2461aeb` (including `c0293a2`). The facts
protocol is exposed only under `cfg(test)` and is not connected to the product
generator. Its native-model probe remains ignored. The runtime change recognizes
the explicit English uncertainty cues `not approved` and `unapproved`.

Core122 passed /6 ignored, Python63, core-test/native UI-test all-targets clippy,
rebuilt native three-window language checks and exact-head code CI pass. Two
real1.7B CPU examples on the production legacy path preserve3000 and approval
negation with both experimental switches off.

**Full-summary quality is still false.** Manual review of the `not approved`
example found unsupported assertions that the meeting concluded, no further
action was taken, no other topics were discussed and no original data changed.
The source states only that the budget of3000 is not approved, with a Budget note.
The narrow regex check passing does not validate those extra assertions. There
was no pre-change control for this sample, so this observation does not attribute
the boilerplate to `c0293a2`.

Exact output, source/binary/native identities and scope:
[approval-negation evidence](../evaluation/windows-approval-negation-2026-10-27.json).
No product fact ledger, full22-case pass, GPU retest or release approval is
inferred from these targeted checks. Earlier results below retain their source
identities and the experiments remain default-off.

## Frozen incremental experiment: rejected

Windows `b0ab6cb` merged shared `8cf7a8f`. The fixture EOF conflict was resolved
by asserting all18 common objects identical and retaining Windows's extra case
plus the3 shared additions:22 cases. Notes language repair and the Windows
default.exe evaluator path were retained. Shared docs-only rejection `25b8828`
was merged after the frozen measurements; no SDK/compiler inputs changed.

The complete22-case real automatic run with **both catch-up and incremental
flags explicitly enabled failed:18/22**. Four failures remain visible:

- `zh_phased_correction`: phase2 did not settle within120s; validation rejected
  omission of an explicit state and prior content remained.
- `zh_long_context_state`: committed prose omitted original recording retention,
  manual editing, noise and boundary requirements.
- `en_incremental_separate_topics`: new5000 appeared, while old3000 was still
  described in the present tense in the same paragraph.
- `en_incremental_completion`: phase2 did not settle; a persisted job outcome
  records the unfinished-state guard, followed by interruption at the budget.

Trace proves actual experiment use:22 full and8 delta prompt events, with30 CPU
worker events. This is a single Windows functional run, not a paired cost study.
The same binary then passed `zh_phased_correction` with incremental disabled and
catch-up still enabled; this targeted control does not establish a full22-case
default-path pass. Core112/5 ignored, Python63, UI-test all-targets clippy, rebuilt
native three-window language checks and exact-head code CI passed independently.

Exact source/binary/model/case identities, outcomes, task traces and the control:
[Windows incremental regression](../evaluation/windows-incremental-regression-2026-10-27.json).
Each automatic phase retains its last draft/repair diagnostic and all metadata
trace events; earlier job bodies may have been overwritten. Do not infer complete
raw-draft retention or cost totals from that last diagnostic.

The shared [complete paired result](../evaluation/incremental-article-2026-10-27.json)
also rejects promotion: legacy20/21, incremental17/21, higher incremental cost,
and an existing legacy stale-budget problem. Both experimental flags remain off
by default. No0.3.4/stable publication is approved. Older19/19 evidence below
retains its historical source and must not be transferred to this new experiment.

## Source and regressions

Frozen regression checkpoint `301e914` cleanly merges shared PR #1 `116469e`, including opt-in minutes
catch-up and persisted-take Metal retry scope. Independent Windows fix `39486ea`
restores GPU status before the first inference job: the certificate survived
restart, but an empty in-memory status concealed it until inference selected the
backend. Restoration uses the existing signed component/model/runtime/hardware
validation. The native query performs blocking validation off the UI thread.

At `301e914`: core105 passed /5 opt-in ignored, Python62 passed, UI-test all-targets
clippy passed. Rebuilt native main/companion/transcript windows pass five
English/simplified/traditional language changes, preserve expansion and show
hardware without activation. The complete automatic-scheduler19-case real-model
run passed with catch-up explicitly enabled; this does not change its production
default. Exact source/binary/model hashes, all19 cases and native-window results:
[shared116 regression](../evaluation/windows-shared116-regression-2026-10-27.json).
Earlier clean-source explicit-generation19/19 is
retained in [candidate integration](../evaluation/windows-candidate-integration-2026-10-27.json);
retained separately from that new scheduling-path result.

Subsequent `32bb449` serializes first status restoration with configuration
decisions and rechecks state inside the gate. A controlled interleaving regression
covers newer testing/disable decisions. Recording startup reads only testing
state while already holding the gate; catalog queries avoid reverse lock order
and release the cache-write gate before returning status. Core106/5 ignored and
both default/UI-test all-targets clippy pass. Rebuilt native signed-catalog check,
test-enable, cold restart with actual CUDA minutes, Use CPU and short capture
lock-order smoke pass. Shared docs-only `54daf12` is also merged.

Functional native QA uses the existing `ui-test` startup hook and fixed DOM
probes. User screenshots are normal; an earlier WGC automation timeout is a
separate tool limitation. No debugger port is used. Do not distribute UI-test
binaries as installers.

## Formal components and signed candidate

The full Windows/Linux × CUDA/Vulkan [matrix40367160891](https://github.com/EazyLee30/omascribe/actions/runs/40367160891)
completed at declared source `164371e`. All four jobs actually checked out
`523f8753`, whose full tree matches that source. All four archives and56 members
were verified. Linux evidence here proves integrity, not execution. The canonical
shared helper verifies original checkout evidence and native-input fingerprints
before reusing these immutable bytes.

Unified [signing40390695525](https://github.com/EazyLee30/omascribe/actions/runs/40390695525)
published the [prerelease candidate](https://github.com/EazyLee30/omascribe/releases/tag/acceleration-ci-40367160891).
Its tag still names `164371e`. The public index passes the normal embedded-key
verifier and matches the original unsigned payload byte for byte. Stable
`acceleration-v1` was **not** promoted. No duplicate SDK build or standalone
Windows release was created.

| Formal Windows runtime | Actual result |
| --- | --- |
| CUDA12.8.1 | NVIDIA621.01 / DriverAPI13050,29 layers offloaded, both bilingual fact/negation requests correct. This fixture's fresh-process total3688ms versus CPU9594ms; not a broad meeting-performance claim. |
| Vulkan | Normal system driver discovery, no ICD override,29 layers offloaded, both facts/negations correct. |
| Module provenance | CUDA redistributables from the component; nvcuda and Vulkan loader from the Windows driver installation. SDK PATH/override variables excluded. |

The host has an existing CUDA11.5 toolkit: it was excluded from tested processes,
not uninstalled. A physically SDK-free machine has not been tested. NVIDIA's
installer showed an aggregate failure despite successful selected rows and
SetupAPI; its overall failure reason is unresolved. The system now runs621.01,
device problem code0, with working CUDA initialization/enumeration. No agent
reboot was performed.

Exact versions/hashes: [four-way candidate](../evaluation/acceleration-candidate-integrity-2026-10-27.json),
[CUDA](../evaluation/windows-cuda-formal-2026-10-27.json),
[Vulkan](../evaluation/windows-vulkan-formal-2026-10-27.json),
[driver](../evaluation/windows-driver-preparation-2026-10-27.json).

## Actual Settings and failure lifecycle

The initial lifecycle measurements froze application `39486ea` before the shared
scheduler merge; later enable-02/restart-04/cpu-03 and short capture exercise the
configuration-guard build after `32bb449`. Each stage keeps its actual app/source
identity. App/CLI hashes, metadata traces, module audits and failed attempts are retained
in [signed lifecycle evidence](../evaluation/windows-signed-lifecycle-2026-10-27.json).
The native fixture is `tests/native-windows-gpu.js`.

| Case | Observation |
| --- | --- |
| Download/install | Real Settings fetched the normal-key signed public candidate, downloaded657MB, checked7 members and installed atomically. Detection/download alone did not activate. |
| Test-enable | Actual CPU/GPU bilingual facts, offload and faster-result gate passed. No activation/installation marker was seeded. |
| Restart | CUDA translation3 items and minutes cursor3, unchanged source/exact citations. After the status fix, cold idle `cpu-02` first observed active before disabling. |
| Use CPU | Settings cleared activation. Subsequent CLI generation actually used CPU with complete source/citations and no remaining child. |
| Insufficient memory admission | Supported4B/8192 estimates4992MiB, exceeding the real4GiB device. Refused before loading; restored1.7B/2048. 4B weights were absent and not downloaded. |
| Allocation OOM | **Not triggered.** An owned DriverAPI context allocated/touched1096MiB, free3296→2200→3296MiB, then released/destroyed. Target1.7B/8192 still passed. This pressure attempt proves cleanup, not OOM fallback. |
| Worker fault | Terminated only an exact-hash direct CUDA child through a held handle after readiness. One failed CUDA request then one CPU attempt; completed minutes, cleared certificate, exact source/citations, no remaining child. |
| Recording plus generation |92s synthetic paced replay retained exact PCM and live CUDA translation/minutes. Startup backlog2880ms exceeded the unchanged2500ms gate: **overall failed**. No microphone acceptance claim. |

Earlier failed fixture attempts remain visible: automatic-translation races,
unsupported context, configuration-busy protection and memory trials that did not
exhaust admission are not counted as passes.

Three92s ASR-only/CPU-concurrent pairs, including Release/static-CRT ASR, also
retain failed startup gates. ASR-only peaks3264/3136/3456ms versus384/456/464ms
maximum after10s. Concurrent arms preserve PCM, live translation/minutes and
citations; fixed-block paired ASR units match. A Debug-only explanation is not
supported. No threshold was relaxed. CLI clock starts after capture setup;
negative capture-wall overhead is not physical device latency. See
[replay diagnostics](../evaluation/windows-recording-contention-2026-10-27.json).

## Reproduction and handoff

Use an isolated workspace with
`OMASCRIBE_ACCELERATION_CANDIDATE_VERSION=acceleration-ci-40367160891`, a prepared
model directory and the CPU runtime. The candidate cache is separate; normal
release key, exact version, pinned runtime and archive/member checks remain.
Do not seed markers or bypass signature checks. Publication stays with the
shared [candidate pipeline](acceleration-candidates.md).

For native QA, build `cargo +1.99.0 build -p omascribe --features ui-test --locked`
with a separate Tauri identifier and protocol scheme. Set `OMASCRIBE_DATA_DIR`,
`OMASCRIBE_MODELS_DIR`, `OMASCRIBE_RUNTIME_DIR`, `OMASCRIBE_UI_TEST_SCRIPT` and a
new `OMASCRIBE_UI_TEST_REPORT`, then launch `target/debug/omascribe.exe`.
For GPU phases prepend `window.__omascribeGpuQa={phase,meeting}` to the fixture.
Seed only fictional transcripts while the app is closed. Settings actions use
real buttons; separately audit effective backend, direct owned children,
source/citations and cleanup. Physical Escape cancels the fixture.

`tests/benchmark_windows_gpu.py --formal` verifies component metadata/archive/
members, strips SDK/driver overrides and audits modules. Its CUDA lane requires
DriverAPI12.8 or newer for acceptance, without changing app CUDA12 compatibility.

Submit exact source/runtime/toolchain/driver versions, hashes and actual results
to the unified pipeline. Keep startup backlog, untriggered allocation OOM,
physical microphone coverage and failed attempts visible. The earlier scheduler
regression passed; the new incremental fact/state gate failed and stays disabled.
Keep the shared legacy stale-budget finding in the release backlog too. Finish
automatic post-stop completion, physical capture and model-quality gates before approval. Uploading a Windows
EXE does not activate Settings downloads.
