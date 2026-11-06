import os
from pathlib import Path
import hashlib
import json
import tempfile
import unittest
from unittest.mock import patch

from windows_runtime_support import acceptance_environment, audit_runtime_modules, loaded_modules, component_identity


class WindowsRuntimeAcceptanceTests(unittest.TestCase):
    def test_component_identity_binds_payload_to_archive_and_pinned_revision(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);runtime=root/'runtime';runtime.mkdir()
            server=runtime/'llama-server.exe';server.write_bytes(b'fictional executable, never run')
            lock=json.loads((Path(__file__).resolve().parents[1]/'models.lock.json').read_text(encoding='utf-8'))
            (runtime/'build-info.json').write_text(json.dumps(dict(revision=lock['llama_cpp']['revision'],backend='vulkan',target='windows-x86_64',format=1)))
            archive=root/'component.tar.bz2';archive.write_bytes(b'fictional archive')
            def asset(path):return dict(name=path.name,bytes=path.stat().st_size,sha256=hashlib.sha256(path.read_bytes()).hexdigest())
            manifest=root/'component.json';manifest.write_text(json.dumps(dict(target='windows-x86_64',backend='vulkan',entrypoint=server.name,
                archive=asset(archive),files=[asset(p) for p in runtime.iterdir()])))
            self.assertEqual(component_identity(manifest,archive,server,'vulkan')['files_verified'],2)
            server.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'checksum'):
                component_identity(manifest,archive,server,'vulkan')

    def test_runtime_environment_cannot_inherit_sdk_or_driver_overrides(self):
        env, removed = acceptance_environment(Path("component"), dict(SystemRoot=r"C:\Windows", PATH="developer-sdk",
            CUDA_PATH="sdk", CUDA_PATH_V12_8="sdk", VULKAN_SDK="sdk", VK_ICD_FILENAMES="driver.json",
            VK_DRIVER_FILES="another.json", VK_ADD_DRIVER_FILES="third.json", GGML_BACKEND_PATH="plugins", LLAMA_ARG_MODEL="other-model"))
        self.assertNotIn("developer-sdk", env["PATH"])
        self.assertTrue({"CUDA_PATH", "CUDA_PATH_V12_8", "VULKAN_SDK", "VK_DRIVER_FILES", "VK_ICD_FILENAMES"}.issubset(removed))
        self.assertNotIn("LLAMA_ARG_MODEL", env)

    def test_external_cuda_dependency_and_missing_driver_do_not_pass(self):
        with patch.dict(os.environ, dict(SystemRoot=str(Path("windows").resolve()))):
            with patch("windows_runtime_support.loaded_modules", return_value=[Path("sdk/cublas64_12.dll")]):
                with self.assertRaisesRegex(RuntimeError, "outside component"):
                    audit_runtime_modules(0, "component", "cuda")
            with patch("windows_runtime_support.loaded_modules", return_value=[]):
                with self.assertRaisesRegex(RuntimeError, "not observed"):
                    audit_runtime_modules(0, "component", "cuda")

    @unittest.skipUnless(os.name == "nt", "native Windows API")
    def test_native_module_inventory_includes_windows_kernel(self):
        self.assertIn("kernel32.dll", {p.name.lower() for p in loaded_modules(os.getpid())})
