#!/usr/bin/env python3
"""Exact-source candidate publication; no key material, SDKs or app installers here."""
import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = "EazyLee30/omascribe"
spec = importlib.util.spec_from_file_location("component_packaging", ROOT / "scripts/acceleration-components.py")
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


def version_ok(version):
    return (packaging.version_ok(version) and version != "acceleration-v1"
            and all(packaging.name_ok(f"OmaScribe-{backend}-{target}-{version}.tar.bz2")
                    for backend in packaging.BACKENDS for target in packaging.TARGETS))


def context(event, event_name, repository, sha, run_id, version="", publish=False, promote=False):
    """Use the same exact head for checks/builds; signing needs explicit authority."""
    if repository != REPOSITORY or not re.fullmatch(r"[1-9][0-9]*", str(run_id)):
        raise ValueError("Unsupported repository/run")
    if event_name == "pull_request":
        pr = event["pull_request"]
        sha = pr["head"]["sha"]
        labels = {label["name"] for label in pr["labels"]}
        publish = (pr["head"]["repo"]["full_name"] == repository
                   and {"build-acceleration-components", "sign-acceleration-candidate"} <= labels)
        if promote:
            raise ValueError("A PR cannot update the stable catalog pointer")
        version = f"acceleration-{'candidate' if publish else 'ci'}-{run_id}"
    elif event_name != "workflow_dispatch":
        raise ValueError("Unsupported release event")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or not version_ok(version):
        raise ValueError("Invalid source/version")
    if promote and not publish:
        raise ValueError("Promotion requires explicit signed publication")
    return dict(source_sha=sha, version=version, sign=bool(publish), promote=bool(promote))


