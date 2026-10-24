# Native desktop preview

macOS and Windows packages are built only on GitHub-hosted runners. The Linux development laptop does not cross-compile them. See [Native release](../.github/workflows/native-release.yml) for the exact source commit, runner and validation logs. This preview needs physical-device testing; a green workflow is not a microphone or visual-quality certification.

## Install

Download the matching installer from [Releases](https://github.com/EazyLee30/omascribe/releases). Check its SHA-256 against `SHA256SUMS.txt` if needed. Models make these packages approximately 1–1.5 GB. Keep at least 4 GB free for installation and additional space for recordings.

| System | Package and installation |
| --- | --- |
| macOS 11+, Apple Silicon | Open `macOS-AppleSilicon.dmg`; drag OmaScribe into Applications. |
| macOS 11+, Intel with AVX2 | Open `macOS-Intel.dmg`; drag OmaScribe into Applications. |
| Windows 10/11, x64 with AVX2 | Run `Windows-x64.exe`; installs for the current user. |

The Mac app uses an ad-hoc signature and is not notarized. macOS may require **System Settings → Privacy & Security → Open Anyway** for this particular downloaded app. Windows may show SmartScreen because the preview has no publisher certificate. Use the application-specific confirmation after checking the repository and filename; do not disable system-wide protection.

Allow microphone access when prompted. On macOS check **Privacy & Security → Microphone**; on Windows check **Privacy & security → Microphone**, including desktop-app access. Relaunch after changing permissions. Choose the desired microphone in OmaScribe Settings. Bluetooth/device changes may require ending the current take and selecting the device again.

All three installers include the pinned ASR and writing models, the ASR worker, CLI, llama.cpp CPU runtime, license notices and build manifest. Windows includes an offline WebView2 installer. No Python, Rust, Node or model download is required to run the app. WAV import is built in. Other audio formats require FFmpeg on `PATH`; Finder-launched macOS apps may not inherit a shell-modified PATH, so use WAV for this preview.

## Test a complete meeting

1. Create a meeting, choose the microphone, and record a short Chinese/English conversation. Include a person’s name, a date and a deliberately unresolved decision.
2. Write a few keywords in the Markdown editor. Confirm that the AI document appears automatically and its source timestamps play the right passage.
3. Pause, resume, and finish the take. **Record another take** in the same meeting. Close and reopen the app, then record a third take. Earlier text, corrections and notes should remain.
4. Open **Files**. Reveal each WAV and transcript in Finder/Explorer. Confirm the complete transcript contains all takes and each take has its own playable WAV.
5. Try the transcript dock, floating window, compact mode and edge snapping. Enable captions, change top/bottom and monitor, then test click-through and idle hiding. Window managers and macOS full-screen Spaces can affect overlay visibility.
6. Edit a generated paragraph and continue speaking. Your paragraph should be retained, with new material added separately. Test toolbar, `/`, literal slash, Chinese input, and Cmd/Ctrl shortcuts.
7. Disconnect external networking while keeping loopback available, relaunch, then record, transcribe, generate notes and export.

CPU inference, microphone-only capture, and single-request writing are intentional in this preview. System-audio mixing, speaker identification and GPU acceleration are not included. Long meetings and device-specific microphone behavior remain to be tested on real Macs and PCs.

## Data and feedback

| System | Writable data |
| --- | --- |
| Linux | `$XDG_DATA_HOME/omascribe` or `~/.local/share/omascribe` |
| macOS | `~/Library/Application Support/OmaScribe` |
| Windows | `%LOCALAPPDATA%\OmaScribe` |

Bundled models stay in the application’s read-only resources. Your recordings, notes, database and exports stay in the account’s data folder, separate from upgrades. The app does not encrypt these files. Environment overrides remain available for isolated testing.

When reporting a problem, include app version, OS version, CPU/architecture, microphone type and steps to reproduce. Describe whether recording, transcription, writing or the window failed. Share a small public/fictional sample if possible; do not upload a personal meeting or raw runtime logs containing meeting text or local request credentials.

## What CI verifies

Linux CI is a prerequisite for the exact same source commit. Each native runner executes core tests and strict Clippy, builds the app and inference binaries, packages the verified models, and exercises the actual installed/bundled resources. Public speech checks two takes, restart persistence and retranscription boundaries. Fictional Chinese-date and English-owner/deadline fixtures exercise the real local writing model, evidence checks and transactional storage. A desktop-process launch smoke test checks for immediate startup failure. Validation JSON accompanies each installer.

Physical microphone capture, permission dialogs, visual layout, caption behavior across Spaces/monitors, and sustained thermal performance are explicitly outside these hosted-runner checks.
