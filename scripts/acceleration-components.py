#!/usr/bin/env python3
"""Build/package optional GPU runtimes, independently of the base installer.

No signing secrets are needed here. The separate signing job signs the exact
index.payload bytes with the existing application release key and version.
"""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / "models.lock.json").read_text(encoding="utf-8"))["llama_cpp"]
TARGETS = {"windows-x86_64", "linux-x86_64"}
BACKENDS = {"cuda", "vulkan"}
LIMIT = 4 * 1024**3
RESERVED = {"installed.json", "verified.json", "active.json", "component-index.json", "payload.tar.bz2"}
CUDA_RUNTIME = re.compile(r"^(?:lib(?:cublas(?:Lt)?|cudart|nvrtc(?:-builtins)?)\.so(?:\.[0-9]+)*|(?:cublas(?:Lt)?|cudart|nvrtc(?:-builtins)?)64_[0-9_]+\.dll)$", re.I)
DRIVER = re.compile(r"^(?:libcuda\.so|nvcuda\.dll|libnvidia|nvml|libamdhip|amdhip|libvulkan|vulkan-1)", re.I)


def run(*argv, **kwargs):
    return subprocess.run([str(a) for a in argv], check=True, **kwargs)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def name_ok(name):
    return len(name) <= 100 and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name) is not None


def version_ok(version):
    return name_ok(version) and version.startswith("acceleration-")


def runtime_member(name):
    return name in {"llama-server", "llama-server.exe", "build-info.json", "llama.cpp-LICENSE.txt", "CUDA-EULA.html"} or CUDA_RUNTIME.fullmatch(name) or re.fullmatch(r"(?:lib)?(?:llama|ggml)[A-Za-z0-9_.-]*\.(?:dll|so(?:\.[0-9]+)*)", name)


def asset(path, url=""):
    return dict(name=path.name, url=url, sha256=digest(path), bytes=path.stat().st_size)


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def package(runtime, backend, target, version, output):
    """Strict flat archive contract shared with core/components.rs."""
    if backend not in BACKENDS or target not in TARGETS or not version_ok(version):
        raise ValueError("Unsupported component backend, target or release version")
    runtime, output = pathlib.Path(runtime), pathlib.Path(output)
    files = sorted(runtime.iterdir())
    if not files or len(files) > 512:
        raise ValueError("Invalid component file count")
    for path in files:
        if path.is_symlink() or not path.is_file() or not name_ok(path.name) or path.name in RESERVED or DRIVER.match(path.name) or not runtime_member(path.name):
            raise ValueError(f"Unexpected component member: {path.name}")
    entrypoint = "llama-server.exe" if target.startswith("windows") else "llama-server"
    required = {entrypoint, "build-info.json", "llama.cpp-LICENSE.txt"}
    if backend == "cuda":
        required.add("CUDA-EULA.html")
        if not any(CUDA_RUNTIME.fullmatch(path.name) for path in files):
            raise ValueError("CUDA runtime dependencies are missing")
    if not required.issubset({path.name for path in files}):
        raise ValueError("Missing runtime or license")
    expected = dict(revision=LOCK["revision"], backend=backend, target=target, format=1)
    if json.loads((runtime / "build-info.json").read_text(encoding="utf-8")) != expected:
        raise ValueError("Build provenance does not match the pinned runtime")
    if sum(path.stat().st_size for path in files) > LIMIT:
        raise ValueError("Component exceeds the unpacked limit")
    output.mkdir(parents=True, exist_ok=True)
    component_id = f"{backend}-{target}"
    archive = output / f"OmaScribe-{component_id}-{version}.tar.bz2"
    part = archive.with_suffix(".part")
    try:
        with tarfile.open(part, "w:bz2", format=tarfile.USTAR_FORMAT) as tar:
            for path in files:
                info = tarfile.TarInfo(path.name)
                info.size = path.stat().st_size
                info.mode = 0o755 if path.name == entrypoint or ".so" in path.name else 0o644
                info.mtime = info.uid = info.gid = 0
                with path.open("rb") as stream:
                    tar.addfile(info, stream)
        if part.stat().st_size > LIMIT:
            raise ValueError("Component exceeds the download limit")
        part.replace(archive)
    finally:
        part.unlink(missing_ok=True)
    manifest = dict(id=component_id, name=f"{backend.upper()} · {target}", backend=backend, target=target,
                    entrypoint=entrypoint, license="MIT + NVIDIA CUDA EULA" if backend == "cuda" else "MIT",
                    archive=asset(archive, f"https://github.com/EazyLee30/omascribe/releases/download/{version}/{archive.name}"),
                    files=[asset(path) for path in files])
    write_json(output / (component_id + ".component.json"), manifest)
    return manifest


