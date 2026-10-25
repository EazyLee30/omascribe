#!/usr/bin/env python3
"""Compose an updater manifest only from the four independently verified native jobs."""
import datetime, json, pathlib
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

if __name__ == '__main__':
    version = json.loads((ROOT/'package.json').read_text())['version']
    assets = ROOT/'release-assets'
    data = manifest(assets, version, (ROOT/f'docs/releases/v{version}.md').read_text())
    (assets/'latest.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
    data = manifest(assets, version, (ROOT/f'docs/releases/v{version}.md').read_text(), runtime=True)
    (assets/'latest-runtime.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
    print('Full and runtime update manifests ready for all four native targets')