def execution_context():
    if os.environ.get("REUSE_CANDIDATE") == "true":
        event = json.loads(pathlib.Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        return reuse_context(event, os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REPOSITORY"])
    return context(json.loads(pathlib.Path(os.environ["GITHUB_EVENT_PATH"]).read_text()),
                   os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REPOSITORY"],
                   os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"],
                   os.environ.get("REQUESTED_VERSION", ""), os.environ.get("SIGN_REQUESTED") == "true",
                   os.environ.get("PROMOTE_REQUESTED") == "true")


def reuse_context(event, event_name, repository):
    match = re.fullmatch(r"sign-acceleration-run-([1-9][0-9]{0,19})", event.get("label", {}).get("name", ""))
    pr = event.get("pull_request", {})
    if (event_name != "pull_request" or event.get("action") != "labeled" or not match
            or repository != REPOSITORY or pr.get("head", {}).get("repo", {}).get("full_name") != repository
            or not re.fullmatch(r"[0-9a-f]{40}", pr.get("head", {}).get("sha", ""))):
        raise ValueError("Reuse signing requires a one-shot label on a same-repository PR")
    return dict(source_sha=pr["head"]["sha"], version="acceleration-ci-"+match[1],
                sign=True, promote=False, reuse_run_id=match[1])


def native_fingerprint(read):
    """Compiled inputs, runner matrix and exact SDK/build/package commands.

    Delivery orchestration and newly added acceptance tests are outside the
    native compiler inputs. Both recognized checkout forms bind the checked
    source through either the event's ancestor base or prepare's exact head.
    """
    workflow = read(".github/workflows/acceleration-components.yml").decode()
    build = workflow.split("  build:\n", 1)[1].split("  catalog:\n", 1)[0]
    checkout = build.split("    steps:\n", 1)[1].split("      - uses: actions/setup-python@", 1)[0]
    recognized = ["      - uses: actions/checkout@v4\n", "      - uses: actions/checkout@v4\n        with:\n          ref: ${{ needs.prepare.outputs.source_sha }}\n"]
    if checkout not in recognized:
        raise ValueError("Unrecognized native source checkout; cannot reuse its artifacts")
    parts = {path: read(path) for path in ["models.lock.json", "scripts/acceleration-components.py", "scripts/runtime_inventory.py"]}
    parts["runner-matrix"] = build.split("    strategy:\n", 1)[1].split("    steps:\n", 1)[0].encode()
    parts["sdk-build-package"] = build.split("      - name: Pinned CMake for Blackwell architecture suffixes\n", 1)[1].split("      - name: Base installer directory must remain untouched\n", 1)[0].encode()
    digest = hashlib.sha256()
    for key, value in sorted(parts.items()):
        digest.update(key.encode()+b"\0"+value+b"\0")
    return digest.hexdigest()


def require_windows_core(sha):
    runs = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/actions/workflows/acceleration-components.yml/runs?head_sha={sha}&per_page=100"], text=True))["workflow_runs"]
    for run in runs:
        if run["head_sha"] != sha:
            continue
        jobs = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/actions/runs/{run['id']}/jobs?per_page=100"], text=True))["jobs"]
        for job in jobs:
            if job["name"] == "windows-core" and job.get("conclusion") is not None:
                if job["conclusion"] == "success":
                    return
                raise ValueError("Windows-core failed for the current source")
    raise ValueError("Current source needs successful Windows-core checks before candidate signing")


def check_reuse(selected, directory):
    def api(path):
        return json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/{path}"], text=True))
    run_id = selected["reuse_run_id"]
    run = api("actions/runs/"+run_id)
    if (run["status"] != "completed" or run["conclusion"] != "success"
            or run["path"] != ".github/workflows/acceleration-components.yml"
            or run["head_repository"]["full_name"] != REPOSITORY):
        raise ValueError("Original complete component workflow has not passed")
    sha = run["head_sha"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid original source")
    matching_prs = [pr for pr in run["pull_requests"] if pr["head"]["sha"] == sha]
    if len(matching_prs) != 1:
        raise ValueError("Original PR source/base evidence is missing")
    subprocess.run(["git", "merge-base", "--is-ancestor", matching_prs[0]["base"]["sha"], sha], check=True)
    def at(commit):
        return native_fingerprint(lambda path: subprocess.check_output(["git", "show", f"{commit}:{path}"]))
    fingerprint = at(sha)
    if at(selected["source_sha"]) != fingerprint:
        raise ValueError("Native compiler/SDK/package inputs changed; cannot reuse old binaries")
    artifacts = api(f"actions/runs/{run_id}/artifacts?per_page=100")["artifacts"]
    required = {f"accelerator-{target}-{backend}" for target in packaging.TARGETS for backend in packaging.BACKENDS} | {"acceleration-catalog"}
    usable = [a for a in artifacts if a["name"] in required and not a["expired"]]
    if len(usable) != 5 or {a["name"] for a in usable} != required:
        raise ValueError("Complete non-expired four-way artifacts and catalog are required")
    directory.mkdir(parents=True, exist_ok=True)
    packaging.write_json(directory / "candidate-build-evidence.json", dict(schema=1,
        publishing_source_sha=selected["source_sha"], build_source_sha=sha, native_input_sha256=fingerprint,
        build_run_id=run_id, artifact_ids={a["name"]:a["id"] for a in usable}, version=selected["version"]))


def require_green_ci(sha):
    runs = json.loads(subprocess.check_output([
        "gh", "api", f"repos/{REPOSITORY}/actions/workflows/ci.yml/runs?head_sha={sha}&per_page=100"
    ], text=True))["workflow_runs"]
    matching = [run for run in runs if run["head_sha"] == sha]
    if not matching or matching[0]["status"] != "completed" or matching[0]["conclusion"] != "success":
        raise ValueError("Application CI for this exact source must pass before signing")


def verify_catalog(directory, version, cli):
    directory = pathlib.Path(directory)
    before = (directory / "index.payload").read_bytes()
    if (directory / "candidate-build-evidence.json").exists():
        compare_reuse(directory)
    index = packaging.index(directory, version)  # All four archives and every member hash.
    if (directory / "index.payload").read_bytes() != before:
        raise ValueError("Signed catalog differs from the complete package inventory")
    envelope = json.loads((directory / "index.json").read_text())
    signature = (directory / "index.payload.sig").read_text().strip()
    if envelope != dict(payload=before.decode("utf-8"), signature=signature):
        raise ValueError("Envelope differs from exact signed bytes")
    expected_key = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())["plugins"]["updater"]["pubkey"]
    if (directory / "updater.pub").read_text().strip() != expected_key:
        raise ValueError("Catalog must use the embedded application release key")
    verified = json.loads(subprocess.check_output([
        str(cli), "verify-update", str(directory / "index.payload"), str(directory / "index.payload.sig"),
        str(directory / "updater.pub"), version
    ], text=True))
    if verified != dict(verified=True, version=version):
        raise ValueError("App verifier did not accept the catalog")
    return index


def compare_reuse(directory):
    if (directory / "index.payload").read_bytes() != (directory / "original-index.payload").read_bytes():
        raise ValueError("Reused candidate payload differs from the original complete catalog")


def release(tag, missing_ok=False):
    # The tag lookup endpoint describes published releases. An authenticated
    # listing includes drafts and prevents duplicate/hidden partial candidates.
    pages = json.loads(subprocess.check_output([
        "gh", "api", "--paginate", "--slurp", f"repos/{REPOSITORY}/releases?per_page=100"
    ], text=True))
    matching = [item for page in pages for item in page if item["tag_name"] == tag]
    if len(matching) > 1 or (not matching and not missing_ok):
        raise ValueError("Release lookup is missing or ambiguous")
    return matching[0] if matching else None


def require_tag(version, expected_sha=None):
    value = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/git/ref/tags/{version}"], text=True))
    if value["object"]["type"] != "commit" or not re.fullmatch(r"[0-9a-f]{40}", value["object"]["sha"]):
        raise ValueError("Expected a maintainer-created immutable lightweight candidate tag")
    if expected_sha is not None and value["object"]["sha"] != expected_sha:
        raise ValueError("Candidate tag does not bind the exact compiled source")


def publish(directory, version, source_sha, cli, promote=False):
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha) or not version_ok(version):
        raise ValueError("Invalid immutable source/version")
    directory = pathlib.Path(directory)
    index = verify_catalog(directory, version, cli)
    if release(version, missing_ok=True) is not None:
        raise ValueError("Immutable release exists; use a new version, including after a failed draft upload")
    evidence = json.loads((directory/"candidate-build-evidence.json").read_text()) if (directory/"candidate-build-evidence.json").exists() else None
    # GITHUB_TOKEN cannot create tags targeting commits with workflow changes.
    # A maintainer pre-creates this exact tag; the signer may never retarget it.
    require_tag(version, evidence["build_source_sha"] if evidence else source_sha)
    provenance = directory / "candidate-provenance.json"
    packaging.write_json(provenance, dict(schema=1, source_sha=source_sha, version=version,
        workflow_run=os.environ.get("GITHUB_RUN_ID"), runtime_revision=index["runtime_revision"],
        reused_build=evidence,
        catalog_sha256=packaging.digest(directory / "index.json"),
        note="Audit metadata; app trust comes from the signed catalog and archive/member hashes, not this unsigned note."))
    files = [directory / component["archive"]["name"] for component in index["components"]]
    files += [directory / "index.json", provenance]
    def run(*args):
        subprocess.run(["gh", "release", *map(str, args), "--repo", REPOSITORY], check=True)
    # A failed/partial upload stays a draft and is never exposed as a valid catalog.
    run("create", version, "--verify-tag", "--draft", "--prerelease", "--latest=false",
        "--title", "Optional GPU validation candidate · " + version,
        "--notes", "Signed optional GPU components for explicit device acceptance. No stable catalog promotion. Base installers do not include these files.")
    run("upload", version, *files)
    uploaded = release(version)
    assets = {asset["name"]: asset for asset in uploaded["assets"]}
    if not uploaded["draft"] or set(assets) != {p.name for p in files}:
        raise ValueError("Incomplete or unexpected remote candidate assets")
    for path in files:
        asset = assets[path.name]
        if asset["state"] != "uploaded" or asset["size"] != path.stat().st_size or asset.get("digest") != "sha256:" + packaging.digest(path):
            raise ValueError("Remote upload digest/size mismatch; candidate remains a draft")
    run("edit", version, "--draft=false", "--prerelease", "--latest=false")
    if promote:
        pointer = release("acceleration-v1", missing_ok=True)
        if pointer is not None and pointer["draft"]:
            raise ValueError("Stable pointer draft requires maintainer inspection")
        if pointer is None:
            require_tag("acceleration-v1")
            run("create", "acceleration-v1", "--verify-tag", "--prerelease", "--latest=false",
                "--title", "Optional acceleration catalog", "--notes", "Signed catalog consumed by OmaScribe settings.")
        run("upload", "acceleration-v1", directory / "index.json", "--clobber")
        pointer = release("acceleration-v1")
        uploaded_index = [a for a in pointer["assets"] if a["name"] == "index.json"]
        if len(uploaded_index) != 1 or uploaded_index[0].get("digest") != "sha256:" + packaging.digest(directory / "index.json"):
            raise ValueError("Stable pointer upload digest mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["context", "reuse-context", "check-ci", "check-reuse", "compare-reuse", "publish"])
    parser.add_argument("--directory", type=pathlib.Path, default=ROOT / "release-assets/accelerators")
    parser.add_argument("--cli", type=pathlib.Path, default=ROOT / "target/debug/omascribe-cli")
    args = parser.parse_args()
    selected = execution_context()
    if args.command in ["context", "reuse-context"]:
        if os.environ["GITHUB_EVENT_NAME"] == "pull_request" and selected["sign"]:
            event = json.loads(pathlib.Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
            subprocess.run(["git", "merge-base", "--is-ancestor", event["pull_request"]["base"]["sha"], selected["source_sha"]], check=True)
        with pathlib.Path(os.environ["GITHUB_OUTPUT"]).open("a") as out:
            for key, value in selected.items():
                out.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    elif not selected["sign"]:
        raise ValueError("Signed publication was not authorized by the workflow context")
    elif args.command == "check-ci":
        require_green_ci(selected["source_sha"])
        require_tag(selected["version"], selected["source_sha"])
    elif args.command == "check-reuse":
        require_green_ci(selected["source_sha"])
        require_windows_core(selected["source_sha"])
        check_reuse(selected, args.directory)
        evidence=json.loads((args.directory/"candidate-build-evidence.json").read_text())
        require_tag(selected["version"], evidence["build_source_sha"])
    elif args.command == "compare-reuse":
        if not selected.get("reuse_run_id"):
            raise ValueError("No original candidate was selected")
        compare_reuse(args.directory)
    else:
        require_green_ci(selected["source_sha"])
        if selected.get("reuse_run_id"):
            require_windows_core(selected["source_sha"])
            check_reuse(selected, args.directory)
        publish(args.directory, selected["version"], selected["source_sha"], args.cli, selected["promote"])


if __name__ == "__main__":
    main()
