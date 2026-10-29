"""Shared isolation and failure capture for real local inference tests (stdlib only)."""
import atexit, hashlib, json, os, pathlib, platform, shutil, signal, subprocess, tempfile, time, uuid
from process_resources import ProcessTreeSampler

REPO = pathlib.Path(__file__).resolve().parents[1]

def suite_passed(results, expected):
    return expected > 0 and len(results) == expected and all(r.get("passed") is True for r in results)

def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_suffix(path.suffix + ".part")
    part.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    part.replace(path)

def read_json(path):
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else None

def sha256(path):
    with pathlib.Path(path).open("rb") as f: return hashlib.file_digest(f, "sha256").hexdigest()

def release_borrowed_models(target, owner):
    """Only remove a model directory freshly created and marked by this harness."""
    marker = target / ".omascribe-harness-borrow.json"
    try:
        if target.is_symlink() or read_json(marker) != dict(schema=1, owner=owner):
            return
        logical_bytes = sum(p.stat().st_size for p in target.rglob("*") if p.is_file())
        shutil.rmtree(target)
        write_json(target.parent / "model-copy-cleanup.json", dict(released=True, logical_bytes=logical_bytes,
            note="Borrowed copies/links only; logical bytes are not guaranteed disk space reclaimed. Source model store retained."))
    except (OSError, ValueError) as error:
        write_json(target.parent / "model-copy-cleanup.json", dict(released=False, error=type(error).__name__))

def isolated_env(root):
    root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, OMASCRIBE_DATA_DIR=str(root),
        OMASCRIBE_MODELS_DIR=os.environ.get("OMASCRIBE_MODELS_DIR", str(pathlib.Path.home() / ".local/share/omascribe/models")),
        OMASCRIBE_RUNTIME_DIR=os.environ.get("OMASCRIBE_RUNTIME_DIR", str(REPO / "target/debug")))
    if os.environ.get("OMASCRIBE_TEST_MODEL_STORE"):
        def reuse(source, target):
            try: return os.link(source, target)
            except OSError: return shutil.copy2(source, target)
        target = root / "model-store"
        fresh = not target.exists()
        if fresh:
            target.mkdir()
            owner = uuid.uuid4().hex
            write_json(target / ".omascribe-harness-borrow.json", dict(schema=1, owner=owner))
            atexit.register(release_borrowed_models, target, owner)
            shutil.copytree(env["OMASCRIBE_TEST_MODEL_STORE"], target, copy_function=reuse, dirs_exist_ok=True)
            write_json(target / ".omascribe-harness-borrow.json", dict(schema=1, owner=owner))
    # These switches only govern optional external services. Core inference remains loopback.
    write_json(root / "network-policy.json", dict(allow_model_downloads=False, allow_online_recommendations=False))
    return env

def command(argv, env, directory, timeout=650, resource_samples=False):
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    options = {"start_new_session": True} if os.name != "nt" else {}
    with (directory / "stdout.txt").open("w", encoding="utf-8") as out, (directory / "stderr.txt").open("w", encoding="utf-8") as err:
        process = subprocess.Popen([str(a) for a in argv], env=env, stdout=out, stderr=err, **options)
        sampler = ProcessTreeSampler(process.pid, record_samples=True).start() if resource_samples else ProcessTreeSampler(process.pid).start()
        failure = None
        try: code = process.wait(timeout=timeout)
        except BaseException as error:
            if os.name == "nt": subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
            else:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            process.wait()
            code, failure = process.returncode, type(error).__name__
        finally:
            resources = sampler.finish()
    if resource_samples:
        write_json(directory / "resources.json", resources)
        resources = {key: value for key, value in resources.items() if key not in ("timeline", "observed_processes")}
        resources["timeline_file"] = "resources.json"
    write_json(directory / "process.json", dict(command=[str(a) for a in argv], code=code,
        seconds=round(time.monotonic()-started,3), failure=failure, resources=resources))
    if failure or code: raise RuntimeError(f"{failure or 'Exit ' + str(code)}; see {directory}")
    return json.loads((directory / "stdout.txt").read_text(encoding="utf-8"))

def source_digest():
    digest = hashlib.sha256()
    for folder, pattern in [("crates", "*.rs"), ("tests", "*.py"), ("tests/fixtures", "*.json")]:
        for path in sorted((REPO / folder).rglob(pattern)):
            digest.update(str(path.relative_to(REPO)).encode()); digest.update(b"\0"); digest.update(path.read_bytes())
    return digest.hexdigest()

def environment(cli):
    # Old comparison binaries may not support build-info and open Engine before failing.
    # Keep even that metadata request away from the user's workspace.
    build = None
    with tempfile.TemporaryDirectory(prefix="omascribe-build-info-") as folder:
        write_json(pathlib.Path(folder)/"network-policy.json", dict(allow_model_downloads=False, allow_online_recommendations=False))
        try:
            result = subprocess.run([str(cli), "build-info"], env=dict(os.environ, OMASCRIBE_DATA_DIR=folder),
                                    capture_output=True, text=True, encoding="utf-8", timeout=10)
            if result.returncode == 0:
                value=json.loads(result.stdout)
                if isinstance(value,dict) and value.get("schema")==1 and len(value.get("source_sha256",""))==64: build=value
        except (OSError, subprocess.TimeoutExpired, ValueError):
            pass
    changed=subprocess.check_output(['git','status','--porcelain','--untracked-files=all','--','crates','tests','Cargo.toml','Cargo.lock','model-catalog.json','models.lock.json'],cwd=REPO,text=True).strip()
    return dict(cli_build=build, core_and_harness_source_sha256=source_digest(), inference_source_tree_dirty=bool(changed), platform=platform.platform(), machine=platform.machine(), python=platform.python_version(),
        cli_sha256=sha256(cli), model_catalog_sha256=sha256(REPO / "model-catalog.json"),
        models_lock_sha256=sha256(REPO / "models.lock.json"),
        suite_revision=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
        # Avoid environment dumps: they can contain tokens or unrelated private paths.
        corpus="public or fictional only", network_during_evaluation="disabled optional services; inference uses local processes")