def index(directory, version):
    """Reject partial, stale or modified build artifacts before asking for signing."""
    if not version_ok(version):
        raise ValueError("Invalid component release version")
    directory = pathlib.Path(directory)
    components = []
    for target in sorted(TARGETS):
        for backend in sorted(BACKENDS):
            path = directory / f"{backend}-{target}.component.json"
            component = json.loads(path.read_text(encoding="utf-8"))
            if component["id"] != f"{backend}-{target}" or component["target"] != target or component["backend"] != backend:
                raise ValueError("Mismatched component matrix")
            archive = component["archive"]
            if not name_ok(archive["name"]) or archive["url"] != f"https://github.com/EazyLee30/omascribe/releases/download/{version}/{archive['name']}":
                raise ValueError("Mismatched component release")
            payload = directory / archive["name"]
            if payload.stat().st_size != archive["bytes"] or digest(payload) != archive["sha256"]:
                raise ValueError("Component archive changed after packaging")
            # Verify every unpacked member against the signed file inventory as well.
            expected = {f["name"]: f for f in component["files"]}
            if len(expected) != len(component["files"]) or not expected or len(expected) > 512:
                raise ValueError("Invalid component inventory")
            seen = set()
            with tarfile.open(payload, "r:bz2") as tar:
                for member in tar:
                    spec = expected.get(member.name)
                    if (not spec or not name_ok(member.name) or member.name in seen or not member.isfile()
                            or member.size != spec["bytes"] or spec["url"] or member.name in RESERVED or DRIVER.match(member.name) or not runtime_member(member.name)):
                        raise ValueError("Invalid component archive member")
                    with tar.extractfile(member) as stream:
                        if hashlib.file_digest(stream, "sha256").hexdigest() != spec["sha256"]:
                            raise ValueError("Unpacked component checksum mismatch")
                    seen.add(member.name)
            if seen != set(expected):
                raise ValueError("Incomplete component archive")
            components.append(component)
    result = dict(schema=1, version=version, runtime_revision=LOCK["revision"], components=components)
    write_json(directory / "index.payload", result)
    return result


def copy_flat(source, destination):
    if destination.exists() and digest(source) != digest(destination):
        raise ValueError(f"Conflicting dependency: {source.name}")
    shutil.copy2(source, destination, follow_symlinks=True)


def dependencies(path, windows):
    command = ["dumpbin", "/dependents", str(path)] if windows else ["readelf", "-d", str(path)]
    text = subprocess.check_output(command, text=True, errors="replace", timeout=30)
    return re.findall(r"^\s+([A-Za-z0-9_.-]+\.dll)\s*$", text, re.M | re.I) if windows else re.findall(r"\(NEEDED\).*?\[([^]]+)\]", text)


