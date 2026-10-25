# Desktop installation and release verification

macOS, Windows and Linux packages are built on matching GitHub-hosted runners. See [Native release](../.github/workflows/native-release.yml) for the exact source commit, runner and validation logs. Hosted-runner checks do not certify physical microphones or visual quality on every device.

## Install

Download the matching installer from [Releases](https://github.com/EazyLee30/omascribe/releases). Check its SHA-256 against `SHA256SUMS.txt` if needed. Models make these packages approximately 1–1.5 GB. Keep at least 4 GB free for installation and additional space for recordings.

| System | Package and installation |
| --- | --- |
| macOS 11+, Apple Silicon | Open `macOS-AppleSilicon.dmg`; drag OmaScribe into Applications. |
| macOS 11+, Intel with AVX2 | Open `macOS-Intel.dmg`; drag OmaScribe into Applications. |
| Windows 10/11, x64 with AVX2 | Run `Windows-x64.exe`; installs for the current user. |
| Linux x64 with AVX2, Ubuntu 22.04+ or compatible | Install the `.deb` with `sudo apt install ./OmaScribe_0.3.3_Linux-x64.deb`, or make the AppImage executable and launch it. |

The Mac app uses an ad-hoc signature and is not notarized. macOS may require **System Settings → Privacy & Security → Open Anyway** for this particular downloaded app. Windows may show SmartScreen because the app has no publisher certificate. Use the application-specific confirmation after checking the repository and filename; do not disable system-wide protection.

Allow microphone access when prompted. On macOS check **Privacy & Security → Microphone**; on Windows check **Privacy & security → Microphone**, including desktop-app access. Relaunch after changing permissions. Choose the desired microphone in OmaScribe Settings. Bluetooth/device changes may require ending the current take and selecting the device again.

All five installers include the pinned ASR and writing models, the ASR worker, CLI, llama.cpp runtime (Metal + CPU on Apple Silicon, CPU elsewhere), license notices and build manifest. Windows includes an offline WebView2 installer. No Python, Rust, Node or model download is required to run the app. WAV import is built in. Other audio formats require FFmpeg on `PATH`; Finder-launched macOS apps may not inherit a shell-modified PATH, so use WAV if FFmpeg is not available to the app.

Linux recording needs `pipewire-bin` and a running PipeWire user session. The Debian installer declares PipeWire, FFmpeg and desktop link-registration dependencies (`desktop-file-utils`, `xdg-utils`). AppImage users install these through their distribution package manager. Without the link-registration tools, the offline workspace still opens and configuration files can be imported in Settings. AppImage mounting may require FUSE 2; `--appimage-extract-and-run` is available if mounting is unavailable. Linux binaries are built on Ubuntu 22.04; older glibc distributions and ARM Linux are not supported by these packages.

## Test a complete meeting

1. Create a meeting, choose the microphone, and record a short Chinese/English conversation. Include a person’s name, a date and a deliberately unresolved decision.
2. Write a few keywords in the Markdown editor. Confirm that the AI document appears automatically and its source timestamps play the right passage.
3. Pause, resume, and finish the take. **Record another take** in the same meeting. Close and reopen the app, then record a third take. Earlier text, corrections and notes should remain.
4. Open **Files**. Reveal each WAV and transcript in Finder/Explorer. Confirm the complete transcript contains all takes and each take has its own playable WAV.
5. Try the transcript dock, floating window, compact mode and edge snapping. Enable captions, change top/bottom and monitor, then test click-through and idle hiding. Window managers and macOS full-screen Spaces can affect overlay visibility.
6. Edit a generated paragraph and continue speaking. Your paragraph should be retained, with new material added separately. Test toolbar, `/`, literal slash, Chinese input, and Cmd/Ctrl shortcuts.
7. Disconnect external networking while keeping loopback available, relaunch, then record, transcribe, generate notes and export.

Microphone-only capture and single-request writing are intentional. Apple Silicon selects Metal when the bundled runtime and an actual Metal device are available, otherwise CPU. Optional local speaker clustering is available after recording; download its models first. MLX remains a separately installed experimental backend. System-audio mixing is not included. Long meetings and device-specific microphone behavior need physical-device testing.

## Data and feedback

| System | Writable data |
| --- | --- |
| Linux | `$XDG_DATA_HOME/omascribe` or `~/.local/share/omascribe` |
| macOS | `~/Library/Application Support/OmaScribe` |
| Windows | `%LOCALAPPDATA%\io.omarchy.omascribe` |

Bundled models stay in the application’s read-only resources. Your recordings, notes, database and exports stay in the account’s data folder, separate from upgrades. The app does not encrypt these files. Environment overrides remain available for isolated testing.

When reporting a problem, include app version, OS version, CPU/architecture, microphone type and steps to reproduce. Describe whether recording, transcription, writing or the window failed. Share a small public/fictional sample if possible; do not upload a personal meeting or raw runtime logs containing meeting text or local request credentials.

## What CI verifies

Linux CI is a prerequisite for the exact same source commit. The workflow publishes a stable release only when all five installers have passed. Each native runner executes core tests and strict Clippy, builds the app and inference binaries, packages the verified models, and exercises the actual installed/bundled resources; Linux checks the payload and desktop launch from both the extracted Debian package and AppImage. Public speech checks two takes, restart persistence and retranscription boundaries. Fictional Chinese-date and English-owner/deadline fixtures exercise the real local writing model, evidence checks and transactional storage. A desktop-process launch smoke test checks for immediate startup failure. Validation JSON is retained in the GitHub Actions artifacts.

Physical microphone capture, permission dialogs, visual layout, caption behavior across Spaces/monitors, and sustained thermal performance are explicitly outside these hosted-runner checks.

## 0.3.2 update checks

Verify update consent is off, cancellation works, signatures reject changed bytes and version labels, and active work prevents installation. 0.3.1 installs can upgrade to 0.3.2 through Settings → Software updates; 0.3.0 needs a manual install first. AUR integration is deferred; this release provides AppImage and deb for Linux.


## 0.3.3 workspace and smaller updates

The first-time installers still include the default offline models. Version 0.3.3 and later use `latest-runtime.json` for routine application/runtime updates. Before replacing the application, the updater copies bundled weights to `model-store/` with the pinned SHA-256 checks; a failed copy stops installation. Existing managed models are reused. These are component-separated updates, not binary delta patches. Clients through 0.3.2 continue to use the compatible full payload in `latest.json` for one transition update.

Each native release job builds both packages, verifies their signatures, extracts/installs the smaller package, confirms it contains no model weights, migrates models from a full installation and then runs real local ASR and minutes with the migrated models. The smaller payload has a 350 MB upper bound. Validation remains in the Actions artifacts; the normal release description starts with five direct installer links.

`tests/native-workbench.js` exercises meeting tabs, fixed and unframed editor chrome, separate OS transcript windows, docking, named layouts and 740×560 controls in an isolated native WebView. macOS manual UI checks also cover slash commands and the selection-only formatting toolbar. These checks use fictional data; they do not establish speaker-identification accuracy on general meetings. A one-speaker public recording checks preservation of an explicitly named voice; overlap/uncertainty and grouped assignments have unit tests.
