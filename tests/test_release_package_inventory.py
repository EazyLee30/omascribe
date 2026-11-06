"""Model-free controls for the exact delivered package inventory gate."""

import hashlib
import importlib.util
import json
import pathlib
import sys
import tarfile
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import runtime_inventory as inventory

SPEC = importlib.util.spec_from_file_location("update_manifest", ROOT / "scripts/update-manifest.py")
PUBLISHER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISHER)
VERSION = "0.3.5"
COMMIT = "a" * 40
MAC = "aarch64-apple-darwin"


def fake_native(native, target, runtime_only=False):
    for name in inventory._required_native_files(target, runtime_only):
        file = native / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(b"fixture")
    (native / inventory.STAMP).write_text(json.dumps({
        "format": 2, "revision": inventory.LOCK["llama_cpp"]["revision"],
        "metal": target == MAC, "optional_gpu_backends": [],
    }))


def make_artifacts(root):
    (root / "package.json").write_text(json.dumps({"version": VERSION}))
    notes = root / f"docs/releases/v{VERSION}.md"
    notes.parent.mkdir(parents=True)
    notes.write_text("Fictional release.")
    for name, target in inventory.release_payloads(VERSION).items():
        folder = root / "release-downloads" / ("package-" + target)
        folder.mkdir(parents=True, exist_ok=True)
        artifact = folder / name
        artifact.write_bytes(b"payload")
        digest = hashlib.sha256(b"payload").hexdigest()
        if name.endswith(inventory.UPDATER_EXTENSIONS):
            (folder / (name + ".sig")).write_text("fixture signature")
            (folder / (name + ".verified.json")).write_text(json.dumps({
                "verified": True, "version": VERSION, "file": name,
            }))
        (folder / (name + ".sha256")).write_text(f"{digest}  {name}\n")
        inventory.write_package_receipt(artifact, target=target, commit=COMMIT, digest=digest)


