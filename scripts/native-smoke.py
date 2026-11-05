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


class DocumentCaseBudgetExpired(RuntimeError):
    pass


def document_result(output, name, fixture_sha256, returncode):
    report = json.loads(output.read_text(encoding="utf-8"))
    assert isinstance(report, dict) and report.get("schema") == 3, "Invalid document report schema"
    assert report.get("fixture_sha256") == fixture_sha256, "Document fixture changed"
    assert report.get("fixture_cases") == [name], "Wrong document fixture selection"
    if returncode == 1 and report.get("passed") is False and report.get("failure_kind") == "budget":
        raise DocumentCaseBudgetExpired("Document evaluator exhausted its command budget")
    rows = report.get("cases")
    assert isinstance(rows, list) and len(rows) == 1, "Incomplete document report"
    row = rows[0]
    assert isinstance(row, dict) and row.get("case") == name, "Wrong document case"
    errors = row.get("errors")
    assert isinstance(errors, list) and all(isinstance(e, str) and e for e in errors), "Invalid case errors"
    if returncode == 1 and report.get("passed") is False and row.get("passed") is False \
            and errors and row.get("failure_kind") == "budget":
        raise DocumentCaseBudgetExpired("Document case exhausted its command budget")
    runs = row.get("runs")
    assert isinstance(runs, list) and runs, "Missing document execution evidence"
    assert all(isinstance(run, dict) and isinstance(run.get("process"), dict)
               and run["process"].get("code") == 0
               and run["process"].get("failure") is None for run in runs), "Document execution failed"
    if returncode == 0:
        assert report.get("passed") is True and row.get("passed") is True, "Exit success disagrees with report"
        assert not errors and row.get("failure_kind") is None, "Failed case reported success"
        return "passed"
    assert returncode == 1, f"Document evaluator exited {returncode}"
    assert report.get("passed") is False and row.get("passed") is False, "Exit failure disagrees with report"
    assert errors and row.get("failure_kind") == "expectation", "Document execution or report failed"
    return "expectation_failed"


