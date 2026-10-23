#!/usr/bin/env python3
"""Pinned, checksum-verified offline assets. Network access is setup-only."""

import argparse, hashlib, json, os, pathlib, shutil, subprocess, tarfile, urllib.request

REPO = pathlib.Path(__file__).resolve().parents[1]
LOCK = json.loads((REPO / "models.lock.json").read_text())
DATA = pathlib.Path(
    os.environ.get(
        "OMASCRIBE_DATA_DIR",
        str(
            pathlib.Path(
                os.environ.get(
                    "XDG_DATA_HOME", str(pathlib.Path.home() / ".local/share")
                )
            )
            / "omascribe"
        ),
    )
)
MODELS = pathlib.Path(os.environ.get("OMASCRIBE_MODELS_DIR", str(DATA / "models")))
RUNTIME = pathlib.Path(os.environ.get("OMASCRIBE_RUNTIME_DIR", str(DATA / "runtime")))
CACHE = (
    pathlib.Path(os.environ.get("XDG_CACHE_HOME", str(pathlib.Path.home() / ".cache")))
    / "omascribe"
)


def valid(path, digest):
    if not path.is_file():
        return False
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest() == digest


def fetch(url, target, digest):
    target.parent.mkdir(parents=True, exist_ok=True)
    if valid(target, digest):
        print("Verified", target.name, flush=True)
        return
    part = target.with_name(target.name + ".part")
    print("Downloading", url, flush=True)
    request = urllib.request.Request(url, headers={"User-Agent": "OmaScribe-setup/0.1"})
    with (
        urllib.request.urlopen(request, timeout=90) as response,
        part.open("wb") as output,
    ):
        shutil.copyfileobj(response, output, 1024 * 1024)
    if not valid(part, digest):
        raise RuntimeError(f"Checksum mismatch: {part}; no unverified model installed")
    part.replace(target)


def models():
    asr = LOCK["asr"]
    folder = MODELS / "zipformer"
    folder.mkdir(parents=True, exist_ok=True)
    if not all(
        valid(folder / name, info["sha256"]) for name, info in asr["files"].items()
    ):
        archive = CACHE / "downloads" / f"{asr['name']}.tar.bz2"
        fetch(asr["url"], archive, asr["sha256"])
        with tarfile.open(archive) as tar:
            # Only exact root model members; never flatten different model variants or extract paths.
            for name, info in asr["files"].items():
                member = tar.getmember(f"{asr['name']}/{name}")
                if not member.isfile():
                    raise RuntimeError(f"Unexpected archive member: {name}")
                target = folder / name
                part = target.with_name(name + ".part")
                with tar.extractfile(member) as source, part.open("wb") as output:
                    shutil.copyfileobj(source, output)
                if not valid(part, info["sha256"]):
                    raise RuntimeError(f"Checksum mismatch: {name}")
                part.replace(target)
    llm = LOCK["llm"]
    fetch(llm["url"], MODELS / llm["name"], llm["sha256"])
    print("Offline models ready:", MODELS, flush=True)


