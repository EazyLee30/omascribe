<p align="center">
  <img src="assets/banner.svg" alt="OmaScribe — a local notebook for your conversations" width="100%">
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a>
</p>
<p align="center">
  <a href="https://github.com/EazyLee30/omascribe/actions/workflows/ci.yml"><img src="https://github.com/EazyLee30/omascribe/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-d8b76e?style=flat-square" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/Linux-Omarchy-91a87d?style=flat-square&logo=linux&logoColor=white" alt="Linux / Omarchy">
  <img src="https://img.shields.io/badge/runtime-offline-647950?style=flat-square" alt="Offline runtime">
  <img src="https://img.shields.io/badge/Rust-Tauri_2-b6c1a8?style=flat-square&logo=rust&logoColor=white" alt="Rust and Tauri 2">
</p>

# A few words from you. The context from your meeting.

**OmaScribe is an offline meeting recorder, bilingual transcriber, and Markdown notebook for Linux.** Write a keyword on the left; related meeting content appears in the AI minutes on the right, with the original source and an audio timestamp.

Built around a 2016 12-inch MacBook running Omarchy, with a dual-core Intel CPU and 8 GB RAM. The name combines **Oma**rchy and **Scribe**.

<p align="center"><img src="docs/screenshots/workspace-en.png" alt="OmaScribe: a rendered Markdown document on the left, keyword-guided meeting minutes on the right" width="100%"></p>
<p align="center"><sub>Actual Linux desktop window. The displayed meeting is a fictional test fixture.</sub></p>

<details>
<summary>See the editor tools and right-side transcript</summary>

![Slash commands in the Markdown editor](docs/screenshots/slash-menu.png)
![Transcript dock beside the notebook and generated document](docs/screenshots/transcript-dock.png)

</details>

## A notebook that listens

- **Start, pause, resume, finish.** Recording has its own file writer. Pauses do not create silence in the saved audio; continuing appends to the same meeting. WAV, MP3, M4A, FLAC, Ogg and other FFmpeg-supported files can also be imported.
- **Write in Markdown, see a document.** A matching dark editor with headings, lists, tasks, tables, quotes, code, links, undo/redo, a toolbar, source mode, and Chinese input support.
- **Type `/` for tools.** Search blocks, insert a timestamp, highlight a keyword or start from a meeting template. Escape keeps the slash; a slash inside normal text or a path stays ordinary text.
- **Let the minutes develop automatically.** Your keywords guide bilingual lexical retrieval. A local model turns relevant transcript evidence into titled, readable paragraphs. No update button is required.
- **Place the transcript where you want it.** Right dock, movable/resizable floating window, compact mode, and left/right edge snap. Native Wayland captions can sit at the top or bottom of a chosen screen, with font, color, opacity, width, line count, alignment, margin and click-through controls.
- **Keep your authorship.** Your Markdown is never rewritten by AI. Editing a generated paragraph protects it from subsequent generation. Sources and audio timestamps stay attached; revised sources are flagged.
- **Export the whole meeting.** Your document, current AI minutes, original sources and transcript export to Markdown, optionally alongside WAV. English, 简体中文 and 繁體中文 interfaces are included.

### From a keyword to a paragraph

You write **预算 3000？** and **张老师 / 周五测试**. The conversation says:

> 张老师周五给测试结果，预算先不定，下周再看。

The intended result is a short written paragraph: “张老师将于周五提供测试结果。预算尚未确定，将于下周继续讨论。” Each paragraph links back to its evidence. The personal estimate `3000？` stays in your notebook.

The pipeline coalesces changes and runs one local writing task at a time. On this older laptop, minutes arrive in batches rather than word by word. Recording and transcription have priority. Generative prose can still be wrong: source, number and uncertainty checks reduce specific mistakes, but **do not prove factual correctness**.

## Build and install

**Current target:** x86-64 Linux with AVX2, PipeWire, GTK3, and WebKitGTK 4.1. Tested on Omarchy. This is an early release; macOS and Windows are not supported yet.

Install the build dependencies on Arch / Omarchy:

```bash
sudo pacman -S --needed base-devel git cmake pkgconf nodejs npm python \
  gtk3 webkit2gtk-4.1 openssl librsvg ffmpeg pipewire wayland wayland-protocols xdotool
```

