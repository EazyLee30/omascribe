# Contributing

OmaScribe starts with a small problem: keep recording, let a person write, and make the transcript useful without a cloud service.

1. Build with `bin/build` and install model assets with `bin/setup`.
2. Use `OMASCRIBE_DATA_DIR=/tmp/omascribe-dev` for development. Set `OMASCRIBE_MODELS_DIR` and `OMASCRIBE_RUNTIME_DIR` to your installed asset directories to avoid duplicate downloads.
3. Run `bin/test` and `cargo clippy --workspace --all-targets -- -D warnings` before opening a PR.
4. Describe the behavior that changes and the validation performed. Include a screenshot for visible changes.

Preserve these boundaries:

- Recording and disk writes cannot wait for inference or the WebView.
- User Markdown and accepted minutes must never be overwritten by AI.
- Source IDs and revisions must survive corrections and re-transcription.
- A failed or cancelled inference must not advance the coverage cursor.
- Runtime assets, microphone recordings, databases, API keys, and personal paths do not belong in Git.
- All three UI languages and READMEs should describe the same behavior.

`tests/native-ui.js` runs inside the real WebKit window with the explicit `ui-test` Cargo feature. It briefly records the microphone to verify pause/resume and finish. Use only an isolated test workspace. Production builds exclude test reporting, WebView snapshots and script injection.

`tests/evaluate.py` exercises the real local writing model through the production retrieval, validation and storage pipeline. Add a fictional regression case before changing prompts, retrieval or merge policy. Report failed cases and timing; a tiny regression suite is not a general factuality benchmark.
