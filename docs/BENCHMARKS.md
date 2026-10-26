# Hardware validation

Tested on a **MacBook9,1 (2016, 12-inch)** with an Intel Core m5-6Y54, 2 cores / 4 threads, approximately 7.6 GiB usable RAM, Intel HD Graphics 515 and a fanless chassis. The host runs Omarchy 4.5.0-rc1 and Linux 7.2.8. Tests were performed on 2026-10-23/24.

These are development measurements from this machine, not guaranteed performance on every installation.

## One-hour recording / ASR baseline

The upstream Zipformer public `test_wavs/0.wav` clip was paced and repeated for 60 minutes through the application's file writer, ASR worker and original note scheduler. Every recorded PCM sample was compared with the expected repeated source.

| Observation | Result |
| --- | --- |
| Recorded duration | 3,600,000 ms |
| Recorded PCM bytes | 115,200,000 |
| Sample comparison | Exact match for the full recording |
| Completion state | Ready; no recorder error |
| Final ASR compute RTF | 0.141 |
| Sampled ASR backlog, P95 / maximum | 320 / 640 ms |
| Sampled process-tree peak PSS | 1,030.2 MiB |
| Lowest sampled system available memory | 2,251 MiB |
| Highest sampled thermal-zone temperature | 99 °C |

**Scope:** this was the earlier recorder baseline with Qwen3.5-0.8B and a 120-second note schedule. It did not include the final GUI, pause/resume, caption window or Qwen3-1.7B document writer. Resource sampling started roughly 16 minutes into the run. Builds and other tests ran concurrently. The temperature is a mixed development workload observation, not a steady-state app power benchmark. Sampled processing backlog is **not** visible caption latency.

The resulting PCM SHA-256 was `89f427467d8f0d3221d47a0d44535a36f9c919346a4f23e49302210611e2d458`.

## Native interaction validation

The actual Tauri/WebKit application was run in a Linux network namespace with loopback enabled and **no external network interface**. A fictional transcript fixture was used for AI checks. Short microphone recordings stayed in a temporary workspace and are not published.

Verified operations include Markdown rendering and table serialization, slash commands, literal slash input, Chinese composition, autosave, start → pause → resume → finish, duration freezing while paused, finish while paused, WAV playback payload, Markdown/audio export, floating/compact/right-docked transcript, and native top/bottom caption commands. The final capture-control run recorded 6,311 ms with an ASR compute RTF of 0.181. Those short samples test controls, not microphone accuracy.

Caption commands created and reconfigured native layer-shell surfaces. They are not a measured pixel-latency or multi-monitor visual compatibility benchmark. README screenshots are app-owned WebKit snapshots with fictional meeting content, not mockups or desktop captures.

## Local document writer

The final model is **Qwen3-1.7B Q4_K_M (Unsloth)**, one CPU thread, 2,048-token context, non-thinking mode. `tests/evaluate.py` runs the production pipeline against four fictional cases: guessed Chinese budget, English owner/deadline, a corrected budget/owner, and bilingual scope.

All four release-CLI cases passed the targeted checks; measured task times were 24.4, 23.8, 36.0 and 35.8 seconds respectively.

See [`validation/document-cases.json`](validation/document-cases.json) for the recorded outputs and checks. These targeted checks cover names, deadlines, selected unsupported claims and source integrity. They do not score general prose quality or prove entailment. During development, small models produced omitted owners, repetitive sections and tense errors; explicit evidence checks and the regression fixtures were added in response. Some repetition, imperfect grouping or unexpected output language may still occur.

The final native automatic-document fixture committed three source-linked paragraphs in 76.8 seconds while a release build was running concurrently. The capture and UI checks used earlier development builds; the final default-model fixture and screenshots used the 1.7B document pipeline.

## Final-model joint load (three minutes)

A 180-second paced public clip loop exercised the release CLI, ASR and Qwen3-1.7B together in a network namespace with no external interface. A local release compilation ran concurrently.

| Observation | Result |
| --- | --- |
| Audio duration / bytes | 180,000 ms / 5,760,000 bytes |
| Full PCM comparison | Exact match |
| Completion / recorder error | Ready / none |
| Final ASR compute RTF | 0.111 |
| Sampled processing backlog P95 / max | 320 / 320 ms |
| Sampled process-tree peak PSS | 2,188.2 MiB (about 2.14 GiB) |
| Lowest sampled available system memory | 2,403 MiB |
| Current generated document | 3 cited sections |

[Machine-readable result](validation/joint-load.json). This includes CLI + ASR + LLM, not the WebView, and is a short joint-load check rather than a full-hour final-app benchmark. No accuracy or visible-latency claim follows from the repeated audio.

## What is not established

- Real-meeting Chinese CER or English WER against independent human transcripts.
- P95 end-to-end caption latency or a full-hour final GUI + 1.7B joint workload.
- Reliable far-field speech, overlapping speakers, proper nouns or diarization.
- Sleep/resume and microphone hot-plug reliability across drivers.
- A universal bound on memory, temperature, battery drain or document latency.

