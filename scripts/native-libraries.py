#!/usr/bin/env python3
"""Keep the native ASR SDK outside Cargo's pruned target cache, with verified inputs."""

import argparse
import platform
import hashlib
import json
import os
import pathlib
import shutil
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--target", required=True)
args = parser.parse_args()
if os.environ.get("GITHUB_ACTIONS") != "true" or platform.system() not in [
    "Darwin",
    "Windows",
]:
    raise SystemExit("Native GitHub runner required")
spec = json.loads((ROOT / "models.lock.json").read_text(encoding="utf-8"))[
    "native_asr"
]["targets"][args.target]
folder = ROOT / "build-assets/native-asr" / args.target
folder.mkdir(parents=True, exist_ok=True)
archive = folder / "sdk.tar.bz2"


def valid(path):
    if not path.is_file():
        return False
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest() == spec["sha256"]


if not valid(archive):
    request = urllib.request.Request(
        spec["url"], headers={"User-Agent": "OmaScribe-release"}
    )
    part = archive.with_suffix(".part")
    with (
        urllib.request.urlopen(request, timeout=120) as response,
        part.open("wb") as output,
    ):
        shutil.copyfileobj(response, output)
    assert valid(part), "ASR SDK checksum mismatch"
    part.replace(archive)
libraries = folder / "lib"
libraries.mkdir(exist_ok=True)
with tarfile.open(archive) as tar:
    prefix = spec["archive_root"] + "/lib/"
    for member in tar:
        name = member.name.removeprefix(prefix)
        if (
            member.isfile()
            and member.name.startswith(prefix)
            and "/" not in name
            and name.endswith((".lib", ".a"))
        ):
            destination = libraries / name
            if not destination.exists() or destination.stat().st_size != member.size:
                with (
                    tar.extractfile(member) as source,
                    destination.open("wb") as output,
                ):
                    shutil.copyfileobj(source, output)
suffix = ".lib" if os.name == "nt" else ".a"
name = "sherpa-onnx-c-api.lib" if os.name == "nt" else "libsherpa-onnx-c-api.a"
assert (libraries / name).is_file()
assert len(list(libraries.glob("*" + suffix))) >= 13
with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as environment:
    environment.write("SHERPA_ONNX_LIB_DIR=" + str(libraries) + "\n")
print(
    "Verified ASR SDK:",
    libraries,
    "libraries:",
    len(list(libraries.glob("*" + suffix))),
)
