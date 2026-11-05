"""Inventory the staged and delivered base packages for optional GPU files."""
import hashlib
import json
import os
import pathlib
import re
import stat
import tarfile
import unicodedata

OPTIONAL_GPU = re.compile(r"(?:^|[/\\])(?:accelerators?(?:[/\\]|$)|(?:lib)?(?:ggml[-_](?:cuda|vulkan|hip|sycl)|cuda(?:rt)?|cublas(?:lt)?|cudnn|cufft|curand|cusolver|cusparse|nvrtc|nvcc|nvcuda|nvjitlink|nvfatbin|nvml|nvidia|amdhip|hipblas|rocblas|rocm|miopen|amd_comgr|sycl|vulkan|DirectML|onnxruntime_providers_(?:cuda|rocm|dml|tensorrt))[^/\\]*\.(?:dll|so(?:\.[0-9]+)*|dylib|exe))", re.I)
ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / "models.lock.json").read_text(encoding="utf-8"))
APP = pathlib.PurePosixPath("OmaScribe.app")
MAC_NATIVE = APP / "Contents/Resources/native"
STAMP = pathlib.PurePosixPath("runtime/llama/build-info.json")
MAX_STAMP = 4096
UPDATER_EXTENSIONS = (".exe", ".AppImage", ".app.tar.gz")
INSTALLER_EXTENSIONS = (*UPDATER_EXTENSIONS, ".deb", ".dmg")
RELEASE_FORMATS = {
    "aarch64-apple-darwin": ("macOS-AppleSilicon", (".dmg", ".app.tar.gz", "-runtime.app.tar.gz")),
    "x86_64-apple-darwin": ("macOS-Intel", (".dmg", ".app.tar.gz", "-runtime.app.tar.gz")),
    "x86_64-pc-windows-msvc": ("Windows-x64", (".exe", "-runtime.exe")),
    "x86_64-unknown-linux-gnu": ("Linux-x64", (".deb", ".AppImage", "-runtime.AppImage")),
}


def assert_base_names(names):
    for name in names:
        if OPTIONAL_GPU.search(str(name)):
            raise ValueError(f"Optional acceleration dependency in base payload: {name}")


def assert_base_runtime(directory):
    directory = pathlib.Path(directory)
    assert_base_names(path.relative_to(directory) for path in directory.rglob("*"))


def _package_member(name, runtime_only):
    assert_base_names([name])
    parts = pathlib.PurePosixPath(str(name).replace("\\", "/").lower()).parts
    if runtime_only and (
        "models" in parts or parts and parts[-1].endswith((".gguf", ".onnx"))
    ):
        raise AssertionError(f"Runtime payload contains models: {name}")


def _required_native_files(target, runtime_only):
    if target not in RELEASE_FORMATS:
        raise ValueError(f"Unknown package target: {target}")
    suffix = ".exe" if target == "x86_64-pc-windows-msvc" else ""
    names = {STAMP, *(pathlib.PurePosixPath(name + suffix) for name in (
        "runtime/omascribe-cli", "runtime/omascribe-asr", "runtime/llama/llama-server",
    ))}
    if not runtime_only:
        names.update(pathlib.PurePosixPath("models/zipformer") / name for name in LOCK["asr"]["files"])
        names.add(pathlib.PurePosixPath("models") / LOCK["llm"]["name"])
    return names


def _unique_object(pairs):
    values = dict(pairs)
    if len(values) != len(pairs):
        raise ValueError("Duplicate package metadata key")
    return values


def _check_stamp(data, target):
    stamp = json.loads(data, object_pairs_hook=_unique_object)
    expected = {
        "revision": LOCK["llama_cpp"]["revision"],
        "metal": target == "aarch64-apple-darwin",
        "optional_gpu_backends": [],
        "format": 2,
    }
    if (stamp != expected or type(stamp["format"]) is not int
            or type(stamp["metal"]) is not bool):
        raise ValueError("Installed llama runtime does not have the pinned base recipe")


def _read_stamp(stream, target):
    data = stream.read(MAX_STAMP + 1)
    if len(data) > MAX_STAMP:
        raise ValueError("Installed llama build stamp is too large")
    _check_stamp(data, target)


def _plain(info):
    # Windows directory junctions are reparse points, even when is_symlink is false.
    return not getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT


