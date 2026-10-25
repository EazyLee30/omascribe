"""Shared isolation and failure capture for real local inference tests (stdlib only)."""
import hashlib, json, os, pathlib, platform, shutil, signal, subprocess, time

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

def isolated_env(root):
    root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, OMASCRIBE_DATA_DIR=str(root),
        OMASCRIBE_MODELS_DIR=os.environ.get("OMASCRIBE_MODELS_DIR", str(pathlib.Path.home() / ".local/share/omascribe/models")),
        OMASCRIBE_RUNTIME_DIR=os.environ.get("OMASCRIBE_RUNTIME_DIR", str(REPO / "target/debug")))
    if os.environ.get("OMASCRIBE_TEST_MODEL_STORE"):
        def reuse(source, target):
            try: return os.link(source, target)
            except OSError: return shutil.copy2(source, target)
        shutil.copytree(env["OMASCRIBE_TEST_MODEL_STORE"], root / "model-store", copy_function=reuse, dirs_exist_ok=True)
    # These switches only govern optional external services. Core inference remains loopback.
    write_json(root / "network-policy.json", dict(allow_model_downloads=False, allow_online_recommendations=False))
    return env

def command(argv, env, directory, timeout=650):
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    options = {"start_new_session": True} if os.name != "nt" else {}
    with (directory / "stdout.txt").open("w", encoding="utf-8") as out, (directory / "stderr.txt").open("w", encoding="utf-8") as err:
        process = subprocess.Popen([str(a) for a in argv], env=env, stdout=out, stderr=err, **options)
        failure = None
        try: code = process.wait(timeout=timeout)
        except BaseException as error:
            if os.name == "nt": subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
            else:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            process.wait()
            code, failure = process.returncode, type(error).__name__
    write_json(directory / "process.json", dict(command=[str(a) for a in argv], code=code,
        seconds=round(time.monotonic()-started,3), failure=failure))
    if failure or code: raise RuntimeError(f"{failure or 'Exit ' + str(code)}; see {directory}")
    return json.loads((directory / "stdout.txt").read_text(encoding="utf-8"))

def source_digest():
    digest = hashlib.sha256()
    for folder, pattern in [("crates", "*.rs"), ("tests", "*.py"), ("tests/fixtures", "*.json")]:
        for path in sorted((REPO / folder).rglob(pattern)):
            digest.update(str(path.relative_to(REPO)).encode()); digest.update(b"\0"); digest.update(path.read_bytes())
    return digest.hexdigest()

def environment(cli):
    return dict(core_and_harness_source_sha256=source_digest(), platform=platform.platform(), machine=platform.machine(), python=platform.python_version(),
        cli_sha256=sha256(cli), model_catalog_sha256=sha256(REPO / "model-catalog.json"),
        suite_revision=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
        # Avoid environment dumps: they can contain tokens or unrelated private paths.
        corpus="public or fictional only", network_during_evaluation="disabled optional services; inference uses local processes")
