#!/usr/bin/env python3
"""Build and validate signed runtime-only updates, retaining models outside the app."""
import argparse, hashlib, json, os, pathlib, platform, shutil, subprocess, tempfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
LABELS={'aarch64-apple-darwin':'macOS-AppleSilicon','x86_64-apple-darwin':'macOS-Intel','x86_64-pc-windows-msvc':'Windows-x64','x86_64-unknown-linux-gnu':'Linux-x64'}
def run(*args,**kwargs): return subprocess.run([str(a) for a in args],check=True,**kwargs)
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--target',required=True);args=parser.parse_args()
    assert os.environ.get('GITHUB_ACTIONS')=='true','Build on the matching native release runner'
    windows=platform.system()=='Windows';linux=platform.system()=='Linux';suffix='.exe' if windows else ''
    assets=ROOT/'build-assets/native'; slim=ROOT/'build-assets/runtime-only'
    shutil.rmtree(slim,ignore_errors=True)
    shutil.copytree(assets,slim,ignore=shutil.ignore_patterns('models'))
    assert not (slim/'models').exists()
    config=json.loads((ROOT/'src-tauri/tauri.release.conf.json').read_text())
    config['bundle']['resources']={'../build-assets/runtime-only/':'native/'}
    if windows: config['bundle']['windows']['webviewInstallMode']={'type':'skip'} # existing installation already has WebView2
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
            # Upgrade the full install exercised by native-smoke, matching NSIS's registered path.
            installed=pathlib.Path(os.environ['RUNNER_TEMP'])/'omascribe-installed'
            run(payload,'/S',f'/D={installed}',timeout=600);native=installed/'native'
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
    (results/'runtime-update.json').write_text(json.dumps({'passed':True,'file':name,'bytes':dest.stat().st_size,'models_reused':True},indent=2))
if __name__=='__main__':main()