def assert_package_tree(root, native, target, *, runtime_only=False):
    """Read an installed or extracted package, without following directory links."""
    root, native = pathlib.Path(root), pathlib.Path(native)
    root_info = root.lstat()
    if not stat.S_ISDIR(root_info.st_mode) or not _plain(root_info):
        raise ValueError(f"Expected a real package directory: {root}")
    relative_native = pathlib.PurePosixPath(native.relative_to(root).as_posix())
    if ".." in relative_native.parts:
        raise ValueError("Native resources are outside the package")
    missing = {relative_native / name for name in _required_native_files(target, runtime_only)}
    pending = [root]
    while pending:
        with os.scandir(pending.pop()) as children:
            for entry in children:
                path = pathlib.Path(entry.path)
                relative = pathlib.PurePosixPath(path.relative_to(root).as_posix())
                _package_member(relative, runtime_only)
                info = entry.stat(follow_symlinks=False)
                if entry.is_symlink():
                    _package_member(os.readlink(path), runtime_only)
                elif not _plain(info):
                    raise ValueError(f"Unexpected package reparse point: {relative}")
                elif stat.S_ISDIR(info.st_mode):
                    pending.append(path)
                elif stat.S_ISREG(info.st_mode):
                    if info.st_size:
                        missing.discard(relative)
                else:
                    raise ValueError(f"Unexpected package member: {relative}")
    if missing:
        raise ValueError(f"Missing regular package files: {sorted(map(str, missing))}")
    with (native / STAMP).open("rb") as stream:
        _read_stamp(stream, target)


def assert_nsis_members(listing, *, runtime_only=False):
    """Check 7-Zip's member section, including files an installer later removes."""
    header, divider, members = listing.partition("\n----------\n")
    paths = [line.removeprefix("Path = ").strip() for line in members.splitlines()
             if line.startswith("Path = ")]
    if not divider or "type = nsis" not in header.lower() or not paths or not all(paths):
        raise AssertionError("Unable to inspect NSIS archive members")
    for name in paths:
        _package_member(name, runtime_only)
    names = {"/" + name.replace("\\", "/").lower().strip("/") for name in paths}
    for path in _required_native_files("x86_64-pc-windows-msvc", runtime_only):
        if not any(name.endswith(f"/native/{path.as_posix().lower()}") for name in names):
            raise ValueError(f"NSIS archive is missing native/{path}")


def _mac_path_key(path):
    # The default macOS volume folds case and Unicode-normalized names.
    return tuple(unicodedata.normalize("NFD", part).casefold() for part in path.parts)


def assert_mac_archive(artifact, target, *, runtime_only=False, extract_to=None):
    """Read the signed updater tar. Full updates need no second model extraction."""
    missing = {MAC_NATIVE / name for name in _required_native_files(target, runtime_only)}
    missing.add(APP / "Contents/MacOS/omascribe")
    seen = {}
    with tarfile.open(artifact, "r|gz") as archive:
        def checked_members():
            for member in archive:
                name = pathlib.PurePosixPath(member.name)
                key = _mac_path_key(name)
                if (not name.parts or name.parts[0] != APP.name or ".." in name.parts
                        or "\\" in member.name or key in seen):
                    raise ValueError(f"Unexpected or duplicate macOS archive member: {member.name}")
                seen[key] = member.isdir()
                _package_member(name, runtime_only)
                if member.issym() or member.islnk():
                    _package_member(member.linkname, runtime_only)
                elif not (member.isdir() or member.isfile()):
                    raise ValueError(f"Unexpected tar member type: {member.name}")
                if member.isfile() and member.size:
                    missing.discard(name)
                if extract_to is None and name == MAC_NATIVE / STAMP and member.isfile():
                    with archive.extractfile(member) as stream:
                        _read_stamp(stream, target)
                yield member

        if extract_to is None:
            for _ in checked_members():
                pass
        else:
            extract_to = pathlib.Path(extract_to)
            extract_to.mkdir()
            archive.extractall(extract_to, members=checked_members(), filter="data")
    if missing:
        raise ValueError(f"Missing regular macOS archive files: {sorted(map(str, missing))}")
    for key in seen:
        if any(parent in seen and not seen[parent] for parent in (key[:i] for i in range(1, len(key)))):
            raise ValueError(f"Non-directory macOS archive ancestor: {'/'.join(key)}")
    if extract_to is not None:
        return extract_to / APP