def run(*args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def runtime():
    spec = LOCK["llama_cpp"]
    target = RUNTIME / "llama"
    stamp = target / "build-info.json"
    if (
        stamp.exists()
        and json.loads(stamp.read_text()).get("revision") == spec["revision"]
        and (target / "llama-server").exists()
    ):
        print("Pinned llama.cpp runtime already installed")
        return
    for program in ("git", "cmake", "c++"):
        if not shutil.which(program):
            raise RuntimeError(f"Missing build dependency: {program}")
    source = CACHE / "llama.cpp"
    source.mkdir(parents=True, exist_ok=True)
    if not (source / ".git").exists():
        run("git", "init", str(source))
    run(
        "git", "-C", str(source), "fetch", "--depth", "1", spec["url"], spec["revision"]
    )
    run("git", "-C", str(source), "checkout", "--detach", "FETCH_HEAD")
    build = source / "build-omascribe"
    run(
        "cmake",
        "-S",
        str(source),
        "-B",
        str(build),
        "-DCMAKE_BUILD_TYPE=Release",
        "-DGGML_NATIVE=OFF",
        "-DGGML_AVX=ON",
        "-DGGML_AVX2=ON",
        "-DGGML_FMA=ON",
        "-DGGML_F16C=ON",
        "-DGGML_AVX512=OFF",
        "-DGGML_OPENMP=OFF",
        "-DLLAMA_OPENSSL=OFF",
        "-DLLAMA_BUILD_TESTS=OFF",
        "-DLLAMA_BUILD_EXAMPLES=OFF",
    )
    run(
        "cmake",
        "--build",
        str(build),
        "--target",
        "llama-server",
        "-j",
        os.environ.get("OMASCRIBE_BUILD_JOBS", "1"),
    )
    target.mkdir(parents=True, exist_ok=True)
    for path in [build / "bin/llama-server", *(build / "bin").glob("lib*.so*")]:
        shutil.copy2(path, target / path.name)
    shutil.copy2(source / "LICENSE", target / "LICENSE")
    stamp.write_text(
        json.dumps(
            dict(revision=spec["revision"], cpu="x86-64 AVX2", build="local"), indent=2
        )
        + "\n"
    )
    print("Local inference runtime ready:", target, flush=True)


def overlay():
    spec = LOCK["gtk_layer_shell"]
    target = RUNTIME / "platform"
    stamp = target / "build-info.json"
    if (
        stamp.exists()
        and json.loads(stamp.read_text()).get("revision") == spec["revision"]
        and (target / "libgtk-layer-shell.so.0").exists()
    ):
        print("Pinned caption runtime already installed")
        return
    for program in ("git", "cc", "pkg-config", "wayland-scanner"):
        if not shutil.which(program):
            raise RuntimeError(f"Missing caption build dependency: {program}")
    meson = shutil.which("meson")
    env = os.environ.copy()
    if not meson or not shutil.which("ninja"):
        venv = CACHE / "build-tools"
        if not (venv / "bin/python").exists():
            run("python3", "-m", "venv", str(venv))
        if not (venv / "bin/meson").exists() or not (venv / "bin/ninja").exists():
            run(str(venv / "bin/python"), "-m", "ensurepip")
            run(
                str(venv / "bin/python"),
                "-m",
                "pip",
                "install",
                "meson==1.9.1",
                "ninja==1.13.0",
            )
        meson = str(venv / "bin/meson")
        env["PATH"] = str(venv / "bin") + os.pathsep + env["PATH"]
    source = CACHE / "gtk-layer-shell"
    source.mkdir(parents=True, exist_ok=True)
    if not (source / ".git").exists():
        run("git", "init", str(source))
    run(
        "git", "-C", str(source), "fetch", "--depth", "1", spec["url"], spec["revision"]
    )
    run("git", "-C", str(source), "checkout", "--detach", "FETCH_HEAD")
    build = source / "build-omascribe"
    if not (build / "build.ninja").exists():
        run(
            meson,
            "setup",
            str(build),
            str(source),
            "--buildtype=release",
            "-Dintrospection=false",
            "-Dvapi=false",
            "-Dexamples=false",
            "-Dtests=false",
            "-Ddocs=false",
            env=env,
        )
    run(meson, "compile", "-C", str(build), "-j", "1", env=env)
    target.mkdir(parents=True, exist_ok=True)
    shutil.copy2(
        build / "src/libgtk-layer-shell.so.0", target / "libgtk-layer-shell.so.0"
    )
    for license in (REPO / "licenses").glob("gtk-layer-shell-*.txt"):
        shutil.copy2(license, target / license.name)
    stamp.write_text(json.dumps(spec, indent=2) + "\n")
    print("Native Wayland captions ready:", target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--overlay-only", action="store_true")
    group.add_argument("--models-only", action="store_true")
    group.add_argument("--runtime-only", action="store_true")
    group.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    if args.verify:
        checks = {
            name: valid(MODELS / "zipformer" / name, info["sha256"])
            for name, info in LOCK["asr"]["files"].items()
        }
        checks[LOCK["llm"]["name"]] = valid(
            MODELS / LOCK["llm"]["name"], LOCK["llm"]["sha256"]
        )
        print(json.dumps(checks, indent=2))
        if not all(checks.values()):
            raise SystemExit(1)
        return
    DATA.mkdir(parents=True, exist_ok=True)
    DATA.chmod(0o700)
    if not args.runtime_only and not args.overlay_only:
        models()
    if not args.models_only and not args.overlay_only:
        runtime()
    if not args.models_only:
        overlay()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"OmaScribe setup: {error}")
