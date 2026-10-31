#!/usr/bin/env python3
"""Build and validate signed runtime-only updates, retaining models outside the app."""
import argparse, hashlib, json, os, pathlib, platform, shutil, subprocess, tempfile
from runtime_inventory import assert_base_names, assert_base_runtime
ROOT=pathlib.Path(__file__).resolve().parents[1]
LABELS={'aarch64-apple-darwin':'macOS-AppleSilicon','x86_64-apple-darwin':'macOS-Intel','x86_64-pc-windows-msvc':'Windows-x64','x86_64-unknown-linux-gnu':'Linux-x64'}
def run(*args,**kwargs): return subprocess.run([str(a) for a in args],check=True,**kwargs)
def assert_runtime_inventory(listing):
    paths = [line.removeprefix("Path = ").strip() for line in listing.splitlines() if line.startswith("Path = ")]
    assert len(paths) > 1, "Unable to inspect NSIS contents"
    assert_base_names(paths[1:])
    for name in paths[1:]:
        parts = pathlib.PureWindowsPath(name).parts
        assert not any(p.lower() == "models" for p in parts), "Runtime payload contains model directory"
        assert pathlib.PureWindowsPath(name).suffix.lower() not in {".gguf", ".onnx"}, "Runtime payload contains model weights"

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--target',required=True);args=parser.parse_args()
    assert os.environ.get('GITHUB_ACTIONS')=='true','Build on the matching native release runner'
    windows=platform.system()=='Windows';linux=platform.system()=='Linux';suffix='.exe' if windows else ''
    assets=ROOT/'build-assets/native'; slim=ROOT/'build-assets/runtime-only'
    shutil.rmtree(slim,ignore_errors=True)
    shutil.copytree(assets,slim,ignore=shutil.ignore_patterns('models'))
    assert_base_runtime(slim)
    assert not (slim/'models').exists()
    config=json.loads((ROOT/'src-tauri/tauri.release.conf.json').read_text())
    config['bundle']['resources']={'../build-assets/runtime-only/':'native/'}
    if windows:
        config['bundle']['windows']['webviewInstallMode']={'type':'skip'} # existing installation already has WebView2
        config['bundle']['windows']['nsis']['installerHooks']=str(ROOT/'packaging/windows/runtime-hooks.nsh')
    path=ROOT/'src-tauri/tauri.runtime.conf.json';path.write_text(json.dumps(config,indent=2))
    bundle=ROOT/'target'/args.target/'release/bundle';full=bundle.with_name('bundle-full')
    if full.exists(): shutil.rmtree(full)
    bundle.rename(full)
    bundles='nsis' if windows else 'appimage' if linux else 'app'
    run('npx.cmd' if windows else 'npx','tauri','bundle','--target',args.target,'--config',path,'--bundles',bundles)
    version=json.loads((ROOT/'package.json').read_text())['version'];label=LABELS[args.target]
    payload=next((bundle/('nsis' if windows else 'appimage' if linux else 'macos')).glob('*.exe' if windows else '*.AppImage' if linux else '*.app.tar.gz'))
    name=f'OmaScribe_{version}_{label}-runtime'+('.exe' if windows else '.AppImage' if linux else '.app.tar.gz')
    out=ROOT/'release-assets';dest=out/name;shutil.copy2(payload,dest)
    assert 1_000_000<dest.stat().st_size<350_000_000,'Runtime update unexpectedly contains model weights'
    shutil.copy2(pathlib.Path(str(payload)+'.sig'),out/(name+'.sig'))
    results=ROOT/'ci-results';key=results/'updater.pub'
    full_cli=assets/'runtime'/('omascribe-cli'+suffix)
    run(full_cli,'verify-update',dest,out/(name+'.sig'),key,version,timeout=180)
    (out/(name+'.verified.json')).write_text(json.dumps({'verified':True,'version':version,'file':name}))
    with dest.open('rb') as f: digest=hashlib.file_digest(f,'sha256').hexdigest()
    (out/(name+'.sha256')).write_text(digest+'  '+name+'\n',newline='\n')
    # Exercise migration from a full installation, then discover models with only the new runtime.
    with tempfile.TemporaryDirectory(prefix='omascribe-runtime-') as folder:
        folder=pathlib.Path(folder)
        env=dict(os.environ,OMASCRIBE_DATA_DIR=str(folder/'data'),OMASCRIBE_MODELS_DIR=str(assets/'models'),OMASCRIBE_RUNTIME_DIR=str(assets/'runtime'),PYTHONUTF8='1')
        run(full_cli,'preserve-models',env=env,timeout=120)
        if windows:
            sevenzip = shutil.which('7z') or str(pathlib.Path(os.environ['ProgramFiles'])/'7-Zip/7z.exe')
            listing = subprocess.check_output([sevenzip,'l','-slt',str(payload)],text=True,encoding='utf-8',errors='replace',timeout=120)
            assert_runtime_inventory(listing)
            (results/'runtime-payload-inventory.txt').write_text(listing,encoding='utf-8')
            # Upgrade the full install exercised by native-smoke, matching NSIS's registered path.
            installed=pathlib.Path(os.environ['RUNNER_TEMP'])/'omascribe-installed'
            native=installed/'native'
            # Remove one owned test file; the updater must actually reinstall it.
            installed_cli=native/'runtime/omascribe-cli.exe'
            # Windows runners keep RUNNER_TEMP on D: and Python's temp directory
            # on C:. A cross-volume rename fails before the updater is exercised.
            shutil.move(installed_cli,folder/'previous-cli.exe')
            run(payload,'/S','/UPDATE',f'/D={installed}',timeout=600)
            assert installed_cli.is_file(), 'Runtime installer did not apply its files'
            assert hashlib.sha256(installed_cli.read_bytes()).digest() == hashlib.sha256(full_cli.read_bytes()).digest(), 'Installed runtime CLI differs from the signed package source'
            legacy_models=native/'models'
            if legacy_models.exists():
                # NSIS /UPDATE deliberately retains old resources. Hide this fallback to
                # prove the migrated store is sufficient, without deleting any user data.
                shutil.move(legacy_models,folder/'legacy-bundled-models')
        elif linux:
            payload.chmod(0o755)
            with (results/'runtime-extract.log').open('wb') as log: run(payload,'--appimage-extract',cwd=folder,stdout=log,stderr=log,timeout=180)
            native=folder/'squashfs-root/usr/lib/OmaScribe/native'
        else:
            app=next((bundle/'macos').glob('*.app'));run('codesign','--verify','--deep','--strict',app);native=app/'Contents/Resources/native'
        assert not (native/'models').exists(),'Update payload includes models'
        env.pop('OMASCRIBE_MODELS_DIR');env.pop('OMASCRIBE_RUNTIME_DIR')
        cli=native/'runtime'/('omascribe-cli'+suffix)
        diag=json.loads(subprocess.check_output([str(cli),'diagnose'],env=env,timeout=30))
        assert all(diag[k] for k in ['asr_ready','llm_ready','worker_ready','server_ready']),diag
        env['OMASCRIBE_TEST_MODEL_STORE']=str(folder/'data/model-store')
        env['OMASCRIBE_RUNTIME_DIR']=str(native/'runtime')
        # This exercises ASR hotword decoding and local minutes with migrated models.
        run('python',ROOT/'tests/multitake.py','--cli',cli,'--output',results/'runtime-multitake.json',env=dict(env,OMASCRIBE_TEST_WAV=str(ROOT/'build-assets/public-sample.wav')),timeout=600)
        run('python',ROOT/'tests/evaluate.py','--cli',cli,'--case','zh_connected_minutes','--output',results/'runtime-minutes.json',env=env,timeout=700)
        if windows:
            if (folder/'legacy-bundled-models').exists(): shutil.move(folder/'legacy-bundled-models',native/'models')
            user_marker=folder/'data/meetings/uninstall-preservation.txt'
            user_marker.parent.mkdir(exist_ok=True)
            user_marker.write_text('fictional user-data preservation marker')
            run(installed/'uninstall.exe','/S',timeout=180)
            import time
            for _ in range(300):
                if not (native/'models').exists(): break
                time.sleep(.2)
            assert not (native/'models').exists(), 'Uninstall left old bundled model resources'
            assert (folder/'data/model-store/zipformer-small/tokens.txt').is_file(), 'Uninstall deleted migrated user models'
            assert user_marker.read_text() == 'fictional user-data preservation marker', 'Uninstall changed user recordings/notes directory'
    (results/'runtime-update.json').write_text(json.dumps({'passed':True,'file':name,'bytes':dest.stat().st_size,'models_reused':True},indent=2))
if __name__=='__main__':main()
