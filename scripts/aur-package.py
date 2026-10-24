#!/usr/bin/env python3
"""Generate a pinned Arch recipe from the verified native deb, before publication."""
import argparse, hashlib, json, pathlib, shutil, tarfile
ROOT = pathlib.Path(__file__).resolve().parents[1]

def generate(assets, out):
    version = json.loads((ROOT / 'package.json').read_text())['version']
    deb = assets / f'OmaScribe_{version}_Linux-x64.deb'
    with deb.open('rb') as f:
        checksum = hashlib.file_digest(f, 'sha256').hexdigest()
    recorded = (assets / (deb.name + '.sha256')).read_text().split()[0]
    assert checksum == recorded, 'Deb checksum mismatch'
    out.mkdir(parents=True, exist_ok=True)
    license_data = (ROOT / 'LICENSE').read_bytes()
    (out / 'LICENSE').write_bytes(license_data)
    template = (ROOT / 'packaging/aur/PKGBUILD.in').read_text()
    (out / 'PKGBUILD').write_text(template.replace('@VERSION@', version).replace('@SHA256@', checksum).replace('@LICENSE_SHA256@', hashlib.sha256(license_data).hexdigest()))
    return deb

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=pathlib.Path, default=ROOT/'release-assets')
    parser.add_argument('--out', type=pathlib.Path, default=ROOT/'build-assets/aur')
    parser.add_argument('--archive', action='store_true')
    args = parser.parse_args()
    if args.archive:
        assert (args.out/'.SRCINFO').is_file(), 'Generate and validate .SRCINFO with makepkg first'
        version = json.loads((ROOT/'package.json').read_text())['version']
        archive = args.assets/f'OmaScribe_{version}_Arch-PKGBUILD.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for name in ['PKGBUILD', '.SRCINFO', 'LICENSE']:
                tar.add(args.out/name, arcname=f'omascribe-bin/{name}')
        with archive.open('rb') as f:
            checksum = hashlib.file_digest(f, 'sha256').hexdigest()
        (args.assets/(archive.name+'.sha256')).write_text(f'{checksum}  {archive.name}\n')
    else:
        deb = generate(args.assets, args.out)
        # The release is not public yet; makepkg validates this local copy against the pinned hash.
        shutil.copy2(deb, args.out/deb.name)
        print('Prepared pinned Arch package recipe:', args.out)