def release_payloads(version):
    """The distinct assets used by the stable release and both update manifests."""
    return {
        f"OmaScribe_{version}_{label}{suffix}": target
        for target, (label, suffixes) in RELEASE_FORMATS.items()
        for suffix in suffixes
    }


def _regular_size(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not _plain(info) or not info.st_size:
        raise ValueError(f"Expected a nonempty regular file: {path.name}")
    return info.st_size


def _receipt(artifact, target, commit, digest):
    if (target not in RELEASE_FORMATS or not re.fullmatch(r"[a-f0-9]{64}", digest)
            or not re.fullmatch(r"[a-f0-9]{40}", commit)):
        raise ValueError("Invalid package target, commit or checksum")
    receipt = {
        "schema": 1, "commit": commit, "target": target, "file": artifact.name,
        "bytes": _regular_size(artifact), "sha256": digest,
    }
    if artifact.name.endswith(UPDATER_EXTENSIONS):
        signature = artifact.with_name(artifact.name + ".sig")
        if _regular_size(signature) > 8192:
            raise ValueError(f"Invalid updater signature size: {signature.name}")
        receipt["signature_sha256"] = hashlib.sha256(signature.read_bytes()).hexdigest()
    return receipt


def write_package_receipt(artifact, *, target, commit, digest):
    """Called only after native inventory and applicable signature checks."""
    receipt = _receipt(artifact, target, commit, digest)
    artifact.with_name(artifact.name + ".inventory.json").write_text(
        json.dumps(receipt, indent=2) + "\n", encoding="utf-8",
    )


def collect_release_artifacts(downloads, assets, version):
    """Reject duplicate basenames before gathering the four producer directories."""
    expected = {"package-" + target for target in RELEASE_FORMATS}
    if {path.name for path in downloads.iterdir()} != expected:
        raise ValueError("Missing or unexpected native package artifacts")
    payloads = release_payloads(version)
    owners = {
        name + suffix: target for name, target in payloads.items()
        for suffix in ("", ".sha256", ".inventory.json", ".sig", ".verified.json")
    }
    files = {}
    for folder in downloads.iterdir():
        info = folder.lstat()
        if not stat.S_ISDIR(info.st_mode) or not _plain(info):
            raise ValueError(f"Expected a producer directory: {folder.name}")
        for path in folder.iterdir():
            if path.name in files:
                raise ValueError(f"Duplicate delivered asset: {path.name}")
            if path.name in owners and folder.name != "package-" + owners[path.name]:
                raise ValueError(f"Delivered asset came from another target: {path.name}")
            _regular_size(path)
            files[path.name] = path
    assets.mkdir()
    for name, path in files.items():
        path.rename(assets / name)


def verify_release_inventory(assets, version, commit):
    """Check the exact artifact set and stream each payload once in the publisher."""
    expected = release_payloads(version)
    names = {path.name for path in assets.iterdir()}
    if ({name for name in names if name.endswith(INSTALLER_EXTENSIONS)} != expected.keys()
            or {name for name in names if name.endswith(".inventory.json")}
            != {name + ".inventory.json" for name in expected}
            or {name for name in names if name.endswith(".sha256")}
            != {name + ".sha256" for name in expected}):
        raise ValueError("Release payload or inventory set is incomplete or unexpected")
    checksums = []
    for name, target in sorted(expected.items()):
        artifact = assets / name
        _regular_size(artifact)
        with artifact.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        receipt_path = assets / (name + ".inventory.json")
        if _regular_size(receipt_path) > 8192:
            raise ValueError(f"Invalid package inventory size: {name}")
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
        if (receipt != _receipt(artifact, target, commit, digest)
                or type(receipt["schema"]) is not int or type(receipt["bytes"]) is not int):
            raise ValueError(f"Package inventory does not match the delivered file: {name}")
        line = f"{digest}  {name}\n"
        sidecar = assets / (name + ".sha256")
        if _regular_size(sidecar) > 512 or sidecar.read_text(encoding="utf-8") != line:
            raise ValueError(f"Package checksum does not match the delivered file: {name}")
        checksums.append(line)
    return "".join(checksums)
