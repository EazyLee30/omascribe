# Testing and evaluation

Use fictional text or licensed public speech. Do not commit microphone audio, personal meetings, databases, model weights or local logs.

## Deterministic checks

```bash
bin/test
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Rust tests cover sample-exact chunk boundaries, pause/resume append behavior, interrupted-recording recovery, the single-writer lock, Chinese/English retrieval, pending-prefix coverage, evidence rejection and atomic document updates. The Python tests ensure the model harness fails on missing required facts, unsupported claims and forged citations.

Push checked source changes promptly; installer and optional GPU builds need not finish before a source push. Ordinary pull-request CI retains application checks, Windows core/ASR tests, CPU worker linking and component packaging contracts. The four-platform/backend SDK build matrix runs only with the `build-acceleration-components` PR label or a manual **Optional acceleration components** workflow dispatch. Remove the label after the intended build to avoid rebuilding it on every push. Skipped builds are not validation evidence; record the successful run and complete native-input revision when reusing earlier results. Publishing still requires the full matrix, archive/member verification and passing application CI for the exact publication commit.

## Real local writing model

After `bin/setup`, build the CLI and run:

```bash
cargo build -p omascribe-core
python3 tests/evaluate.py --cli target/debug/omascribe-cli \
  --output /tmp/omascribe-evaluation.json
```

The harness uses production retrieval → writer → validation → storage with an isolated database. Eighteen scenarios cover later corrections across phases, language selection, named speakers, pure mic checks, source closure beyond nine passages, and human-protected paragraphs. Inputs, drafts, validation failures, citations, timing and token usage survive failed cases. Output paths must be fresh. These targeted checks do not establish general meeting quality.

For the unified offline ASR/minutes harness, public corpus preparation, baseline comparisons, hotword compatibility and optional 30-minute stress runs, see [CORE-HARNESS.md](CORE-HARNESS.md).

## Actual Tauri / WebKit flow

```bash
npm run build
cargo build -p omascribe --features ui-test -p omascribe-asr -p omascribe-core
export OMASCRIBE_DATA_DIR=/tmp/omascribe-ui-validation
export OMASCRIBE_MODELS_DIR="$HOME/.local/share/omascribe/models"
# Use the newly built ASR worker, not an older installed binary.
mkdir -p /tmp/omascribe-test-runtime
ln -sfn "$PWD/target/debug/omascribe-asr" /tmp/omascribe-test-runtime/omascribe-asr
ln -sfn "$HOME/.local/share/omascribe/runtime/llama" /tmp/omascribe-test-runtime/llama
ln -sfn "$HOME/.local/share/omascribe/runtime/platform" /tmp/omascribe-test-runtime/platform
export OMASCRIBE_RUNTIME_DIR=/tmp/omascribe-test-runtime
target/debug/omascribe-cli diagnose
python3 tests/seed-fixture.py "$OMASCRIBE_DATA_DIR"
# native-refinement.js checks citations without waiting for the writer model;
# seed a committed document section for that run only. native-ui.js must use a
# clean fixture so it exercises the real generation pipeline:
#   python3 tests/seed-fixture.py "$OMASCRIBE_DATA_DIR" --with-document
OMASCRIBE_UI_TEST_SCRIPT="$PWD/tests/native-ui.js" \
OMASCRIBE_UI_TEST_REPORT=/tmp/omascribe-ui-report.json \
  target/debug/omascribe
