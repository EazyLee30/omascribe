"""Release policy/transactions with fictional packages; no signing or remote writes."""
import importlib.util
import gzip
import io
import itertools
import json
import pathlib
import tempfile
import unittest
import zipfile
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("acceleration_release", ROOT / "scripts/acceleration-release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class AccelerationReleaseTests(unittest.TestCase):
    def event(self, labels=(), fork=False):
        return dict(pull_request=dict(head=dict(sha="a"*40, repo=dict(full_name="external/fork" if fork else release.REPOSITORY)),
                    base=dict(sha="b"*40), labels=[dict(name=name) for name in labels]))

    def test_exact_pr_head_and_candidate_requires_both_labels_and_same_repository(self):
        labels = ["build-acceleration-components", "sign-acceleration-candidate", "build-windows-vulkan"]
        for switches in itertools.product([False, True], repeat=3):
            selected = [label for enabled, label in zip(switches, labels) if enabled]
            for fork in [False, True]:
                context = release.context(self.event(selected, fork), "pull_request", release.REPOSITORY, "c"*40, 123)
                self.assertEqual(context["source_sha"], "a"*40)  # Never the synthetic merge commit.
                self.assertEqual(context["sign"], switches[0] and switches[1] and not fork)
                self.assertFalse(context["promote"])
                self.assertEqual(context["version"], "acceleration-candidate-123" if context["sign"] else "acceleration-ci-123")
        with self.assertRaises(ValueError):
            release.context(self.event(labels), "pull_request", release.REPOSITORY, "c"*40, 123, promote=True)

    def test_reuse_signing_is_one_shot_same_repo_and_never_promotes(self):
        event = {**self.event(), "action":"labeled", "label":dict(name="sign-acceleration-run-123")}
        selected = release.reuse_context(event, "pull_request", release.REPOSITORY)
        self.assertEqual(selected["version"], "acceleration-ci-123")
        self.assertEqual(selected["reuse_run_id"], "123")
        self.assertFalse(selected["promote"])
        for name in ["build-acceleration-components", "sign-acceleration-run-0", "sign-acceleration-run-../escape"]:
            with self.assertRaises(ValueError):
                release.reuse_context({**event,"label":dict(name=name)}, "pull_request", release.REPOSITORY)
        with self.assertRaises(ValueError):
            release.reuse_context({**event,"pull_request":self.event(fork=True)["pull_request"]}, "pull_request", release.REPOSITORY)
        with self.assertRaises(ValueError):
            release.reuse_context({**event,"action":"synchronize"}, "pull_request", release.REPOSITORY)

    def test_native_reuse_fingerprint_includes_sdk_runner_and_build_sources(self):
        workflow = (ROOT / ".github/workflows/acceleration-components.yml").read_bytes()
        inputs = {path:(ROOT/path).read_bytes() for path in ["models.lock.json", "scripts/acceleration-components.py", "scripts/runtime_inventory.py"]}
        inputs[".github/workflows/acceleration-components.yml"] = workflow
        original = release.native_fingerprint(inputs.__getitem__)
        lf = workflow.replace(b"\r\n", b"\n")
        for ending in [lf, lf.replace(b"\n", b"\r\n")]:
            self.assertEqual(release.native_fingerprint({**inputs, ".github/workflows/acceleration-components.yml":ending}.__getitem__), original)
        for path, change in [("models.lock.json", lambda b:b+b" "),
                             ("scripts/acceleration-components.py", lambda b:b+b"# changed build\n"),
                             (".github/workflows/acceleration-components.yml", lambda b:b.replace(b'cuda: "12.8.1"', b'cuda: "12.9.0"')),
                             (".github/workflows/acceleration-components.yml", lambda b:b.replace(b'ubuntu-22.04', b'ubuntu-24.04'))]:
            updated = {**inputs, path:change(inputs[path])}
            self.assertNotEqual(release.native_fingerprint(updated.__getitem__), original)
        altered = workflow.replace(b'ref: ${{ needs.prepare.outputs.source_sha }}', b'ref: another-branch')
        with self.assertRaises(ValueError):
            release.native_fingerprint({**inputs, ".github/workflows/acceleration-components.yml":altered}.__getitem__)

    def test_reuse_refuses_unfinished_partial_expired_or_changed_native_inputs(self):
        names = {f"accelerator-{target}-{backend}" for target in release.packaging.TARGETS for backend in release.packaging.BACKENDS}|{"acceleration-catalog"}
        for failure in [None,"unfinished","partial","expired","native","checkout"]:
            with tempfile.TemporaryDirectory() as temp:
                directory = pathlib.Path(temp)
                run = dict(status="completed",conclusion="success",path=".github/workflows/acceleration-components.yml",
                           head_repository=dict(full_name=release.REPOSITORY),head_sha="b"*40,
                           pull_requests=[dict(head=dict(sha="f"*40),base=dict(sha="e"*40))])  # This API field follows today's PR, not the old run.
                artifacts = [dict(name=name,id=n,expired=False) for n,name in enumerate(sorted(names),1)]
                jobs=[dict(name=f"build (fixture-{i})",id=i,conclusion="success") for i in range(4)]
                if failure == "unfinished":run["status"] = "in_progress"
                if failure == "partial":artifacts.pop()
                if failure == "expired":artifacts[0]["expired"] = True
                if failure == "checkout":jobs.pop()
                def output(argv, **kwargs):
                    if "artifacts?" in argv[-1]:return json.dumps(dict(artifacts=artifacts))
                    if "jobs?" in argv[-1]:return json.dumps(dict(jobs=jobs))
                    return json.dumps(run)
                with patch.object(release.subprocess,"check_output",side_effect=output), patch.object(release.subprocess,"run"), \
                     patch.object(release,"checkout_sha_for_job",return_value="c"*40), \
                     patch.object(release,"native_fingerprint",side_effect=["old","new" if failure=="native" else "old","old"]):
                    selected=dict(reuse_run_id="123",source_sha="a"*40,version="acceleration-ci-123")
                    if failure:
                        with self.assertRaises(ValueError):release.check_reuse(selected,directory)
                    else:
                        release.check_reuse(selected,directory)
                        evidence=json.loads((directory/"candidate-build-evidence.json").read_text())
                        self.assertEqual(evidence["build_source_sha"],"b"*40)
                        self.assertEqual(evidence["checkout_source_sha"],"c"*40)
                        self.assertEqual(len(evidence["artifact_ids"]),5)
                self.assertEqual((directory/"candidate-build-evidence.json").exists(), failure is None)

    def test_checkout_log_uses_the_actual_original_commit_on_both_platforms(self):
        for executable in ['/usr/bin/git','"C:\\Program Files\\Git\\cmd\\git.exe"']:
            log=f'2026-10-26T17:00:00Z [command]{executable} log -1 --format=%H\n2026-10-26T17:00:00Z '+"a"*40+'\n'
            self.assertEqual(release.checkout_sha(log.encode()),"a"*40)
        with self.assertRaises(ValueError):release.checkout_sha(b'no checkout source\n')

    def test_log_failure_diagnostics_do_not_expose_download_urls_or_contents(self):
        diagnostic=release.log_read_diagnostic(b"",b'gh: request failed (HTTP 403) signed-url?secret=private',1)
        self.assertEqual(diagnostic,dict(bytes=0,kind="empty",exit_code=1,http_status=403))
        self.assertNotIn("secret",str(diagnostic))
        zipped=io.BytesIO()
        with zipfile.ZipFile(zipped,"w") as archive: archive.writestr("log.txt","private")
        for data,kind in [(zipped.getvalue(),"zip"),(gzip.compress(b"private"),"gzip"),(b'{"private":true}',"json"),(b'private log text',"text")]:
            self.assertEqual(release.log_read_diagnostic(data,b"",-15)["kind"],kind)

    def test_reused_catalog_payload_must_match_the_original_before_signing(self):
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp)
            (root/"index.payload").write_bytes(b"exact signed input")
            (root/"original-index.payload").write_bytes(b"exact signed input")
            release.compare_reuse(root)
            (root/"original-index.payload").write_bytes(b"different")
            with self.assertRaises(ValueError):release.compare_reuse(root)

    def test_manual_candidate_and_pointer_promotion_are_separate_explicit_actions(self):
        args = ({}, "workflow_dispatch", release.REPOSITORY, "a"*40, 123, "acceleration-candidate-test")
        self.assertFalse(release.context(*args)["sign"])
        signed = release.context(*args, publish=True)
        self.assertTrue(signed["sign"])
        self.assertFalse(signed["promote"])
        self.assertTrue(release.context(*args, publish=True, promote=True)["promote"])
        with self.assertRaises(ValueError):
            release.context(*args, promote=True)
        for version in ["../escape", "acceleration-v1", "acceleration-test\nvalue", "v0.3.4", "acceleration-"+"a"*75]:
            with self.assertRaises(ValueError):
                release.context({}, "workflow_dispatch", release.REPOSITORY, "a"*40, 1, version, True)

    def packages(self, folder, version="acceleration-candidate-fixture"):
        packages = folder / "packages"
        for target in sorted(release.packaging.TARGETS):
            for backend in sorted(release.packaging.BACKENDS):
                runtime = folder / (target+backend)
                runtime.mkdir()
                (runtime / ("llama-server.exe" if target.startswith("windows") else "llama-server")).write_bytes(b"fictional, never execute")
                (runtime / "llama.cpp-LICENSE.txt").write_text("fictional license")
                release.packaging.write_json(runtime / "build-info.json", dict(revision=release.packaging.LOCK["revision"], backend=backend, target=target, format=1))
                if backend == "cuda":
                    (runtime / "CUDA-EULA.html").write_text("fictional license")
                    (runtime / ("cudart64_12.dll" if target.startswith("windows") else "libcudart.so.12")).write_bytes(b"fictional runtime")
                release.packaging.package(runtime, backend, target, version, packages)
        release.packaging.index(packages, version)
        (packages / "index.payload.sig").write_text("fictional signature; native verifier is mocked only in transaction tests")
        (packages / "updater.pub").write_text(json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())["plugins"]["updater"]["pubkey"])
        (packages / "index.json").write_text(json.dumps(dict(payload=(packages/"index.payload").read_bytes().decode("utf-8"), signature=(packages/"index.payload.sig").read_text())))
        return packages

    def remote(self, packages, wrong_digest=False):
        files = [*packages.glob("*.tar.bz2"), packages/"index.json", packages/"candidate-provenance.json"]
        assets = [dict(name=p.name, state="uploaded", size=p.stat().st_size, digest="sha256:"+release.packaging.digest(p)) for p in files]
        if wrong_digest:
            assets[0]["digest"] = "sha256:" + "0"*64
        return dict(draft=True, assets=assets)

    def test_candidate_upload_remains_draft_until_every_remote_asset_matches(self):
        for wrong in [False, True]:
            with tempfile.TemporaryDirectory() as temp:
                packages = self.packages(pathlib.Path(temp))
                version = "acceleration-candidate-fixture"
                with patch.object(release.subprocess, "check_output", return_value=json.dumps(dict(verified=True, version=version))), \
                     patch.object(release.subprocess, "run") as run, \
                     patch.object(release, "require_tag") as tag, \
                     patch.object(release, "release", side_effect=lambda tag, missing_ok=False: None if missing_ok else self.remote(packages, wrong)):
                    if wrong:
                        with self.assertRaisesRegex(ValueError, "digest/size"):
                            release.publish(packages, version, "a"*40, pathlib.Path("fictional-cli"))
                    else:
                        release.publish(packages, version, "a"*40, pathlib.Path("fictional-cli"))
                    commands = [call.args[0] for call in run.call_args_list]
                    tag.assert_called_once_with(version, "a"*40)
                    self.assertIn("--draft", commands[0])
                    self.assertIn("--verify-tag", commands[0])
                    self.assertEqual(any("edit" in c for c in commands), not wrong)
                    self.assertTrue(all("acceleration-v1" not in c for c in commands))

    def test_existing_release_corrupt_archive_or_wrong_key_never_reaches_upload(self):
        for failure in ["existing", "archive", "key", "envelope"]:
            with tempfile.TemporaryDirectory() as temp:
                packages = self.packages(pathlib.Path(temp))
                version = "acceleration-candidate-fixture"
                if failure == "archive":
                    next(packages.glob("*.tar.bz2")).write_bytes(b"tampered")
                elif failure == "key":
                    (packages / "updater.pub").write_text("wrong key")
                elif failure == "envelope":
                    (packages / "index.json").write_text('{}')
                with patch.object(release.subprocess, "check_output", return_value=json.dumps(dict(verified=True, version=version))), \
                     patch.object(release.subprocess, "run") as run, patch.object(release, "release", return_value={}):
                    with self.assertRaises(ValueError):
                        release.publish(packages, version, "a"*40, pathlib.Path("fictional-cli"))
                    run.assert_not_called()

    def test_signature_verifier_failure_and_incomplete_matrix_stop_publication(self):
        with tempfile.TemporaryDirectory() as temp:
            packages = self.packages(pathlib.Path(temp))
            with patch.object(release.subprocess, "check_output", return_value='{"verified":false}'), \
                 patch.object(release.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    release.publish(packages, "acceleration-candidate-fixture", "a"*40, pathlib.Path("fictional-cli"))
                run.assert_not_called()
            next(packages.glob("*.component.json")).unlink()
            with patch.object(release.subprocess, "check_output") as verifier:
                with self.assertRaises(FileNotFoundError):
                    release.verify_catalog(packages, "acceleration-candidate-fixture", pathlib.Path("fictional-cli"))
                verifier.assert_not_called()

    def test_exact_source_latest_application_ci_is_required(self):
        for status, conclusion, sha in [("completed", "success", "a"*40), ("in_progress", None, "a"*40), ("completed", "failure", "a"*40), ("completed", "success", "b"*40)]:
            with patch.object(release.subprocess, "check_output", return_value=json.dumps(dict(workflow_runs=[dict(status=status, conclusion=conclusion, head_sha=sha)]))):
                if status == "completed" and conclusion == "success" and sha == "a"*40:
                    release.require_green_ci("a"*40)
                else:
                    with self.assertRaises(ValueError):
                        release.require_green_ci("a"*40)

    def test_windows_source_checks_cannot_be_borrowed_from_another_head(self):
        for conclusion in ["success","failure",None]:
            responses=[dict(workflow_runs=[dict(head_sha="b"*40,id=1),dict(head_sha="a"*40,id=2)]),
                       dict(jobs=[dict(name="windows-core",conclusion=conclusion)])]
            with patch.object(release.subprocess,"check_output",side_effect=[json.dumps(v) for v in responses]) as query:
                if conclusion=="success":release.require_windows_core("a"*40)
                else:
                    with self.assertRaises(ValueError):release.require_windows_core("a"*40)
                self.assertIn("/runs/2/jobs?",query.call_args.args[0][-1])

    def test_existing_drafts_are_found_and_tags_cannot_be_retargeted(self):
        draft=dict(tag_name="acceleration-ci-123",draft=True,assets=[])
        with patch.object(release.subprocess,"check_output",return_value=json.dumps([[draft]])):
            self.assertEqual(release.release("acceleration-ci-123"),draft)
            self.assertIsNone(release.release("missing",missing_ok=True))
        with patch.object(release.subprocess,"check_output",return_value=json.dumps(dict(object=dict(type="commit",sha="a"*40)))):
            release.require_tag("acceleration-ci-123","a"*40)
            with self.assertRaises(ValueError):release.require_tag("acceleration-ci-123","b"*40)


if __name__ == "__main__":
    unittest.main()
