"""Packaging contracts, not GPU performance claims. All payloads are fictional."""
import importlib.util
import json
import pathlib
import tarfile
import tempfile
import unittest
import sys
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from runtime_inventory import assert_base_names
spec = importlib.util.spec_from_file_location("components", ROOT / "scripts/acceleration-components.py")
components = importlib.util.module_from_spec(spec)
spec.loader.exec_module(components)


class AccelerationPackagingTests(unittest.TestCase):
    def runtime(self, folder, backend="vulkan", target="linux-x86_64"):
        folder.mkdir()
        (folder / ("llama-server.exe" if target.startswith("windows") else "llama-server")).write_bytes(b"fictional fixture, never execute")
        (folder / "llama.cpp-LICENSE.txt").write_text("fixture")
        components.write_json(folder / "build-info.json", dict(revision=components.LOCK["revision"], backend=backend, target=target, format=1))
        if backend == "cuda":
            (folder / "CUDA-EULA.html").write_text("fixture license")
            (folder / ("cudart64_12.dll" if target.startswith("windows") else "libcudart.so.12")).write_bytes(b"fictional dependency")
        return folder

    def test_deterministic_hash_checked_matrix_and_partial_build_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            root = pathlib.Path(root)
            output = root / "packages"
            for target in sorted(components.TARGETS):
                for backend in sorted(components.BACKENDS):
                    runtime = self.runtime(root / (backend + target), backend, target)
                    with self.assertRaises(FileNotFoundError):
                        components.index(output, "acceleration-test-1")
                    first = components.package(runtime, backend, target, "acceleration-test-1", output)
                    second = components.package(runtime, backend, target, "acceleration-test-1", output)
                    self.assertEqual(first, second)
                    with tarfile.open(output / first["archive"]["name"]) as tar:
                        self.assertTrue(all(m.isfile() and m.mtime == 0 for m in tar))
            self.assertEqual(len(components.index(output, "acceleration-test-1")["components"]), 4)
            (output / first["archive"]["name"]).write_bytes(b"changed")
            with self.assertRaises(ValueError):
                components.index(output, "acceleration-test-1")

    def test_source_revision_driver_sdk_links_and_reserved_metadata_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            root = pathlib.Path(root)
            runtime = self.runtime(root / "runtime")
            for name in ["nvcuda.dll", "libcuda.so.1", "libvulkan.so.1", "installed.json", "nvcc.exe"]:
                (runtime / name).write_bytes(b"unexpected")
                with self.assertRaises(ValueError):
                    components.package(runtime, "vulkan", "linux-x86_64", "acceleration-test-1", root / "out")
                (runtime / name).unlink()
            (runtime / "SDK").mkdir()
            with self.assertRaises(ValueError):
                components.package(runtime, "vulkan", "linux-x86_64", "acceleration-test-1", root / "out")
            (runtime / "SDK").rmdir()
            (runtime / "build-info.json").write_text('{}')
            with self.assertRaises(ValueError):
                components.package(runtime, "vulkan", "linux-x86_64", "acceleration-test-1", root / "out")

    def test_cuda_dependency_closure_uses_soname_only(self):
        with tempfile.TemporaryDirectory() as root:
            root = pathlib.Path(root)
            output, cuda = root / "runtime", root / "cuda"
            output.mkdir()
            (cuda / "lib64").mkdir(parents=True)
            (output / "libggml-cuda.so").write_bytes(b"fixture")
            for name in ["libcudart.so", "libcudart.so.12", "libcudart.so.12.8.1", "libcublas.so.12", "libcublasLt.so.12", "libcuda.so.1"]:
                (cuda / "lib64" / name).write_bytes(name.encode())
            def deps(path, windows):
                return {"libggml-cuda.so": ["libcudart.so.12", "libcublas.so.12", "libcuda.so.1"], "libcublas.so.12": ["libcublasLt.so.12"]}.get(path.name, [])
            with patch.object(components, "dependencies", deps):
                components.collect_cuda(output, cuda, False)
            self.assertEqual({p.name for p in output.iterdir()}, {"libggml-cuda.so", "libcudart.so.12", "libcublas.so.12", "libcublasLt.so.12"})

    def test_bad_release_target_and_backend_rejected(self):
        for version, target, backend in [("../escape", "linux-x86_64", "vulkan"), ("v0.3.4", "linux-x86_64", "cuda"), ("acceleration-test-1", "macos-arm64", "cuda"), ("acceleration-test-1", "linux-x86_64", "unverified")]:
            with self.assertRaises(ValueError):
                components.package(pathlib.Path("missing"), backend, target, version, pathlib.Path("missing"))

    def test_base_installer_rejects_optional_gpu_dependencies(self):
        assert_base_names(["runtime/llama/llama-server", "runtime/onnxruntime.dll", "runtime/libggml-metal.dylib", "models/tokens.txt"])
        for name in ["native/runtime/libggml-cuda.so", "runtime/cublas64_12.dll", "libcudart.so.12", "runtime/ggml-vulkan.dll", "runtime/librocblas.so", "native/accelerators/index.json", "runtime/libsycl.so.8"]:
            with self.assertRaises(ValueError):
                assert_base_names([name])