class ReleasePackageInventoryTests(unittest.TestCase):
    def test_extracted_tree_rejects_optional_files_outside_native(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            native = root / "usr/lib/OmaScribe/native"
            target = "x86_64-unknown-linux-gnu"
            fake_native(native, target)
            inventory.assert_package_tree(root, native, target)
            outside = root / "usr/lib/libcudart.so.12"
            outside.write_bytes(b"fixture")
            with self.assertRaisesRegex(ValueError, "Optional acceleration dependency"):
                inventory.assert_package_tree(root, native, target)
            outside.unlink()
            (native / "runtime/omascribe-asr").unlink()
            with self.assertRaisesRegex(ValueError, "Missing regular package files"):
                inventory.assert_package_tree(root, native, target)

    def test_runtime_tree_rejects_legacy_models_and_mismatched_recipe(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            native = root / inventory.MAC_NATIVE
            fake_native(native, MAC, runtime_only=True)
            inventory.assert_package_tree(root, native, MAC, runtime_only=True)
            models = native / "models"
            models.mkdir()
            with self.assertRaisesRegex(AssertionError, "contains models"):
                inventory.assert_package_tree(root, native, MAC, runtime_only=True)
            models.rmdir()
            with self.assertRaisesRegex(ValueError, "pinned base recipe"):
                inventory.assert_package_tree(root, native, "x86_64-apple-darwin", runtime_only=True)

    def test_nsis_member_scan_checks_install_payload_and_transient_members(self):
        target = "x86_64-pc-windows-msvc"
        header = "Path = setup.exe\nType = Nsis\n----------\n"
        paths = [f"Path = $INSTDIR/native/{name}\n" for name in sorted(
            inventory._required_native_files(target, runtime_only=True))]
        inventory.assert_nsis_members(header + "".join(paths), runtime_only=True)
        with self.assertRaisesRegex(ValueError, "missing native"):
            inventory.assert_nsis_members(header + "".join(paths[:-1]), runtime_only=True)
        with self.assertRaisesRegex(ValueError, "Optional acceleration dependency"):
            inventory.assert_nsis_members(header + "".join(paths) + "Path = $PLUGINSDIR\\cublas64_12.dll\n",
                                          runtime_only=True)
        with self.assertRaisesRegex(AssertionError, "Unable to inspect"):
            inventory.assert_nsis_members("Path = setup.exe\nPath = incomplete")

    def test_mac_tar_readback_and_safe_runtime_extract(self):
        for runtime in (False, True):
            with self.subTest(runtime=runtime), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                app = root / inventory.APP
                fake_native(root / inventory.MAC_NATIVE, MAC, runtime)
                executable = app / "Contents/MacOS/omascribe"
                executable.parent.mkdir()
                executable.write_bytes(b"app fixture")
                executable.chmod(0o755)
                artifact = root / "update.app.tar.gz"
                with tarfile.open(artifact, "w:gz") as archive:
                    archive.add(app, arcname=inventory.APP.name)
                extract = root / "extracted" if runtime else None
                unpacked = inventory.assert_mac_archive(artifact, MAC, runtime_only=runtime, extract_to=extract)
                if runtime:
                    self.assertEqual(unpacked, extract / inventory.APP)
                    self.assertEqual((unpacked / "Contents/MacOS/omascribe").read_bytes(), b"app fixture")
                    inventory.assert_package_tree(unpacked, unpacked / "Contents/Resources/native",
                                                  MAC, runtime_only=True)
                with tarfile.open(artifact, "w:gz") as archive:
                    archive.add(app, arcname=inventory.APP.name)
                    member = tarfile.TarInfo(f"{inventory.APP}/Contents/libggml-vulkan.dylib")
                    archive.addfile(member)
                with self.assertRaisesRegex(ValueError, "Optional acceleration dependency"):
                    inventory.assert_mac_archive(artifact, MAC, runtime_only=runtime)
                with tarfile.open(artifact, "w:gz") as archive:
                    archive.add(app, arcname=inventory.APP.name)
                    alias = inventory.MAC_NATIVE / "runtime/LLAMA/BUILD-INFO.JSON"
                    archive.addfile(tarfile.TarInfo(str(alias)))
                with self.assertRaisesRegex(ValueError, "duplicate macOS archive member"):
                    inventory.assert_mac_archive(artifact, MAC, runtime_only=runtime)

    def test_publisher_writes_two_manifests_and_eleven_checksums(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            make_artifacts(root)
            PUBLISHER.prepare_release(root, COMMIT)
            assets = root / "release-assets"
            expected = inventory.release_payloads(VERSION)
            self.assertEqual(len(expected), 11)
            self.assertEqual(set(line.split("  ", 1)[1] for line in
                                 (assets / "SHA256SUMS.txt").read_text().splitlines()), set(expected))
            for name in ("latest.json", "latest-runtime.json"):
                self.assertEqual(len(json.loads((assets / name).read_text())["platforms"]), 4)

    def test_publisher_rejects_stale_or_moved_receipts_and_tampered_payloads(self):
        for mutation, message in {
            "signature": "does not match",
            "same-size payload": "does not match",
            "missing receipt": "incomplete",
            "old commit": "does not match",
            "duplicate target filename": "Duplicate|another target",
            "moved receipt": "another target",
        }.items():
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                make_artifacts(root)
                artifact = next((root / "release-downloads").glob("*/OmaScribe*Windows-x64.exe"))
                receipt = artifact.with_name(artifact.name + ".inventory.json")
                other = next(folder for folder in artifact.parent.parent.iterdir() if folder != artifact.parent)
                if mutation == "signature":
                    artifact.with_name(artifact.name + ".sig").write_text("different signature")
                elif mutation == "same-size payload":
                    artifact.write_bytes(b"corrupt")
                    sidecar = artifact.with_name(artifact.name + ".sha256")
                    sidecar.write_text(f"{hashlib.sha256(b'corrupt').hexdigest()}  {artifact.name}\n")
                elif mutation == "missing receipt":
                    receipt.unlink()
                elif mutation == "old commit":
                    data = json.loads(receipt.read_text())
                    data["commit"] = "b" * 40
                    receipt.write_text(json.dumps(data))
                elif mutation == "duplicate target filename":
                    (other / artifact.name).write_bytes(artifact.read_bytes())
                else:
                    receipt.rename(other / receipt.name)
                with self.assertRaisesRegex(ValueError, message):
                    PUBLISHER.prepare_release(root, COMMIT)
                self.assertFalse((root / "release-assets/latest.json").exists())
                self.assertFalse((root / "release-assets/latest-runtime.json").exists())
