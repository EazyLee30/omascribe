#!/usr/bin/env python3
"""Native release preparation. Runs ONLY on GitHub-hosted macOS, Windows and Linux runners."""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / "models.lock.json").read_text(encoding="utf-8"))
ASSETS = ROOT / "build-assets/native"
RUNTIME = ASSETS / "runtime"
WINDOWS = platform.system() == "Windows"
MACOS = platform.system() == "Darwin"
SUFFIX = ".exe" if WINDOWS else ""


def run(*args, **kwargs):
    subprocess.run([str(a) for a in args], check=True, **kwargs)


def download(url, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "OmaScribe-release"})
    with (
        urllib.request.urlopen(request, timeout=120) as source,
        target.open("wb") as output,
    ):
        shutil.copyfileobj(source, output)


def prepare(target):
    metal = target == "aarch64-apple-darwin"
    RUNTIME.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, OMASCRIBE_MODELS_DIR=str(ASSETS / "models"))
    run("python", ROOT / "scripts/setup.py", "--models-only", env=env)
    # Reuse the verified ASR archive for the public smoke sample, never ship test recordings.
    archive = (
        pathlib.Path.home()
        / ".cache/omascribe/downloads"
        / (LOCK["asr"]["name"] + ".tar.bz2")
    )
    if not archive.exists():
        download(LOCK["asr"]["url"], archive)
    with archive.open("rb") as f:
        assert hashlib.file_digest(f, "sha256").hexdigest() == LOCK["asr"]["sha256"]
    sample = ROOT / "build-assets/public-sample.wav"
    with tarfile.open(archive) as tar:
        with (
            tar.extractfile(LOCK["asr"]["name"] + "/test_wavs/0.wav") as source,
            sample.open("wb") as output,
        ):
            shutil.copyfileobj(source, output)

    llama = ROOT / "build-assets/llama.cpp"
    llama.mkdir(exist_ok=True)
    if not (llama / ".git").exists():
        run("git", "init", llama)
    run(
        "git",
        "-C",
        llama,
        "fetch",
        "--depth",
        "1",
        LOCK["llama_cpp"]["url"],
        LOCK["llama_cpp"]["revision"],
    )
    run("git", "-C", llama, "checkout", "--detach", "FETCH_HEAD")
    runtime_stamp = RUNTIME / "llama/build-info.json"
    expected = {"revision": LOCK["llama_cpp"]["revision"], "metal": metal, "format": 1}
    if not ((RUNTIME / "llama" / ("llama-server" + SUFFIX)).is_file()
            and runtime_stamp.is_file() and json.loads(runtime_stamp.read_text()) == expected):
        build = llama / "build-native"
        flags = [
            "-DCMAKE_BUILD_TYPE=Release",
            "-DBUILD_SHARED_LIBS=OFF",
            "-DGGML_BACKEND_DL=OFF",
            "-DGGML_NATIVE=OFF",
            "-DGGML_METAL=" + ("ON" if metal else "OFF"),
            "-DGGML_METAL_EMBED_LIBRARY=" + ("ON" if metal else "OFF"),
            "-DGGML_ACCELERATE=" + ("ON" if metal else "OFF"),
            "-DGGML_BLAS=OFF",
            "-DGGML_OPENMP=OFF",
            "-DGGML_CUDA=OFF",
            "-DLLAMA_OPENSSL=OFF",
            "-DLLAMA_CURL=OFF",
            "-DLLAMA_BUILD_TESTS=OFF",
            "-DLLAMA_BUILD_EXAMPLES=OFF",
            "-DLLAMA_BUILD_UI=OFF",
            "-DLLAMA_USE_PREBUILT_UI=OFF",
            "-DLLAMA_BUILD_SERVER=ON",
        ]
        if target.startswith("x86_64"):
            flags += [
                "-DGGML_AVX=ON",
                "-DGGML_AVX2=ON",
                "-DGGML_FMA=ON",
                "-DGGML_F16C=ON",
                "-DGGML_AVX512=OFF",
            ]
        if WINDOWS:
            flags += ["-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DLLAMA_STATIC=ON"]
        elif MACOS:
            flags += ["-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0"]
        run("cmake", "-S", llama, "-B", build, *flags)
        run(
            "cmake",
            "--build",
            build,
            "--config",
            "Release",
            "--target",
            "llama-server",
            "--parallel",
            "3",
        )
        server = next(build.rglob("llama-server" + SUFFIX))
        (RUNTIME / "llama").mkdir(exist_ok=True)
        shutil.copy2(server, RUNTIME / "llama" / server.name)
        runtime_stamp.write_text(json.dumps(expected, indent=2) + "\n")
    run(
        "cargo",
        "build",
        "--release",
        "--locked",
        "--target",
        target,
        "-p",
        "omascribe-core",
        "-p",
        "omascribe-asr",
    )
    for name in ["omascribe-cli", "omascribe-asr"]:
        shutil.copy2(
            ROOT / "target" / target / "release" / (name + SUFFIX),
            RUNTIME / (name + SUFFIX),
        )
    if not WINDOWS:
        for binary in [
            RUNTIME / "omascribe-cli",
            RUNTIME / "omascribe-asr",
            RUNTIME / "llama/llama-server",
        ]:
            binary.chmod(0o755)
            if MACOS:
                run("codesign", "--force", "--sign", "-", binary)

    licenses = ASSETS / "licenses"
    licenses.mkdir(exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", licenses / "OmaScribe-MIT.txt")
    shutil.copy2(ROOT / "models.lock.json", ASSETS / "models.lock.json")
    shutil.copy2(llama / "LICENSE", licenses / "llama.cpp-MIT.txt")
    for name, url in {
        "Apache-2.0.txt": "https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.13.8/LICENSE",
        "onnxruntime-MIT.txt": "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.23.2/LICENSE",
        "zipformer-model-card.md": "https://huggingface.co/csukuangfj/k2fsa-zipformer-bilingual-zh-en-t/raw/main/README.md",
        "Qwen3-model-card.md": "https://huggingface.co/Qwen/Qwen3-1.7B/raw/main/README.md",
    }.items():
        download(url, licenses / name)
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT
        )
    )
    inventory = []
    for package in metadata["packages"]:
        inventory.append(
            {k: package.get(k) for k in ["name", "version", "license", "repository"]}
        )
        folder = pathlib.Path(package["manifest_path"]).parent
        for source in folder.iterdir():
            if source.is_file() and source.name.lower().startswith(
                ("license", "copying", "notice")
            ):
                destination = (
                    licenses
                    / "rust"
                    / (package["name"] + "-" + package["version"])
                    / source.name
                )
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
    (licenses / "rust-packages.json").write_text(
        json.dumps(inventory, indent=2), encoding="utf-8"
    )
    for source in (ROOT / "node_modules").rglob("*"):
        if source.is_file() and source.name.lower().startswith(
            ("license", "copying", "notice")
        ):
            destination = (
                licenses / "javascript" / source.relative_to(ROOT / "node_modules")
            )
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
    (ASSETS / "build-info.json").write_text(
        json.dumps(
            {
                "commit": os.environ["GITHUB_SHA"],
                "target": target,
                "runner": os.environ.get("RUNNER_NAME"),
                "llama_cpp": LOCK["llama_cpp"]["revision"],
                "asr": LOCK["asr"]["name"],
                "llm": LOCK["llm"]["name"],
                "gpu": metal,
                "inference_backends": ["cpu", "metal"] if metal else ["cpu"],
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    config = {
        "bundle": {
            "active": True,
            "category": "Productivity",
            "shortDescription": "Offline meeting notes, in your own words.",
            "resources": {"../build-assets/native/": "native/"},
            "icon": [
                "icons/32x32.png",
                "icons/128x128.png",
                "icons/128x128@2x.png",
                "icons/icon.icns",
                "icons/icon.ico",
            ],
        }
    }
    if WINDOWS:
        config["bundle"]["windows"] = {
            "webviewInstallMode": {"type": "offlineInstaller"},
            "nsis": {
                "installMode": "currentUser",
                "languages": ["English", "SimpChinese", "TradChinese"],
                "displayLanguageSelector": True,
            },
        }
    elif MACOS:
        config["app"] = {"macOSPrivateApi": True}
        config["bundle"]["macOS"] = {
            "minimumSystemVersion": "11.0",
            "signingIdentity": "-",
            "entitlements": "Entitlements.plist",
            "infoPlist": "Info.plist",
        }
    else:
        config["bundle"]["linux"] = {"deb": {"depends": ["pipewire-bin", "ffmpeg", "libgtk-layer-shell0"]}}
    (ROOT / "src-tauri/tauri.release.conf.json").write_text(
        json.dumps(config, indent=2), encoding="utf-8"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true" or platform.system() not in [
        "Darwin",
        "Linux",
        "Windows",
    ]:
        raise SystemExit(
            "Native packages must be built on matching GitHub Actions runners."
        )
    hosts = {"aarch64-apple-darwin": "Darwin", "x86_64-apple-darwin": "Darwin", "x86_64-pc-windows-msvc": "Windows", "x86_64-unknown-linux-gnu": "Linux"}
    if hosts.get(args.target) != platform.system():
        raise SystemExit("The release target must match the runner OS")
    prepare(args.target)
