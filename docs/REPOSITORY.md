# Repository map

The root contains the workspace manifests, model manifests, main README and project directories. Desktop web entry pages and their resources live together under `src/`.

| Location | Contents |
| --- | --- |
| `src/` | Desktop TypeScript, styles and the four HTML entry pages |
| `src/assets/` | Bundled artwork, app icon and companion atlas |
| `src/public/` | Static files copied unchanged into the frontend build |
| `src-tauri/` | Native desktop windows, commands and Tauri configuration |
| `crates/` | Shared Rust core, CLI and ASR worker |
| `scripts/` | Setup, packaging and release tools |
| `scripts/bin/` | Linux source setup/build/install/test launchers |
| `packaging/` | Platform packaging files and third-party license notices |
| `docs/` | Guides, research, validation evidence and release notes |
| `docs/i18n/` | Simplified and traditional Chinese READMEs |
| `.github/` | Workflows, contribution guidelines and security policy |
| `landing/` | Public website source and build |
| `community/` | Reviewed model profiles and submission schema |

## Development entry points

Run commands from the repository root:

```sh
npm run dev
npm run build
cargo test -p omascribe-core --locked
scripts/bin/setup --help
```

Vite uses `src/` as its web root and writes to the repository's `dist/`. The native application and dev server still open `/index.html`, `/transcript.html`, `/captions.html` and `/companion.html`. Static brand URLs remain `/brands/...`.

## Path migration

| Previous source path | Current source path |
| --- | --- |
| `index.html`, `transcript.html`, `captions.html`, `companion.html` | Same filenames under `src/` |
| `assets/` | `src/assets/` |
| `public/` | `src/public/` |
| `bin/` | `scripts/bin/` |
| `licenses/` | `packaging/licenses/` |
| `README.zh-CN.md`, `README.zh-TW.md` | Same filenames under `docs/i18n/` |
| `CONTRIBUTING.md`, `SECURITY.md` | Same filenames under `.github/` |

The launchers resolve their own location, so they can also be called from another working directory. Historical release notes describe their tagged versions and may retain the old `bin/` commands. Runtime data, model-store paths and packaged license destinations are independent of these source-directory moves.

When synchronizing an older platform branch, merge this layout change before adding more assets or entry pages. Preserve edits made on both branches; update any new `bin/setup` guidance to `scripts/bin/setup`, new icon references to `src/assets/icon.svg`, and new public files to `src/public/`. The shared core remains in `crates/`, native code in `src-tauri/` and TypeScript in `src/`.