A public short-clip loop is useful for persistence and scheduling tests, but cannot replace a representative meeting evaluation. Reproduce the tests with [TESTING.md](TESTING.md), then evaluate your own audio and notes before relying on them.

## v0.1.1 regression (2026-10-24)

The real Tauri/WebKit flow passed 11 checks, including two separate finished recordings in one meeting, pause/resume, file-manager reveal, five readable meeting files, automatic cited minutes and preservation of a manually edited paragraph. Loading, reading and writing phases were all observed. The fictional document task completed in about 193 seconds while the CPU was observed around 800 MHz with a power-saving energy preference. This run verifies completion under slow conditions; it is not a new throughput or latency benchmark. [Machine-readable result](validation/native-v0.1.1.json).

The Rust suite includes 12 tests: SSE completion/truncation/error handling, source/uncertainty validation, transactional cancellation, legacy recording recovery, sample-exact append and WAV export across chunk boundaries. The two Python harness unit tests also pass.

A separate public-speech regression imported the same sherpa example into two takes, restarting the app between them. All 321,700 samples (20.106 seconds) were retained; the first WAV and a saved transcript correction were unchanged. New segment IDs and timestamps continued monotonically. A full retranscription also respected the take boundary and left both WAVs unchanged. This verifies data integrity, not recognition accuracy. [Machine-readable result](validation/multitake-v0.1.1.json).

## CPU writing-model comparison (2026-10-26)

Apple M5, macOS 27.0.1 arm64, Rust 1.99 debug CLI; Qwen3 Q4_K_M, one CPU thread, 2,048 context and 4,096 MiB process ceiling. Both models ran the same 18 fictional minutes cases twice, reversing model order. Inputs include phased corrections, exact citations, owner/deadline/negation checks, output language and protected human paragraphs. Every model/CLI/runtime digest was verified, including revalidation after the comparison. [Machine-readable summary](evaluation/minutes-models-2026-10-26.json) binds the complete local raw report by SHA-256.

| Model | Passed cases per trial | All-case time per trial | Sampled CPU per trial | Peak summed tree RSS |
| --- | --- | --- | --- | --- |
| 1.7B | 18 / 18, both trials | 243.9 / 245.8 s | 230.3 / 232.8 s | 2,526.8 / 2,526.5 MiB |
| 0.6B | 16 / 18, both trials | 118.0 / 118.8 s | 106.1 / 108.0 s | 1,173.3 / 1,173.4 MiB |

All failed/repaired work is included. The 1.7B arm made 21 attempts with one rejected draft/repair per trial. The 0.6B arm made 26 attempts with eight rejected drafts and four repairs per trial. In `bilingual_scope` and `zh_minutes_from_en`, 0.6B repeatedly emitted English despite the Chinese output request. The validator rejected those drafts and left sources pending; no wrong-language document was accepted. The quality gate therefore keeps the 1.7B default. These results do not establish that 0.6B can replace it or that a cross-model escalation policy works.

Case time includes CLI/workspace work and all requests; model downloads and asset hashing are outside the measured case loop. Each generation command opens a CLI, so this does not measure a continuously warm meeting worker. CPU is a 250-ms sampled lower bound; summed RSS may double-count shared pages. PSS, GPU memory and energy were unavailable. The corpus predates the additional Windows name fixture in PR #2; integrated validation must rerun the current full corpus. Pattern/coverage/citation checks are targeted regressions, not general semantic accuracy, ASR speed or a release/battery benchmark.

## Apple Metal baseline (2026-10-27)

The same 1.7B weights and 18 fictional fixtures ran on Apple M5 / 24 GiB / macOS 27.0.1, using production balanced batching, one thread, 2,048 context and the pinned llama runtime. Both backends passed all cases in both reversed-order trials, including one rejected draft and its repair in each arm. Every actual Metal model command proved 29/29 layers offloaded; shortcut-only mic checks were counted separately. [Bound summary and raw report hashes](evaluation/apple-metal-2026-10-27.json).

| Backend | Usable complete-suite times | Sampled CPU | Peak summed tree RSS |
| --- | --- | --- | --- |
| CPU | 244.0 s; separate repeated arm 247.7 s | 230.8 / 232.6 s | 2,528.4 / 2,529.0 MiB |
| Metal | 32.2 / 32.3 s | 10.2 / 10.3 s | 1,489.4 / 1,487.2 MiB |

The original first CPU arm (261.1 s) overlapped another workspace's 22-second `npm ci`, confirmed by timestamps. It remains in the raw quality record but is excluded from isolated performance comparisons; only that arm was repeated with the same inputs and executable. This does not fully control unrelated desktop activity. These times include CLI work, loading, all cases and repairs, not just decoding. Sampled CPU is not energy, and summed RSS is not unique physical unified-memory usage. Metal working-set/free values are not independent VRAM; PSS/GPU allocations/power were unknown. There is no natural-ASR contention, sustained thermal or M1–M4 result yet. The Windows PR's additional fixture/source changes still require Mac integration validation. No new backend/model default or stable release is admitted by this baseline.
