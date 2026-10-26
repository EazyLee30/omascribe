# Optional acceleration components

The normal installer contains CPU inference plus Metal on supported Apple Silicon builds. CUDA, HIP, Vulkan, SYCL and their SDKs are not base runtime dependencies. Build stamps explicitly invalidate older cached runtimes; `scripts/runtime_inventory.py` rejects optional libraries in the base staging tree and runtime-only updates.

`scripts/acceleration-components.py` builds the exact `models.lock.json` llama.cpp revision in `build-assets/accelerator-*`, never `build-assets/native`. Current independent variants are CUDA 12.8.1 and Vulkan on Windows/Linux x86_64. HIP is not offered until its dependency layout, compatibility and device tests are implemented. The CUDA package copies only loader-required runtime libraries under their exact requested names; it does not copy display drivers, compilers, headers or development aliases. The Vulkan loader/driver is supplied by the system. Licenses and build provenance are included.

The optional workflow produces four `.tar.bz2` archives and checks every unpacked member against its hash/size inventory. The unsigned `index.payload` is generated only after all four variants exist. Publishing is off by default. Explicit publishing requires green application CI for that commit, signs the exact payload using the application release key, verifies it using the core verifier, uploads immutable archives to `acceleration-REVISION-N`, then updates the signed catalog at the separate `acceleration-v1` release. Neither release replaces the stable application release. Sources: [pinned CUDA build](https://github.com/ggml-org/llama.cpp/blob/5aeac37a35b44fde4bdffaec9159f2fcb253b622/ggml/src/ggml-cuda/CMakeLists.txt), [pinned Vulkan build](https://github.com/ggml-org/llama.cpp/blob/5aeac37a35b44fde4bdffaec9159f2fcb253b622/ggml/src/ggml-vulkan/CMakeLists.txt), [CUDA Windows installation packages](https://docs.nvidia.com/cuda/archive/12.8.1/cuda-installation-guide-microsoft-windows/index.html).

## App flow

1. Probe adapters in the app-owned CLI with a timeout, separately from the GUI/audio thread. A detected card is a candidate, not a verified runtime.
2. Settings → Models offers compatible candidates and a signed catalog check. Catalog/download actions obey the existing model download network policy; no inventory or meeting text is uploaded.
3. Download to a separate staging directory; verify archive hash, exact regular-file members, per-file hashes and pinned runtime provenance; then rename atomically. Recheck all file hashes before the local execution test.
4. **Test & enable** runs only while idle and with the backend set to Automatic. It compares CPU and the component using the current GGUF model/context/thread/resource configuration. It requires free device memory, actual model offload, complete output, correct multilingual facts/negation and faster measured completion. Both model processes are reaped after the test.
5. A successful certificate is local and bound to device/driver, signed component inventory, model file and inference configuration. An unknown driver cannot carry verification into a later app session. Configuration/driver changes require another test. Explicit CPU never uses an optional component.
6. GPU startup failure removes activation and tries CPU once. Failed GPU generation transport is reaped before one CPU retry. Existing transactional/revision checks guard commits. Cancellation, maintenance and system memory pressure do not trigger an extra generation retry. Starting audio cancels an in-progress acceleration test.

The short local test is an admission check, not proof of whole-meeting quality, battery savings or all-driver compatibility. Separate ASR, translation, minutes, speaker, long-audio and hardware gates in the implementation plan remain required.

## Validation status

The local Mac CPU model gate and fixture tests pass. Windows/Linux component compilation, real device offload, GTX 1650 performance, driver-missing/OOM/reset, AMD shared-memory behavior and rolling Linux driver changes are still unverified. Do not describe these paths as production-validated or publish a speedup percentage based on the Mac result.

For the Windows 10 / GTX 1650 session, retain the exact app/CLI/component/model hashes and driver version. Compare the same public/fictional input on CPU and CUDA, then Vulkan if available. Record cold and warm timings, process RSS and available GPU measurements; missing values remain null. Exercise cancellation and CPU fallback before enabling the component for normal meetings. Never require the user to install the CUDA development toolkit.