Install a current stable [Rust toolchain](https://www.rust-lang.org/tools/install), then:

```bash
git clone https://github.com/EazyLee30/omascribe.git
cd omascribe
bin/build          # One compile job by default, suitable for an 8 GB machine
bin/setup          # Download verified models and build pinned llama.cpp and native caption support
bin/install        # Install to your user account and app launcher
~/.local/bin/omascribe
```

Setup needs a network connection, roughly 1.6 GB of model downloads, and additional space for build caches. **After setup, recording, transcription, editing, inference, and export run offline.** No account or API key is required. Models are not stored in Git or bundled in the app archive.

`OMASCRIBE_BUILD_JOBS=2 bin/build` allows a faster build on a machine with more headroom. Compiling on a fanless MacBook can make it warm and temporarily slow the desktop.

## Use it

1. Create a meeting and choose your microphone in **Settings**.
2. **Start recording.** Write naturally; use the toolbar, Markdown shortcuts or `/` menu. Bold a topic or a name to guide retrieval.
3. The AI document develops automatically on the right. Open **Transcript** for a floating view; use the display gear for docking and **CC** for screen captions.
4. **Pause** when needed, then **Resume**. Click a source timestamp to listen, or edit a paragraph to preserve your wording.
5. **Finish** recording. Leave the app open while the remaining transcript and document batches complete, then export.

| Shortcut | Action |
| --- | --- |
| `Ctrl Shift R` | Start / finish recording; resume when paused |
| `Ctrl Shift P` | Pause / resume recording |
| `Ctrl B` / `Ctrl I` | Bold / italic |
| `Ctrl K` | Insert or edit a link |
| `Ctrl Alt 1` / `2` / `3` | Heading level |
| `Ctrl Shift 8` / `7` / `9` | Bullet / numbered / task list |
| `Ctrl Z` / `Ctrl Shift Z` | Undo / redo |
| `Ctrl F` | Open and search the right transcript dock |
| `Ctrl N` | New meeting |
| `/` at the start of a paragraph | Block menu; arrows + Enter to select, Escape for literal text |

## Local by construction

```text
Microphone / audio file → independent PCM writer → streaming ASR → SQLite
                                                                  ↓
Markdown → keywords → lexical RAG → bounded writer → validation → document revisions
                                                                  ↓
                                                        automatic merge · playback · export
```

The note model runs only when needed, with one thread, one request, and a 2,048-token context. It unloads after each pass. A sustained ASR backlog cancels note inference. Audio never passes through the WebView.

Default storage: `~/.local/share/omascribe/` (or `$XDG_DATA_HOME/omascribe`). It contains recordings, the SQLite database, models, runtime, and exports. This directory is user-only; files are **not encrypted** by the app.

| Environment variable | Override |
| --- | --- |
| `OMASCRIBE_DATA_DIR` | Meeting workspace |
| `OMASCRIBE_MODELS_DIR` | Model directory |
| `OMASCRIBE_RUNTIME_DIR` | ASR worker and `llama/` runtime directory |

A CLI is included: `omascribe-cli diagnose`, `import`, `record`, `replay`, `snapshot`, `note`, `generate`, and `export`. Results are JSON. Run `omascribe-cli help` for arguments. Only one process may write to a workspace at a time.

## Validation and limits

See the [hardware test report](docs/BENCHMARKS.md) and [reproduction guide](docs/TESTING.md). Public audio and a clearly fictional note fixture are used; no private meeting recording is included. A short clip or repeated-sample stress test cannot establish real meeting CER/WER or caption P95 latency.

Current boundaries:

- Microphone capture and file import; no system-audio mixer or speaker diarization yet.
- Mandarin/English ASR is a small, older model. Names, far-field speech, overlap, and code-switching need your own evaluation.
- Keywords guide **note retrieval**, not ASR hotword bias, in this release.
- Small-model writing can omit facts or misread a correction. Generated paragraphs need review; edited paragraphs are never automatically rewritten.
- Floating window edge placement is integrated with Hyprland. Screen captions require a Wayland compositor supporting layer-shell. They are not macOS/Windows overlays.
- No speaker diarization, translation mode or embedding-based semantic search. The current RAG is bounded, per-meeting BM25 plus Chinese character/bigram and exact keyword matching.
- Keep the machine awake during a meeting. Sleep/driver recovery and automatic sleep inhibition are not yet fully validated.
- There is no cloud sync, cross-meeting search, or automatic app updater.

## Project notes

- [Architecture](docs/ARCHITECTURE.md) · [Models, hashes, and licenses](docs/MODELS.md)
- [Contributing](CONTRIBUTING.md) · [Privacy and security](SECURITY.md)
- Inspired by the user-guided note interaction in [Google Foresight](https://developers.google.com/edge/foresight); this is an independent Rust/Tauri implementation.
- Built with [Tauri](https://tauri.app/), [Tiptap](https://tiptap.dev/), [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), [llama.cpp](https://github.com/ggml-org/llama.cpp), and [Qwen](https://huggingface.co/Qwen).

Code and original artwork: [MIT](LICENSE). Third-party models and dependencies retain their own licenses.
