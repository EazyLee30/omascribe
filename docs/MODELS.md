# Models and native runtimes

All exact URLs, immutable revisions, byte counts and SHA-256 digests are in [`models.lock.json`](../models.lock.json). `bin/setup` verifies downloaded weights before installing them. Runtime inference never downloads assets.

| Component | Pinned choice | Role |
| --- | --- | --- |
| ASR | sherpa-onnx small bilingual streaming Zipformer, 2023-02-16 | Mandarin/English partial and final transcript |
| Note writer | unsloth/Qwen3-1.7B-GGUF, Q4_K_M, Unsloth quantization | Cited written paragraphs |
| LLM runtime | llama.cpp commit `5aeac37a35b44fde4bdffaec9159f2fcb253b622` | CPU inference, one thread, 2,048 context |
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

Builds are pinned and use one job. Caption support is built into the user's data directory; it does not replace system libraries. If Meson/Ninja are unavailable, setup creates an isolated build-tools virtual environment. Building requires GTK3, Wayland and wayland-protocols development files. The local llama.cpp build targets x86-64 AVX2; older CPUs are unsupported by this preset.

## Licenses

OmaScribe code and original SVG artwork are MIT. Qwen3-1.7B weights are Apache-2.0; sherpa-onnx and the selected Zipformer model card specify Apache-2.0. Native packages include upstream model cards and dependency license notices. llama.cpp is MIT. gtk-layer-shell as a library is LGPL-3.0-or-later, with individual portions under MIT/GPL notices; all provided notices are retained in `licenses/` and its source is pinned above. It is loaded as a separate replaceable shared library. Tauri, Tiptap, GTK and the remaining dependencies retain their own licenses. The app's MIT license does not relicense these components.

Primary sources: [ASR model documentation](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html), [Qwen model repository](https://huggingface.co/unsloth/Qwen3-1.7B-GGUF), [llama.cpp](https://github.com/ggml-org/llama.cpp), [gtk-layer-shell](https://github.com/wmww/gtk-layer-shell).

The native macOS/Windows release workflow builds a static CPU llama.cpp runtime on the corresponding hosted runner, with GPU and OpenMP disabled. It includes the same checksum-verified weights as Linux; test recordings stay outside installers. The Zipformer [source model card](https://huggingface.co/csukuangfj/k2fsa-zipformer-bilingual-zh-en-t) identifies its upstream model and Apache-2.0 license.
