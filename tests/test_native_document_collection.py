"""Model-free controls for the installed-package document release gate."""
import importlib.util
import json
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_smoke", ROOT / "scripts/native-smoke.py")
SMOKE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SMOKE)


class DocumentCollectionTests(unittest.TestCase):
    def exercise(self, failed=()):
        calls = []
        with tempfile.TemporaryDirectory() as folder:
            results = pathlib.Path(folder)

            def completed(argv, **kwargs):
                case = argv[argv.index("--case") + 1]
                calls.append(case)
                code = int(case in failed)
                output = pathlib.Path(argv[argv.index("--output") + 1])
                output.write_text(json.dumps(dict(
                    schema=3, fixture_cases=[case], passed=not code,
                    cases=[dict(case=case, passed=not code, errors=["No document"] if code else [],
                                failure_kind="expectation" if code else None,
                                runs=[dict(process=dict(code=0, failure=None))])],
                )))
                if kwargs.get("check") and code:
                    raise subprocess.CalledProcessError(code, argv)
                return subprocess.CompletedProcess(argv, code)

            with patch.object(SMOKE.subprocess, "run", side_effect=completed):
                try:
                    SMOKE.run_document_cases(pathlib.Path("fixture-cli"), {}, results,
                                             [dict(name="first"), dict(name="second")])
                    failed_gate = False
                except (RuntimeError, subprocess.CalledProcessError):
                    failed_gate = True
        return calls, failed_gate

    def test_semantic_failure_collects_next_case_but_still_fails_release(self):
        calls, failed_gate = self.exercise(failed={"first"})
        self.assertEqual(calls, ["first", "second"])
        self.assertTrue(failed_gate)

    def test_all_passing_cases_run_once_and_pass(self):
        calls, failed_gate = self.exercise()
        self.assertEqual(calls, ["first", "second"])
        self.assertFalse(failed_gate)


if __name__ == "__main__":
    unittest.main()