def collect_cuda(output, cuda, windows):
    # Copy only names actually requested by the loader, avoiding three copies of
    # large Linux libraries under their development symlink/version/SONAME names.
    folders = [cuda / "bin", cuda / "lib64", cuda / "targets/x86_64-linux/lib"]
    candidates = {p.name.lower(): p for folder in folders if folder.is_dir() for p in folder.iterdir() if p.is_file() and CUDA_RUNTIME.fullmatch(p.name)}
    queue = [p for p in output.iterdir() if p.suffix.lower() == ".dll" or ".so" in p.name]
    visited, count = set(), 0
    while queue:
        path = queue.pop()
        if path.name.lower() in visited:
            continue
        visited.add(path.name.lower())
        for name in dependencies(path, windows):
            if not CUDA_RUNTIME.fullmatch(name):
                continue
            source = candidates.get(name.lower())
            if source is None:
                raise ValueError(f"Missing CUDA runtime dependency: {name}")
            destination = output / name
            if not destination.exists():
                copy_flat(source, destination)
                count += 1
            queue.append(destination)
    if not count:
        raise ValueError("CUDA backend has no detected runtime dependencies")


def vulkan_flags(sdk, windows):
    """Recent SDKs put loader libraries below a component-specific directory."""
    headers = [sdk / "Include", sdk / "include"]
    include = next((p for p in headers if (p / "vulkan/vulkan.h").is_file()), None)
    libraries = [sdk / "Lib/vulkan-1.lib", sdk / "Lib/VulkanLoader/lib/vulkan-1.lib"] if windows else [sdk / "lib/libvulkan.so", sdk / "lib/VulkanLoader/lib/libvulkan.so", sdk / "lib/VulkanLoader/lib/libvulkan.so.1"]
    library = next((p for p in libraries if p.is_file()), None)
    if include is None or library is None:
        raise ValueError(f"Incomplete Vulkan SDK: headers={include is not None}, loader={library is not None}")
    flags = [f"-DVulkan_INCLUDE_DIR={include}", f"-DVulkan_LIBRARY={library}", f"-DCMAKE_PREFIX_PATH={sdk}"]
    configs = list(sdk.rglob("SPIRV-HeadersConfig.cmake"))
    if configs:
        flags.append(f"-DSPIRV-Headers_DIR={configs[0].parent}")
    return flags


