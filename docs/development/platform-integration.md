# Three-platform integration status

Updated 2026-10-26. This records source/CI/device evidence separately; an available backend or green build does not prove accelerated inference or release readiness. The full research plan remains in `inference-implementation.md`.

| Lane | Owning PR | Implemented / observed | Next verification |
| --- | --- | --- | --- |
| Shared inference and Mac | [#1](https://github.com/EazyLee30/omascribe/pull/1) | Scoped workers, fair resumable translation, terminology queue, cache validity, lower-cost retrieval; CPU 1.7B passed the 18-case minutes corpus in both orders; 0.6B failed two output-language cases and is not promoted | Ground-truth ASR corpus, full integrated fixtures, incremental minutes and speaker work; Metal/Apple resource and fallback gates |
| Windows GPU / native UX | [#2](https://github.com/EazyLee30/omascribe/pull/2), stacked on #1 | Native UI/locale/name checks reported from Windows 10 / GTX 1650. Driver compatibility and static optional-runtime builds implemented. Targeted Vulkan run `40336497395` compiled, then failed its runtime version probe; catalog correctly skipped | Windows owns its loader fix and subsequent targeted build; CUDA 11.5 local experiment is separate from distributable CUDA 12.8; real offload/facts/VRAM/time/failure fallback still required |
| Linux / Omarchy desktop | [#3](https://github.com/EazyLee30/omascribe/pull/3) | PR reports live logind inhibitor, PipeWire mixed capture, theme and positioning tests on Omarchy / Hyprland / Intel m5. Linux lifecycle/desktop code stays in that lane | Integrated CI/native checks, actual remote-call/echo and mixed-DPI review; baseline/duplicate workflow history must be aligned with #1 |

## Apple Silicon priority

The shipped M-series runtime declares Metal enabled at llama.cpp revision `5aeac37a35b44fde4bdffaec9159f2fcb253b622`; its native `--list-devices` exposes `MTL0: Apple M5`. Current auto selection can choose Metal and asks for GPU layers. These establish availability/configuration, not successful model computation.

Mac work next requires actual positive layer-offload/Metal compute evidence, then the same pinned 1.7B model and full minutes/translation corpus on CPU versus Metal, with reversed order and all repairs/failures counted. Measure startup, prefill/decode, sampled process resources and available unified memory separately. Runtime-reported Metal working-set/free figures must not be called independent physical VRAM or whole-system free memory. GPU allocation/energy values stay unknown unless measured by the worker or a suitable device API. Do not sum shared memory twice.

Resource admission, cancellation under ASR backlog, model/runtime/config changes and Metal failure-to-CPU behavior need explicit tests. M5 measurements cannot establish M1/M2/M3/M4 performance; keep architecture/build compatibility and missing-device evidence visible. Compare targeted settings such as batching/attention only after an untuned production baseline, preserving facts/language/citations and the recording priority. MLX/CoreML remain separately gated experiments; CUDA/HIP/Vulkan SDKs stay outside base installers.

## Coordination and integration gates

Windows source already follows #1. Linux commit `35ae29f` also has #1's `9c7c4b4` as its parent, despite PR #3 targeting main; its current diff consequently repeats shared inference history. A merge-tree check found one conflict, the equivalent Windows-Vulkan workflow edit. [Coordination comment](https://github.com/EazyLee30/omascribe/pull/3#issuecomment-6419558075) asks that lane to align its base and inherit the shared workflow instead of submitting it twice. No branch was changed or merged by this check.

Before integration: fetch exact heads, inspect shared hunks, resolve against the final implementation, run the updated complete fixtures (including Windows's added case) and native checks for each affected platform. Optional SDK build results bind their exact native inputs; partial developer artifacts cannot publish a catalog. Report source checks promptly in PRs; installer compilation and stable publishing are distinct later gates. Cleanup of completed generated images and borrowed weights is part of each stage, while current-run inputs and reproducible result records remain available.
