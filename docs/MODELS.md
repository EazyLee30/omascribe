# Models and native runtimes

Bundled assets are pinned in [`models.lock.json`](../models.lock.json); selectable/downloadable models are pinned in [`model-catalog.json`](../model-catalog.json). `bin/setup` verifies downloaded weights before installing them. Runtime inference never downloads assets.

| Component | Pinned choice | Role |
| --- | --- | --- |
| ASR | sherpa-onnx small bilingual streaming Zipformer, 2023-02-16 | Mandarin/English partial and final transcript |
| Note writer | unsloth/Qwen3-1.7B-GGUF, Q4_K_M, Unsloth quantization | Cited written paragraphs |
| LLM runtime | llama.cpp commit `5aeac37a35b44fde4bdffaec9159f2fcb253b622` | CPU; Metal on Apple Silicon builds, bounded threads/context |
| Captions | gtk-layer-shell 0.10.1, commit `fd88ba666c18ff65ea786bf7b2e270d840030817` | Native Wayland overlay |

The installed ASR inference files total roughly 58 MiB. Its upstream archive is about 458 MB and contains variants; setup extracts only the five exact root files. The Qwen GGUF is 1,107,409,472 bytes. File size is not running memory.

The initial prototype used Qwen3.5-0.8B. It is no longer the default. The document writer uses the larger 1.7B model because the product requires prose, while preserving bounded input, short output and background scheduling. This tradeoff needs further testing on diverse real meetings.

## Reproduce setup

```bash
bin/setup --models-only
bin/setup --runtime-only   # llama.cpp and caption library
bin/setup --overlay-only   # caption library only
bin/setup --verify         # verify installed weight hashes
```

Builds are pinned and use one job. Linux caption support is built into the user's data directory; it does not replace system libraries. If Meson/Ninja are unavailable, setup creates an isolated build-tools virtual environment. The Linux overlay requires GTK3, Wayland and wayland-protocols development files; macOS skips this Linux-only build. The x86-64 llama.cpp preset requires AVX2. Apple Silicon builds enable Metal and Accelerate with embedded Metal kernels.

## Licenses

OmaScribe code and original SVG artwork are MIT. Qwen3-1.7B weights are Apache-2.0; sherpa-onnx and the selected Zipformer model card specify Apache-2.0. Native packages include upstream model cards and dependency license notices. llama.cpp is MIT. gtk-layer-shell as a library is LGPL-3.0-or-later, with individual portions under MIT/GPL notices; all provided notices are retained in `licenses/` and its source is pinned above. It is loaded as a separate replaceable shared library. Tauri, Tiptap, GTK and the remaining dependencies retain their own licenses. The app's MIT license does not relicense these components.

Primary sources: [ASR model documentation](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html), [Qwen model repository](https://huggingface.co/unsloth/Qwen3-1.7B-GGUF), [llama.cpp](https://github.com/ggml-org/llama.cpp), [gtk-layer-shell](https://github.com/wmww/gtk-layer-shell).

The native macOS/Windows release workflow builds a static llama.cpp runtime on the corresponding hosted runner. Apple Silicon enables Metal/Accelerate; Windows and Intel macOS retain CPU inference. OpenMP remains disabled. The build manifest records backend capability, so an old CPU-only installation is not mistaken for an accelerated runtime. Packages include the same checksum-verified weights as Linux; test recordings stay outside installers. The Zipformer [source model card](https://huggingface.co/csukuangfj/k2fsa-zipformer-bilingual-zh-en-t) identifies its upstream model and Apache-2.0 license.

An optional experimental MLX runtime uses the separate pinned Qwen3 1.7B MLX 4-bit catalog model. It requires explicit developer setup and is not bundled with the installer. See [Apple Silicon implementation and benchmark](APPLE_SILICON.md) for resource controls, offline-loading boundaries, measured M5 results and current MLX limitations.

Additional curated weights: Qwen3 0.6B/4B Q4_K_M and the larger bilingual Zipformer (Apache-2.0), Pyannote segmentation 3.0 (MIT), and 3D-Speaker ERes2Net embeddings (Apache-2.0). See [sherpa speaker models](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/models.html) and each catalog entry’s source. The speaker pair is downloaded on demand, not included in the default installer. Model size/RAM figures are estimates; compare actual results on your recordings.