def run_document_cases(cli, env, results, cases, *, target=None, budget_seconds=1200,
                       fixture_path=None):
    """Collect semantic failures once; incomplete execution still stops the gate."""
    fixture_path = fixture_path or ROOT / "tests/fixtures/document-cases.json"
    fixture_bytes = fixture_path.read_bytes()
    assert json.loads(fixture_bytes) == cases, "Must run the complete current document fixture"
    names = [c["name"] for c in cases]
    assert names and len(set(names)) == len(names), "Empty or duplicate document cases"
    assert all(isinstance(name, str) and pathlib.Path(name).name == name
               and name not in (".", "..") for name in names), "Invalid document case name"
    assert 0 < budget_seconds <= 1200, "Invalid document collection budget"
    fixture_sha256 = hashlib.sha256(fixture_bytes).hexdigest()
    results.mkdir(parents=True, exist_ok=True)
    summary_path = results / "document-suite.json"
    started = time.monotonic()
    summary = dict(schema=1, passed=False, complete=False, target=target,
                   commit=env.get("GITHUB_SHA"), fixture_sha256=fixture_sha256,
                   planned_cases=names, cases=[], unattempted_cases=names,
                   budget_seconds=budget_seconds, case_timeout_seconds=700,
                   stopped_reason=None)

    def save():
        attempted = {row["case"] for row in summary["cases"]}
        summary["unattempted_cases"] = [name for name in names if name not in attempted]
        elapsed = time.monotonic() - started
        summary["seconds"] = round(elapsed, 3)
        if elapsed >= budget_seconds and summary["stopped_reason"] is None:
            summary["stopped_reason"] = "collection_budget"
        summary["complete"] = not summary["unattempted_cases"] and all(
            row["state"] in ("passed", "expectation_failed") for row in summary["cases"])
        summary["passed"] = summary["complete"] and summary["stopped_reason"] is None and all(
            row["state"] == "passed" for row in summary["cases"])
        temporary = summary_path.with_suffix(".json.part")
        temporary.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        temporary.replace(summary_path)

    save()
    for name in names:
        remaining = budget_seconds - (time.monotonic() - started)
        if remaining <= 0:
            summary["stopped_reason"] = "collection_budget"
            break
        output = results / (name + ".json")
        if output.exists():
            summary["stopped_reason"] = "preexisting_case_report"
            break
        case_started = time.monotonic()
        case_budget = min(700, remaining)
        row = dict(case=name, report=output.name, artifacts=output.with_suffix(".artifacts").name,
                   state="execution_failed", returncode=None)
        summary["cases"].append(row)
        try:
            process = subprocess.run(
                ["python", str(ROOT / "tests/evaluate.py"), "--cli", str(cli),
                 "--case", name, "--output", str(output), "--budget-seconds", str(case_budget)],
                env=env, check=False, timeout=case_budget + 30,
            )
            row["returncode"] = process.returncode
            row["state"] = document_result(output, name, fixture_sha256, process.returncode)
        except DocumentCaseBudgetExpired as error:
            row["state"] = "budget_expired"
            row["error"] = str(error)
            summary["stopped_reason"] = "case_budget"
        except subprocess.TimeoutExpired:
            row["state"] = "timed_out"
            summary["stopped_reason"] = "case_timeout"
        except (OSError, ValueError, AssertionError, TypeError) as error:
            row["error"] = f"{type(error).__name__}: {error}"
            summary["stopped_reason"] = "execution_or_report_failure"
        row["seconds"] = round(time.monotonic() - case_started, 3)
        save()
        if summary["stopped_reason"]:
            break
    save()
    # Include final report work in admission, even after the last case passed.
    if summary["passed"] and time.monotonic() - started >= budget_seconds:
        summary["stopped_reason"] = "collection_budget"
        save()
    if not summary["passed"]:
        raise RuntimeError(f"Document release gate failed; see {summary_path}")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("This installed-package check runs on GitHub Actions only")
    windows = platform.system() == "Windows"
    linux = platform.system() == "Linux"
    suffix = ".exe" if windows else ""
    linux_packages = []
    product_name = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["productName"]
    results = ROOT / "ci-results"
    results.mkdir(exist_ok=True)
    bundle = ROOT / "target" / args.target / "release/bundle"
    if windows:
        installer = next((bundle / "nsis").glob("*.exe"))
        installed = pathlib.Path(os.environ["RUNNER_TEMP"]) / "omascribe-installed"
        subprocess.run(
            [str(installer), "/S", f"/D={installed}"], check=True, timeout=600
        )
        assets = installed / "native"
        app = installed / "omascribe.exe"
    elif linux:
        deb = next((bundle / "deb").glob("*.deb"))
        appimage = next((bundle / "appimage").glob("*.AppImage"))
        dependencies = subprocess.check_output(["dpkg-deb", "-f", str(deb), "Depends"], text=True).strip()
        assert all(dep in dependencies for dep in ["pipewire-bin", "ffmpeg", "libwebkit2gtk-4.1-0", "libgtk-3-0", "desktop-file-utils", "xdg-utils"]), dependencies
        (results / "linux-package-dependencies.json").write_text(json.dumps({"depends":dependencies}), encoding="utf-8")
        extracted = pathlib.Path(os.environ["RUNNER_TEMP"]) / "omascribe-linux"
        extracted.mkdir(exist_ok=True)
        subprocess.run(["dpkg-deb", "-x", str(deb), str(extracted / "deb")], check=True, timeout=180)
        appimage.chmod(0o755)
        with (ROOT / "build-assets/appimage-extract.log").open("wb") as log:
            subprocess.run([str(appimage), "--appimage-extract"], cwd=extracted, stdout=log, stderr=log, check=True, timeout=180)
        for source, folder in [(deb, extracted / "deb"), (appimage, extracted / "squashfs-root")]:
            linux_packages.append((source, folder / "usr/lib" / product_name / "native", folder / "usr/bin/omascribe", folder))
        installer, assets, app, _ = linux_packages[0]
    else:
        installer = next((bundle / "dmg").glob("*.dmg"))
        mount = pathlib.Path(os.environ["RUNNER_TEMP"]) / "omascribe-dmg"
        mount.mkdir(exist_ok=True)
        subprocess.run(
            [
                "hdiutil",
                "attach",
                "-nobrowse",
                "-readonly",
                "-mountpoint",
                str(mount),
                str(installer),
            ],
            check=True,
            timeout=120,
        )
        application = (
            pathlib.Path(os.environ["RUNNER_TEMP"])
            / "omascribe-installed/OmaScribe.app"
        )
        application.parent.mkdir(exist_ok=True)
        try:
            source = next(mount.glob("*.app"))
            subprocess.run(
                ["ditto", str(source), str(application)], check=True, timeout=120
            )
        finally:
            subprocess.run(["hdiutil", "detach", str(mount)], check=True, timeout=60)
        subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", str(application)], check=True
        )
        assets = application / "Contents/Resources/native"
        app = application / "Contents/MacOS/omascribe"
    assert app.is_file(), app
    assert assets.is_dir(), (str(assets), [str(p) for p in assets.parent.parent.iterdir()] if assets.parent.parent.is_dir() else [])
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
        if linux:
            # Also verify the AppImage payload independently of the Debian package.
            for source, package_assets, binary, folder in linux_packages:
                assert binary.is_file(), binary
                assert package_assets.is_dir(), package_assets
                package_env = dict(env, WEBKIT_DISABLE_DMABUF_RENDERER="1")
                package_env.pop("OMASCRIBE_MODELS_DIR", None)
                package_env.pop("OMASCRIBE_RUNTIME_DIR", None)
                discovered_package = json.loads(subprocess.check_output([str(package_assets / "runtime/omascribe-cli"), "diagnose"], env=package_env, timeout=30))
                assert all(discovered_package[k] for k in ["asr_ready", "llm_ready", "worker_ready", "server_ready"]), discovered_package
                # Probe from the GUI executable directory, not only the nested runtime folder.
                probe = binary.parent / "omascribe-resource-check"
                shutil.copy2(package_assets / "runtime/omascribe-cli", probe)
                try:
                    gui_discovery = json.loads(subprocess.check_output([str(probe), "diagnose"], env=package_env, timeout=30))
                    assert all(gui_discovery[k] for k in ["asr_ready", "llm_ready", "worker_ready", "server_ready"]), gui_discovery
                finally:
                    probe.unlink()
                launcher = folder / "AppRun" if source.suffix == ".AppImage" else binary
                if source.suffix == ".deb":
                    # A minimal desktop without link-registration tools must still start.
                    empty_path = pathlib.Path(data) / "no-desktop-tools"
                    empty_path.mkdir()
                    fallback_log = results / "deb-no-desktop-tools-launch.log"
                    with fallback_log.open("wb") as log:
                        process = subprocess.Popen([str(binary)], env=dict(package_env, PATH=str(empty_path)), stdout=log, stderr=log, start_new_session=True)
                        try:
                            time.sleep(12)
                            assert process.poll() is None, f"Offline workspace failed without desktop tools: {process.returncode}"
                        finally:
                            import signal
                            try:
                                os.killpg(process.pid, signal.SIGTERM)
                            except ProcessLookupError:
                                pass
                            process.wait(timeout=30)
                    assert "Configuration link registration unavailable" in fallback_log.read_text(encoding="utf-8", errors="replace")
                with (results / (source.suffix.lstrip(".") + "-launch.log")).open("wb") as log:
                    process = subprocess.Popen([str(launcher)], env=package_env, stdout=log, stderr=log, start_new_session=True)
                    try:
                        time.sleep(12)
                        assert process.poll() is None, f"{source.name} exited: {process.returncode}"
                    finally:
                        import signal
                        try:
                            os.killpg(process.pid, signal.SIGTERM)
                        except ProcessLookupError:
                            pass
                        process.wait(timeout=30)
                launch_log = results / (source.suffix.lstrip(".") + "-launch.log")
                assert "Configuration link registration unavailable" not in launch_log.read_text(encoding="utf-8", errors="replace"), launch_log
            env["WEBKIT_DISABLE_DMABUF_RENDERER"] = "1"
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
        cases = json.loads(
            (ROOT / "tests/fixtures/document-cases.json").read_text(encoding="utf-8")
        )
        run_document_cases(cli, env, results, cases, target=args.target)
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
        "x86_64-unknown-linux-gnu": "Linux-x64",
    }[args.target]
    installers = [item[0] for item in linux_packages] if linux else [installer]
    for package in installers:
        destination = output / f"OmaScribe_{version}_{label}{package.suffix}"
        shutil.copy2(package, destination)
        with destination.open("rb") as f:
            digest = hashlib.file_digest(f, "sha256").hexdigest()
        assert 1_000_000 < destination.stat().st_size < 2_000_000_000
        (output / (destination.name + ".sha256")).write_text(
            digest + "  " + destination.name + "\n", encoding="utf-8", newline="\n"
        )
    # Verify each updater payload and its signed version before publishing metadata.
    if windows:
        updater = installer
        updater_name = f"OmaScribe_{version}_{label}.exe"
    elif linux:
        updater = appimage
        updater_name = f"OmaScribe_{version}_{label}.AppImage"
    else:
        updater = next((bundle / "macos").glob("*.app.tar.gz"))
        updater_name = f"OmaScribe_{version}_{label}.app.tar.gz"
    destination = output / updater_name
    if not destination.exists():
        shutil.copy2(updater, destination)
        with destination.open("rb") as f:
            digest = hashlib.file_digest(f, "sha256").hexdigest()
        (output / (updater_name + ".sha256")).write_text(digest + "  " + updater_name + "\n", encoding="utf-8", newline="\n")
    sig = output / (updater_name + ".sig")
    shutil.copy2(pathlib.Path(str(updater) + ".sig"), sig)
    key_file = results / "updater.pub"
    key_file.write_text(json.loads((ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["plugins"]["updater"]["pubkey"], encoding="utf-8")
    subprocess.run([str(cli), "verify-update", str(destination), str(sig), str(key_file), version], check=True, timeout=180)
    # Corrupted bytes and relabeled versions must be rejected by the same verifier.
    corrupt = results / "corrupt-update.bin"
    corrupt.write_bytes(b"not the signed installer")
    for payload, expected_version in [(corrupt, version), (destination, "99.0.0")]:
        result = subprocess.run([str(cli), "verify-update", str(payload), str(sig), str(key_file), expected_version], capture_output=True, timeout=180)
        assert result.returncode != 0, "Invalid update was accepted"
    (output / (updater_name + ".verified.json")).write_text(json.dumps({"verified": True, "version": version, "file": updater_name}), encoding="utf-8")
    shutil.make_archive(
        str(output / f"OmaScribe_{version}_{label}_validation"), "zip", results
    )


if __name__ == "__main__":
    main()
