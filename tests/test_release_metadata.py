import importlib.util, json, pathlib, tempfile, unittest, sys
sys.path.insert(0, str(pathlib.Path(__file__).parents[1] / 'scripts'))

spec = importlib.util.spec_from_file_location('manifest', pathlib.Path(__file__).parents[1] / 'scripts/update-manifest.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class UpdateManifestTests(unittest.TestCase):
    def test_partial_or_wrong_version_native_build_cannot_publish_updates(self):
        with tempfile.TemporaryDirectory() as root:
            assets = pathlib.Path(root)
            with self.assertRaises(AssertionError):
                module.manifest(assets, '0.3.1', 'Notes')
            for suffix in module.TARGETS.values():
                name = f'OmaScribe_0.3.1_{suffix}'
                (assets/name).write_bytes(b'payload')
                (assets/(name+'.sig')).write_text('signature')
                (assets/(name+'.verified.json')).write_text(json.dumps({'file':name,'version':'0.3.1','verified':True}))
            self.assertEqual(set(module.manifest(assets, '0.3.1', 'Notes')['platforms']), set(module.TARGETS))
            (assets/(name+'.verified.json')).write_text(json.dumps({'file':name,'version':'0.3.0','verified':True}))
            with self.assertRaises(AssertionError):
                module.manifest(assets, '0.3.1', 'Notes')

    def test_runtime_updates_use_separate_signed_assets(self):
        with tempfile.TemporaryDirectory() as root:
            assets=pathlib.Path(root)
            for suffix in module.TARGETS.values():
                name=f'OmaScribe_0.3.3_{suffix}'.replace('.app.tar.gz','-runtime.app.tar.gz').replace('.exe','-runtime.exe').replace('.AppImage','-runtime.AppImage')
                (assets/name).write_bytes(b'runtime only')
                (assets/(name+'.sig')).write_text('signature')
                (assets/(name+'.verified.json')).write_text(json.dumps({'file':name,'version':'0.3.3','verified':True}))
            result=module.manifest(assets,'0.3.3','Notes',runtime=True)
            self.assertEqual(len(result['platforms']),4)
            for data in result['platforms'].values():
                self.assertIn('/v0.3.3-updates/',data['url'])
                self.assertIn('-runtime.',data['url'])
            with self.assertRaises(AssertionError):module.manifest(assets,'0.3.3','Notes')


class RuntimeInventoryTests(unittest.TestCase):
    def test_payload_inventory_rejects_models_but_allows_runtime_libraries(self):
        spec = importlib.util.spec_from_file_location("runtime_update", pathlib.Path(__file__).parents[1] / "scripts/native-runtime-update.py")
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        runtime.assert_runtime_inventory("Path = update.exe\nPath = native\\runtime\\sherpa-onnx-c-api.dll\n")
        for member in ["native\\models\\tokens.txt", "native\\weights.gguf", "native\\encoder.onnx"]:
            with self.assertRaises(AssertionError): runtime.assert_runtime_inventory("Path = update.exe\nPath = " + member)