```

This test briefly records the real microphone. It checks slash commands and literal slashes, Markdown table roundtrip, Chinese composition, autosave, start/pause/resume/finish, frozen pause duration, finish while paused, a second take in the same meeting, the Files dialog, WAV export, floating/compact/docked transcript, native top/bottom captions and automatic cited document generation. Run it on a disposable workspace and inspect the JSON report. A returned caption API call is not a visual pixel assertion; inspect the actual overlay as well.

The test-only feature can also snapshot the app's own WebKit content to `OMASCRIBE_UI_SCREENSHOTS` for visual QA. It cannot capture other applications. All test commands and startup-script injection are absent from the default production build.

## Verify without external networking

On a Linux system permitting user/network namespaces:

```bash
unshare -Urn bash -c 'ip link set lo up; exec target/debug/omascribe'
```

Keep loopback up because Rust communicates with the local model server. The namespace has no external network interface. Models and runtime must already be installed. Audio capture still uses the local PipeWire socket.

## Public speech and sustained load

The upstream Zipformer archive includes short public `test_wavs` examples. Keep them outside this repository, then use:

```bash
omascribe-cli import /path/to/public-sample.wav
omascribe-cli replay /path/to/public-sample.wav 3600
```

Replay paces the input and loops it through the real capture/ASR/scheduler path. Run `tests/monitor.py PID /tmp/resources.jsonl` to sample process-tree PSS, available system memory and thermal zones without collecting command lines. A repeated clip establishes stability of that input path, not Chinese CER, English WER, speaker handling or meeting comprehension. Use independent human references for accuracy evaluation; never treat ASR output as its own ground truth.

For the experimental per-stream hotword lifecycle, build the current ASR worker and use a disposable output:

```bash
python3 tests/prepare_hotword_fixtures.py --directory artifacts/fixtures/sherpa-public
python3 tests/benchmark_hotword_lifecycle.py --model /path/to/zipformer-small --output artifacts/benchmarks/hotwords.json
python3 tests/evaluate_hotword_updates.py --model /path/to/zipformer-small --fixtures artifacts/fixtures/sherpa-public --output artifacts/benchmarks/hotwords-boundaries.json
```

Preparation is an explicit download; the two evaluation scripts only use local assets. They select both legacy and experimental modes themselves. `--shared` can supply a compatible tokenizer directory when the selected model lacks one. Fresh result paths preserve earlier evidence. The lifecycle report measures initialization/reconfiguration and child resources, not speech accuracy. The boundary regression checks real public-audio transcript equivalence, finalization and sample-exact pause/resume, plus silence/noise. It does not replace independent CER/WER/MER, visible latency or name anti-case evaluation. The application keeps the old path unless `OMASCRIBE_ASR_STREAM_HOTWORDS=1` is explicitly set.

See [BENCHMARKS.md](BENCHMARKS.md) for the measured scope, software versions and limitations.

## Multiple takes and readable files

After `bin/setup` and a workspace build, run `python3 tests/multitake.py --cli target/debug/omascribe-cli`. The harness imports the public sherpa sample twice into one isolated meeting, restarts between takes, and checks sample-exact append, stable IDs and a human correction, cumulative timestamps, two playable WAVs and refreshed text mirrors. It never opens the real meeting workspace. The unit suite also covers legacy migration, recovery and a WAV range crossing a 30-second chunk boundary.

SSE unit cases cover Unicode chunks, usage events, normal completion, premature EOF, token-limit termination, error events and oversized data. `tests/evaluate.py` uses the real server and validates the complete streamed generation/validation/commit path.

## Block editor and column controls

`tests/native-editor.js` is a focused `ui-test` startup script for a disposable data directory. It creates a fictional meeting and checks slash search/Enter, the platform block-menu shortcut, moving/duplicating blocks, deletion confirmation/cancellation, independent undo, literal slashes, Chinese composition, persisted Markdown, hide/restore, continuous wide–narrow–wide resizing without cropped columns, and block-handle tracking while scrolling. It does not start microphone capture. Keep its report and any screenshots outside Git. When running beside the normal application, build with a separate Tauri identifier (for example `TAURI_CONFIG='{"identifier":"si.eazylee.omascribe.editorqa"}'`) so the single-instance bridge does not forward into the user's open app.

Also perform pointer drag/drop and menu-position checks in the rendered editor: a newly written block must move without losing text, undo must undo just that move, menus must remain in the viewport, and changing layouts must preserve the same document. Native acceptance and browser pointer checks complement each other; a synthetic DOM event alone is not a real pointer-drag test.
