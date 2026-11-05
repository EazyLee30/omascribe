#!/usr/bin/env python3
"""Compose an updater manifest only from the four independently verified native jobs."""
import datetime, json, os, pathlib
from runtime_inventory import collect_release_artifacts, verify_release_inventory
ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGETS = {'darwin-aarch64':'macOS-AppleSilicon.app.tar.gz','darwin-x86_64':'macOS-Intel.app.tar.gz','windows-x86_64':'Windows-x64.exe','linux-x86_64':'Linux-x64.AppImage'}

def manifest(assets, version, notes, runtime=False):
    platforms = {}
    for platform, suffix in TARGETS.items():
        name = f'OmaScribe_{version}_{suffix}'
        if runtime:
            name = name.replace('.app.tar.gz','-runtime.app.tar.gz').replace('.exe','-runtime.exe').replace('.AppImage','-runtime.AppImage')
        assert (assets/name).is_file(), f'Missing {name}'
        signature = (assets/(name+'.sig')).read_text().strip()
        report = json.loads((assets/(name+'.verified.json')).read_text())
        assert report == {'verified':True, 'version':version, 'file':name}, f'Unverified update: {name}'
        assert signature, f'Empty signature: {name}'
        platforms[platform] = {'signature':signature,'url':f"https://github.com/EazyLee30/omascribe/releases/download/v{version}{'-updates' if runtime else ''}/{name}"}
    return {'version':version,'notes':notes,'pub_date':datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00','Z'),'platforms':platforms}

def prepare_release(root, commit):
    version = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    notes = (root / f"docs/releases/v{version}.md").read_text(encoding="utf-8")
    assets = root / "release-assets"
    collect_release_artifacts(root / "release-downloads", assets, version)
    checksums = verify_release_inventory(assets, version, commit)
    full = manifest(assets, version, notes)
    runtime = manifest(assets, version, notes, runtime=True)
    for name, data in [("latest.json", full), ("latest-runtime.json", runtime)]:
        (assets / name).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n",
                                   encoding="utf-8")
    (assets / "SHA256SUMS.txt").write_text(checksums, encoding="utf-8", newline="\n")


if __name__ == '__main__':
    prepare_release(ROOT, os.environ["GITHUB_SHA"])
    print('Full and runtime update manifests ready for all four native targets')
