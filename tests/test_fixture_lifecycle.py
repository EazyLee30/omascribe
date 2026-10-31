import hashlib
import io
import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch

from pinned_download import download
import harness_support


class PublicDownloadTests(unittest.TestCase):
    def test_prepared_corpus_reuse_requires_prior_manifest_and_audio_hashes_without_network(self):
        import json
        import prepare_asr_fixtures as fixtures
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            (root / "speech.wav").write_bytes(b"fictional audio bytes")
            manifest = dict(dataset="google/fleurs", revision=fixtures.REVISION, license="CC-BY-4.0",
                            cases=[dict(name="fixture", file="speech.wav", sha256=fixtures.digest(root/"speech.wav"), reference="fictional gold")])
            path = root / "manifest.json"
            path.write_text(json.dumps(manifest))
            expected = fixtures.digest(path)
            with patch.object(fixtures.urllib.request, "urlopen", side_effect=AssertionError("Offline only")):
                self.assertEqual(fixtures.verify_existing(root, expected), manifest)
                (root / "speech.wav").write_bytes(b"tampered")
                with self.assertRaisesRegex(ValueError, "audio checksum"):
                    fixtures.verify_existing(root, expected)
                manifest["cases"][0]["reference"] = "edited references cannot reuse the old baseline"
                path.write_text(json.dumps(manifest))
                with self.assertRaisesRegex(ValueError, "manifest checksum"):
                    fixtures.verify_existing(root, expected)

    def test_short_response_resumes_at_actual_byte_offset(self):
        payload = b"abcdefgh"
        requests = []
        def fetch(request, timeout):
            value = request.get_header("Range")
            requests.append(value)
            start, end = map(int, value.removeprefix("bytes=").split("-"))
            response = io.BytesIO(payload[start:end+1] if len(requests) > 1 else payload[:2])
            response.status = 206
            response.headers = {"Content-Range": f"bytes {start}-{end}/{len(payload)}", "Content-Length": str(end-start+1)}
            return response
        with tempfile.TemporaryDirectory() as folder, patch("pinned_download.urllib.request.urlopen", fetch):
            path = pathlib.Path(folder) / "public.tar.gz"
            download("https://fixture.invalid/pinned", path, len(payload), hashlib.sha256(payload).hexdigest(), block_bytes=4)
            self.assertEqual(path.read_bytes(), payload)
            self.assertEqual(requests, ["bytes=0-3", "bytes=2-5", "bytes=6-7"])
            self.assertFalse(path.with_suffix(".gz.part").exists())
            download("https://fixture.invalid/pinned", path, len(payload), hashlib.sha256(payload).hexdigest())
            self.assertEqual(len(requests), 3)

    def test_wrong_range_and_wrong_hash_cannot_be_promoted(self):
        payload = b"abcdefgh"
        def response(start):
            result = io.BytesIO(payload[start:])
            result.status = 206
            result.headers = {"Content-Range": f"bytes {start}-7/8", "Content-Length": str(8-start)}
            return result
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder) / "public.tar.gz"
            part = path.with_suffix(".gz.part")
            part.write_bytes(b"xxx")
            with patch("pinned_download.urllib.request.urlopen", return_value=response(0)):
                with self.assertRaisesRegex(ValueError, "range does not match"):
                    download("https://fixture.invalid/pinned", path, 8, hashlib.sha256(payload).hexdigest())
            self.assertEqual(part.read_bytes(), b"xxx")
            with patch("pinned_download.urllib.request.urlopen", return_value=response(3)):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    download("https://fixture.invalid/pinned", path, 8, hashlib.sha256(payload).hexdigest())
            self.assertFalse(path.exists())
            self.assertEqual(path.with_suffix(".gz.rejected").read_bytes(), b"xxxdefgh")


class BorrowedModelTests(unittest.TestCase):
    def test_only_fresh_marked_copies_are_released_and_source_stays_intact(self):
        with tempfile.TemporaryDirectory() as folder:
            base = pathlib.Path(folder)
            source = base / "source"
            source.mkdir()
            (source / "weight.gguf").write_bytes(b"fictional-model")
            root = base / "run"
            with patch.dict(os.environ, OMASCRIBE_TEST_MODEL_STORE=str(source)), patch.object(harness_support.atexit, "register") as register:
                harness_support.isolated_env(root)
            callback, target, owner = register.call_args.args
            self.assertEqual((target / "weight.gguf").read_bytes(), b"fictional-model")
            callback(target, "wrong-owner")
            self.assertTrue(target.exists())
            callback(target, owner)
            self.assertFalse(target.exists())
            self.assertEqual((source / "weight.gguf").read_bytes(), b"fictional-model")
            self.assertTrue(harness_support.read_json(root / "model-copy-cleanup.json")["released"])

    def test_existing_model_store_is_neither_overwritten_nor_scheduled_for_cleanup(self):
        with tempfile.TemporaryDirectory() as folder:
            base = pathlib.Path(folder)
            source = base / "source"
            source.mkdir()
            (source / "weight.gguf").write_bytes(b"new")
            root = base / "existing"
            target = root / "model-store"
            target.mkdir(parents=True)
            (target / "weight.gguf").write_bytes(b"original")
            with patch.dict(os.environ, OMASCRIBE_TEST_MODEL_STORE=str(source)), patch.object(harness_support.atexit, "register") as register:
                harness_support.isolated_env(root)
            register.assert_not_called()
            self.assertEqual((target / "weight.gguf").read_bytes(), b"original")


if __name__ == "__main__":
    unittest.main()
