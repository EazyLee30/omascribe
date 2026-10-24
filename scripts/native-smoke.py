#!/usr/bin/env python3
"""Exercise installed native assets with public speech and fictional meeting evidence."""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("This installed-package check runs on GitHub Actions only")
    windows = platform.system() == "Windows"
    suffix = ".exe" if windows else ""
    bundle = ROOT / "target" / args.target / "release/bundle"
    if windows:
        installer = next((bundle / "nsis").glob("*.exe"))
        installed = pathlib.Path(os.environ["RUNNER_TEMP"]) / "omascribe-installed"
        subprocess.run(
            [str(installer), "/S", f"/D={installed}"], check=True, timeout=600
        )
        assets = installed / "native"
        app = installed / "omascribe.exe"
    else:
        installer = next((bundle / "dmg").glob("*.dmg"))
        application = next((bundle / "macos").glob("*.app"))
        subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", str(application)], check=True
        )
        assets = application / "Contents/Resources/native"
        app = application / "Contents/MacOS/omascribe"
    assert app.is_file(), app
    assert assets.is_dir(), assets
    runtime = assets / "runtime"
    cli = runtime / ("omascribe-cli" + suffix)
    results = ROOT / "ci-results"
    results.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="omascribe-install-") as data:
        env = dict(
            os.environ,
            OMASCRIBE_DATA_DIR=data,
            OMASCRIBE_MODELS_DIR=str(assets / "models"),
            OMASCRIBE_RUNTIME_DIR=str(runtime),
            OMASCRIBE_TEST_WAV=str(ROOT / "build-assets/public-sample.wav"),
            PYTHONUTF8="1",
        )
        # The installed CLI and GUI share asset resolution. Do not mask a broken bundle path.
        discovery_env = dict(env)
        discovery_env.pop("OMASCRIBE_MODELS_DIR", None)
        discovery_env.pop("OMASCRIBE_RUNTIME_DIR", None)
        discovered = json.loads(
            subprocess.check_output(
                [str(cli), "diagnose"], env=discovery_env, timeout=30
            )
        )
        assert all(
            discovered[key]
            for key in ["asr_ready", "llm_ready", "worker_ready", "server_ready"]
        ), discovered
        subprocess.run(
            ["python", str(ROOT / "scripts/setup.py"), "--verify"],
            env=env,
            check=True,
            timeout=120,
        )
        diagnose = json.loads(
            subprocess.check_output([str(cli), "diagnose"], env=env, timeout=30)
        )
        for key in [
            "asr_ready",
            "llm_ready",
            "recorder_ready",
            "worker_ready",
            "server_ready",
        ]:
            assert diagnose[key], (key, diagnose)
        assert diagnose["available_memory_mib"] > 1400, diagnose
        subprocess.run(
            [
                "python",
                str(ROOT / "tests/multitake.py"),
                "--cli",
                str(cli),
                "--output",
                str(results / "multitake.json"),
            ],
            env=env,
            check=True,
            timeout=600,
        )
        for case in ["zh_calendar_citations", "en_owner_and_deadline"]:
            cases = json.loads(
                (ROOT / "tests/fixtures/document-cases.json").read_text(
                    encoding="utf-8"
                )
            )
            assert case in [c["name"] for c in cases], case
            subprocess.run(
                [
                    "python",
                    str(ROOT / "tests/evaluate.py"),
                    "--cli",
                    str(cli),
                    "--case",
                    case,
                    "--output",
                    str(results / (case + ".json")),
                ],
                env=env,
                check=True,
                timeout=700,
            )
        # Smoke launch checks main process survival, not microphone permission or visual quality.
        with (results / "launch.log").open("wb") as log:
            process = subprocess.Popen(
                [str(app)], env=discovery_env, stdout=log, stderr=log
            )
            time.sleep(12)
            try:
                assert process.poll() is None, (
                    f"Desktop application exited: {process.returncode}"
                )
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=30)
    (results / "installed-package.json").write_text(
        json.dumps(
            {
                "passed": True,
                "target": args.target,
                "commit": os.environ["GITHUB_SHA"],
                "checks": [
                    "installed assets",
                    "ASR multi-take and retranscription",
                    "local cited writing",
                    "desktop process launch",
                ],
                "not_tested": [
                    "physical microphone",
                    "system permission prompts",
                    "visual UI",
                    "hour-long thermal endurance",
                ],
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    output = ROOT / "release-assets"
    output.mkdir(exist_ok=True)
    version = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]
    label = {
        "aarch64-apple-darwin": "macOS-AppleSilicon",
        "x86_64-apple-darwin": "macOS-Intel",
        "x86_64-pc-windows-msvc": "Windows-x64",
    }[args.target]
    destination = output / f"OmaScribe_{version}_{label}{installer.suffix}"
    shutil.copy2(installer, destination)
    with destination.open("rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    assert 1_000_000 < destination.stat().st_size < 2_000_000_000
    (output / (destination.name + ".sha256")).write_text(
        digest + "  " + destination.name + "\n", encoding="utf-8"
    )
    shutil.make_archive(
        str(output / f"OmaScribe_{version}_{label}_validation"), "zip", results
    )


if __name__ == "__main__":
    main()
