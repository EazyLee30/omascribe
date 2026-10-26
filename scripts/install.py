#!/usr/bin/env python3
"""Install the built application into the current user's home. Never touches desktop config."""

import os, pathlib, shutil, stat

repo = pathlib.Path(__file__).resolve().parents[1]
data = pathlib.Path(
    os.environ.get("XDG_DATA_HOME", str(pathlib.Path.home() / ".local/share"))
)
root = data / "omascribe"
runtime = root / "runtime"
runtime.mkdir(parents=True, exist_ok=True)
root.chmod(0o700)
for name in ("omascribe", "omascribe-cli", "omascribe-asr"):
    source = repo / "target/release" / name
    if not source.is_file():
        raise SystemExit("Run scripts/bin/build before scripts/bin/install.")
    target = runtime / name
    temporary = target.with_suffix(".new")
    shutil.copy2(source, temporary)
    temporary.replace(target)
bindir = pathlib.Path.home() / ".local/bin"
bindir.mkdir(parents=True, exist_ok=True)
for name in ("omascribe", "omascribe-cli"):
    # A symlink keeps environment overrides available for testing and custom model directories.
    target = bindir / name
    if target.exists() and not target.is_symlink():
        raise SystemExit(f"Refusing to overwrite unrelated executable: {target}")
    temporary = bindir / (name + ".new")
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(runtime / name)
    temporary.replace(target)
icons = data / "icons/hicolor/scalable/apps"
icons.mkdir(parents=True, exist_ok=True)
shutil.copy2(repo / "src/assets/icon.svg", icons / "omascribe.svg")
apps = data / "applications"
apps.mkdir(parents=True, exist_ok=True)
# Desktop entry Exec quoting is not shell quoting.
executable = (
    str(bindir / "omascribe")
    .replace("\\", "\\\\")
    .replace('"', '\\"')
    .replace("`", "\\`")
    .replace("$", "\\$")
    .replace("%", "%%")
)
entry = (
    (repo / "packaging/omascribe.desktop")
    .read_text()
    .replace("Exec=omascribe", f'Exec="{executable}"')
)
(apps / "omascribe.desktop").write_text(entry)
print(
    "Installed OmaScribe. Open it from the app launcher, or run", bindir / "omascribe"
)
