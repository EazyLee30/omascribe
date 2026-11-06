"""Run the production frontend save-queue regression in the existing test suite."""
import pathlib
import shutil
import subprocess
import unittest


class NoteSaveQueueTests(unittest.TestCase):
    def test_failed_saves_do_not_restore_an_outdated_draft(self):
        root = pathlib.Path(__file__).resolve().parents[1]
        node = shutil.which("node")
        if node is None or not (root / "node_modules/typescript").exists():
            self.skipTest("Frontend dependencies are required (npm ci)")
        result = subprocess.run(
            [node, "--test", "tests/note-save-queue.mjs"],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
