import importlib.util, json, pathlib, tempfile, unittest

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