def build(backend, output):
    """Native CI builds only. SDK/compiler/driver files never enter the output."""
    if platform.machine().lower() not in {"x86_64", "amd64"} or platform.system() not in {"Windows", "Linux"} or backend not in BACKENDS:
        raise ValueError("GPU components require a native Windows/Linux x86_64 builder")
    target = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
    work = ROOT / "build-assets" / f"accelerator-{backend}-{target}"
    source, cmake = work / "llama.cpp", work / "build"
    source.mkdir(parents=True, exist_ok=True)
    if not (source / ".git").exists():
        run("git", "init", source)
    run("git", "-C", source, "fetch", "--depth", "1", LOCK["url"], LOCK["revision"])
    run("git", "-C", source, "checkout", "--detach", "FETCH_HEAD")
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != LOCK["revision"]:
        raise ValueError("Unexpected runtime source revision")
    flags = ["-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=ON", "-DGGML_BACKEND_DL=ON", "-DGGML_NATIVE=OFF",
             "-DGGML_CPU_ALL_VARIANTS=OFF", "-DGGML_AVX=ON", "-DGGML_AVX2=ON", "-DGGML_FMA=ON", "-DGGML_F16C=ON", "-DGGML_AVX512=OFF",
             "-DGGML_METAL=OFF", "-DGGML_ACCELERATE=OFF", "-DGGML_HIP=OFF", "-DGGML_SYCL=OFF", "-DGGML_BLAS=OFF", "-DGGML_OPENMP=OFF",
             "-DLLAMA_OPENSSL=OFF", "-DLLAMA_CURL=OFF", "-DLLAMA_BUILD_TESTS=OFF", "-DLLAMA_BUILD_EXAMPLES=OFF",
             "-DLLAMA_BUILD_UI=OFF", "-DLLAMA_USE_PREBUILT_UI=OFF", "-DLLAMA_BUILD_SERVER=ON",
             "-DGGML_CUDA=" + ("ON" if backend == "cuda" else "OFF"), "-DGGML_VULKAN=" + ("ON" if backend == "vulkan" else "OFF")]
    if backend == "cuda":
        flags += ["-DCMAKE_CUDA_ARCHITECTURES=75-virtual;80-virtual;86-real;89-real;90-virtual;120a-real", "-DGGML_CUDA_NCCL=OFF"]
    else:
        flags += vulkan_flags(pathlib.Path(os.environ["VULKAN_SDK"]), os.name == "nt")
    if os.name == "nt":
        flags += ["-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DLLAMA_STATIC=ON"]
    run("cmake", "-S", source, "-B", cmake, *flags)
    run("cmake", "--build", cmake, "--config", "Release", "--target", "llama-server", "--parallel", "2" if backend == "cuda" else "3")
    output = pathlib.Path(output)
    if output.exists():
        raise ValueError("Choose a new empty component runtime directory")
    output.mkdir(parents=True)
    entrypoint = "llama-server.exe" if os.name == "nt" else "llama-server"
    binary = next((cmake / "bin").rglob(entrypoint))
    copy_flat(binary, output / entrypoint)
    for path in sorted((cmake / "bin").rglob("*")):
        if path.is_file() and (path.suffix.lower() == ".dll" or ".so" in path.name):
            copy_flat(path, output / path.name)
    if backend == "cuda":
        cuda = pathlib.Path(os.environ["CUDA_PATH"])
        # Narrow, redistributable runtime allowlist. Never copy the CUDA SDK wholesale.
        collect_cuda(output, cuda, os.name == "nt")
        request = urllib.request.Request("https://docs.nvidia.com/cuda/archive/12.8.1/eula/index.html", headers={"User-Agent": "OmaScribe-component-build"})
        with urllib.request.urlopen(request, timeout=60) as response:
            (output / "CUDA-EULA.html").write_bytes(response.read(2 * 1024**2))
    shutil.copy2(source / "LICENSE", output / "llama.cpp-LICENSE.txt")
    write_json(output / "build-info.json", dict(revision=actual, backend=backend, target=target, format=1))
    if os.name != "nt":
        for path in output.iterdir():
            if path.name == entrypoint or ".so" in path.name:
                run("patchelf", "--set-rpath", "$ORIGIN", path)
    env = dict(os.environ, LD_LIBRARY_PATH=str(output.resolve()), PATH=str(output.resolve()) + os.pathsep + os.environ["PATH"])
    version = subprocess.check_output([str((output / entrypoint).resolve()), "--version"], stderr=subprocess.STDOUT, env=env, timeout=30, text=True)
    if actual[:7] not in version:
        raise ValueError("Built executable revision mismatch")
    # Loading the GPU/model is deliberately a separate local validation, not a CI claim.
    print(json.dumps(dict(runtime=str(output), backend=backend, target=target, revision=actual)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    b = commands.add_parser("build")
    b.add_argument("--backend", choices=sorted(BACKENDS), required=True)
    b.add_argument("--output", type=pathlib.Path, required=True)
    p = commands.add_parser("package")
    p.add_argument("--runtime", type=pathlib.Path, required=True)
    p.add_argument("--backend", choices=sorted(BACKENDS), required=True)
    p.add_argument("--target", choices=sorted(TARGETS), required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--output", type=pathlib.Path, required=True)
    i = commands.add_parser("index")
    i.add_argument("--directory", type=pathlib.Path, required=True)
    i.add_argument("--version", required=True)
    args = parser.parse_args()
    if args.command == "build":
        build(args.backend, args.output)
    elif args.command == "package":
        package(args.runtime, args.backend, args.target, args.version, args.output)
    else:
        index(args.directory, args.version)


if __name__ == "__main__":
    main()
