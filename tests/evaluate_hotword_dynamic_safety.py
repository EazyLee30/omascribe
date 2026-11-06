#!/usr/bin/env python3
"""One bounded, offline Legacy/Stream dynamic-context gate; no accuracy/default promotion.

Reuses the protocols from benchmark_hotword_lifecycle.py,
evaluate_hotword_updates.py, the retained input-contract and fresh-context runs.
Freeze JSON uses source_revision, core_and_harness_source_sha256, worker_sha256.
The owner must bind that worker digest to its build proof before running this gate.
"""
import argparse
import array
from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import random
import shutil
import signal
import subprocess
import sys
import threading
import time
import uuid
import wave

from evaluate_name_bias import file_identity
from harness_support import REPO, read_json, sha256, source_digest, write_json
import process_resources
from process_resources import ProcessTreeSampler

TOTAL_SECONDS, ARM_SECONDS, RESERVE_SECONDS = 180, 30, 15
FIXTURE_MANIFEST_SHA = "658ce425103e9f87d33457618b6381830ab8c545a2d20518f444973130fddbc5"
MODEL_NAMES = {"encoder-epoch-99-avg-1.int8.onnx", "decoder-epoch-99-avg-1.onnx",
               "joiner-epoch-99-avg-1.int8.onnx", "tokens.txt", "bpe.model"}
INITIAL, BETA = "OMASCRIBE\nPROJECT ALPHA", "OMASCRIBE\nPROJECT BETA"
MODES = ("legacy", "stream")


def require(value, message):
    if not value:
        raise RuntimeError(message)


def require_process_sampler():
    require(process_resources._psutil is not None,
            "This gate requires psutil for process-tree exit evidence; install it in the harness environment before running")


class Budget:
    def __init__(self, seconds=TOTAL_SECONDS, reserve=RESERVE_SECONDS):
        self.started, self.seconds, self.reserve = time.monotonic(), seconds, reserve

    def remaining(self, limit=ARM_SECONDS):
        value = min(limit, self.started + self.seconds - self.reserve - time.monotonic())
        if value <= 0:
            raise TimeoutError("Global budget exhausted; no next worker/phase")
        return value


def environment(mode):
    require(mode in MODES, "Unknown mode")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OMASCRIBE_", "LLAMA_ARG_"))}
    env.update(OMASCRIBE_ASR_STREAM_HOTWORDS="1" if mode == "stream" else "0",
               OMASCRIBE_ASR_THREADS="1", OMP_NUM_THREADS="1", OPENBLAS_NUM_THREADS="1")
    return env


def group_absent(pid):
    try:
        os.killpg(pid, 0)
        return False
    except ProcessLookupError:
        return True
    except PermissionError:
        return False


class Native:
    """Same JSON worker protocol as the boundary helper, with one shared deadline."""
    def __init__(self, child, directory, budget, arm_deadline):
        self.child, self.directory, self.budget, self.deadline = child, directory, budget, arm_deadline
        self.inbox, self.events, self.reader_error = queue.Queue(), [], None
        self.output = (directory / "stdout.jsonl").open("w", encoding="utf-8")
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def remaining(self):
        return self.budget.remaining(self.deadline - time.monotonic())

    def read(self):
        try:
            for line in self.child.stdout:
                self.output.write(line)
                self.output.flush()
                try:
                    self.inbox.put(json.loads(line))
                except ValueError:
                    self.inbox.put({"event": "error", "message": "Invalid worker JSON"})
        except Exception as error:
            self.reader_error = type(error).__name__
        finally:
            self.inbox.put(None)

    def until(self, predicate):
        while True:
            event = self.inbox.get(timeout=self.remaining())
            require(event is not None, "Worker exited before required event")
            require(isinstance(event, dict), "Worker JSON is not an object")
            self.events.append(event)
            require(event.get("event") != "error", "Worker reported an error; see stdout")
            if predicate(event):
                return event

    def sleep(self, seconds):
        require(self.remaining() >= seconds, "Insufficient phase budget")
        time.sleep(seconds)

    def wait(self):
        require(self.child.wait(timeout=self.remaining()) == 0, "Worker exited unsuccessfully")


@contextmanager
def worker(argv, directory, mode, budget, processes):
    require(os.name == "posix", "This process-group gate currently supports macOS/Linux")
    require_process_sampler()
    timeout = budget.remaining()
    directory.mkdir(parents=True, exist_ok=False)
    record = dict(mode=mode, directory=str(directory), launched=False, exit_confirmed=False,
                  code=None, killed=False, errors=[])
    processes.append(record)
    child = sampler = native = None
    started = time.monotonic()
    with (directory / "stderr.txt").open("w", encoding="utf-8") as err:
        try:
            child = subprocess.Popen([str(x) for x in argv], env=environment(mode), stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=err, text=True, encoding="utf-8", bufsize=1, start_new_session=True)
            record.update(launched=True, pid=child.pid, pgid=os.getpgid(child.pid))
            require(record["pgid"] == child.pid, "Worker did not get an isolated process group")
            sampler = ProcessTreeSampler(child.pid, record_samples=True).start()
            native = Native(child, directory, budget, started + timeout)
            yield native
            native.wait()
        finally:
            # No asynchronous timer: always reap the owned group and finish exit evidence.
            if child is not None:
                if not group_absent(child.pid):
                    record["killed"] = True
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                try:
                    record["code"] = child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    record["errors"].append("Owned worker could not be reaped")
                if native is not None:
                    native.reader.join(timeout=3)
                    record["reader_stopped"] = not native.reader.is_alive()
                    record["reader_error"] = native.reader_error
                    if record["reader_stopped"]:
                        native.output.close()
                    if record["reader_stopped"] and child.stdout is not None:
                        child.stdout.close()
                resources = sampler.finish() if sampler is not None else {}
                write_json(directory / "resources.json", resources)
                record["group_absent"] = group_absent(child.pid)
                record["exit_confirmed"] = (type(record["code"]) is int and record["group_absent"]
                    and resources.get("processes_exit_confirmed") is True
                    and record.get("reader_stopped") is True and record.get("reader_error") is None)
            else:
                record["exit_confirmed"] = True  # Popen failed before a worker existed.
            record["seconds"] = round(time.monotonic() - started, 3)
            write_json(directory / "process.json", record)
        require(record["exit_confirmed"] and record["code"] == 0 and not record["killed"],
                "Worker did not exit normally with complete exit evidence")


def check_config(config, mode, count, loads, updates, reason=None):
    require(config.get("hotword_update_mode") == ("recognizer" if mode == "legacy" else "stream_experiment"), "Wrong mode")
    require(config.get("threads") == 1 and config.get("hotword_count") == count, "Wrong thread/context count")
    require(config.get("decoder") == ("modified_beam_search" if count else "greedy_search"), "Wrong search algorithm")
    require(config.get("default_hotword_buffer_present") is (bool(count) and mode == "legacy"), "Stale/wrong default context buffer")
    require(config.get("model_load_attempts") == loads and config.get("stream_updates") == updates, "Unexpected native context lifecycle")
    status = config.get("hotword_input") or {}
    if reason:
        require(status.get("status") == "rejected" and status.get("previous_context_retained") is True
            and (status.get("error") or {}).get("code") == reason and bool(config.get("fallback")), "Rejected update did not retain context")
    else:
        require(status.get("status") == "accepted" and status.get("error") is None
            and status.get("previous_context_retained") is False and config.get("fallback") is None, "Accepted context retained an error/fallback")


class Capture:
    def __init__(self, native, audio):
        self.native, self.audio, self.samples = native, audio, 0
        self.digest, self.phases = hashlib.sha256(), []

    def words(self, value):
        temporary = self.audio / "hotwords.next"
        temporary.write_text(value, encoding="utf-8")
        temporary.replace(self.audio / "hotwords.txt")

    def pcm(self, payload):
        require(len(payload) % 2 == 0, "Odd PCM byte count")
        self.digest.update(payload)
        offset = 0
        while offset < len(payload):
            index, position = divmod(self.samples, 16000 * 30)
            count = min((16000 * 30 - position) * 2, len(payload) - offset)
            with (self.audio / f"{index:06}.pcm").open("ab") as out:
                out.write(payload[offset:offset + count])
            offset += count
            self.samples += count // 2

    def config(self):
        for _ in range(20):  # Existing boundary writer is not atomic; no inference retry.
            try:
                return read_json(self.audio / "asr-config.json") or {}
            except (OSError, ValueError):
                self.native.sleep(.01)
        raise RuntimeError("Missing complete decoder diagnostics")

    def progress(self, milliseconds):
        return self.native.until(lambda e: e.get("event") == "progress" and e["processed_ms"] >= milliseconds)

    def boundary(self):
        end = self.samples * 1000 // 16000
        return self.native.until(lambda e: e.get("event") == "boundary" and e.get("cause") == "pause" and e["processed_ms"] == end)

    def phase(self, name, payload, words):
        before, start = len(self.native.events), self.samples * 1000 // 16000
        self.words(words)
        (self.audio / "capture.paused").unlink()
        self.pcm(payload)
        (self.audio / "capture.paused").touch()
        self.boundary()
        end = self.samples * 1000 // 16000
        segments = [e for e in self.native.events[before:] if e.get("event") == "segment"]
        row = dict(case=name, segments=segments, config=self.config())
        self.phases.append(row)
        write_json(self.audio.parent / "phases.json", self.phases)
        require(all(start <= s["start_ms"] <= s["end_ms"] <= end for s in segments), "Segment escapes phase")
        require(not segments if name in ("silence", "quiet-noise") else bool(segments), "Missing speech or silence hallucination")
        return row

    def finish(self):
        (self.audio / "capture.done").touch()
        done = self.native.until(lambda e: e.get("event") == "done")
        require(done.get("processed_ms") == self.samples * 1000 // 16000, "Final PCM tail was not consumed")
        self.native.wait()
        segments = [e for e in self.native.events if e.get("event") == "segment"]
        require([s["seq"] for s in segments] == list(range(1, len(segments) + 1)), "Missing/duplicate segment sequence")
        actual, size = hashlib.sha256(), 0
        for index, path in enumerate(sorted(self.audio.glob("*.pcm"))):
            require(path.name == f"{index:06}.pcm" and not path.is_symlink(), "Unexpected PCM archive member")
            payload = path.read_bytes()
            actual.update(payload)
            size += len(payload)
        require(size == self.samples * 2 and actual.hexdigest() == self.digest.hexdigest(), "Archived PCM changed")
        return dict(samples=self.samples, pcm_sha256=actual.hexdigest(), final_processed_ms=done["processed_ms"], segments=segments)


@contextmanager
def capture(assets, directory, mode, budget, processes, initial=INITIAL, paused=True):
    audio = directory / "audio"
    audio.mkdir(parents=True)
    (audio / "hotwords.txt").write_text(initial, encoding="utf-8")
    if paused:
        (audio / "capture.paused").touch()
    argv = [assets.worker, assets.model, audio, "0", "1", "", audio / "hotwords.txt", ""]
    with worker(argv, directory / "process", mode, budget, processes) as native:
        native.until(lambda e: e.get("event") == "ready")
        session = Capture(native, audio)
        if paused:
            session.boundary()
        yield session


def lifecycle(assets, directory, mode, budget, processes):
    argv = [assets.worker, "--benchmark-hotwords", assets.model, directory / "decoder", ""]
    with worker(argv, directory / "process", mode, budget, processes) as native:
        result = native.until(lambda e: e.get("kind") == "hotword-lifecycle-only")
    loads, updates = (13, 0) if mode == "legacy" else (1, 12)
    require(len(result.get("updates_ms", [])) == 12 and result.get("unchanged_noop") is True
        and result.get("includes_audio") is False, "Incomplete fixed lifecycle protocol")
    check_config(result["after_updates"], mode, 2, loads, updates)
    check_config(result["after_disable_enable"], mode, 2, loads + 2, updates)
    return result


def boundary(assets, directory, mode, budget, processes):
    contexts = [BETA, "OMASCRIBE", "", "OMASCRIBE\nPROJECT GAMMA", BETA, BETA]
    phases = list(assets.audio)
    phases.append(("silence", bytes(16000 * 2 * 10)))
    rng = random.Random(1729)
    noise = array.array("h", (int(32 * math.sin(2 * math.pi * 120 * i / 16000) + rng.gauss(0, 6)) for i in range(16000 * 10)))
    if sys.byteorder != "little":
        noise.byteswap()
    phases.append(("quiet-noise", noise.tobytes()))
    rows = []
    with capture(assets, directory, mode, budget, processes) as session:
        for i, ((name, pcm), words) in enumerate(zip(phases, contexts)):
            row = session.phase(name, pcm, words)
            loads = [2, 3, 4, 5, 6, 6][i] if mode == "legacy" else [1, 1, 2, 3, 3, 3][i]
            updates = 0 if mode == "legacy" else [1, 2, 2, 2, 3, 3][i]
            check_config(row["config"], mode, [2, 1, 0, 2, 2, 2][i], loads, updates)
            rows.append(row)
        final = session.finish()
    require(sum(len(row["segments"]) for row in rows) == len(final["segments"]), "Duplicate final passage")
    return dict(phases=rows, final=final)


def rejected_updates(assets, directory, mode, budget, processes):
    words = ["PROJECT :99999", "界" * 5334, INITIAL, BETA, BETA]
    rows = []
    with capture(assets, directory, mode, budget, processes) as session:
        for i, text in enumerate(words):
            row = session.phase("silence", bytes(16000 * 2), text)
            check_config(row["config"], mode, 2, 1 + (mode == "legacy" and i >= 3),
                         int(mode == "stream" and i >= 3), ["invalid_words", "input_too_large", None, None, None][i])
            require(row["config"]["hotword_input"].get("stage") == "refresh", "Wrong rejection/recovery stage")
            rows.append(row)
        final = session.finish()
    return dict(phases=rows, final=final)


def fresh_context(assets, directory, mode, budget, processes):
    snapshots = []
    # Existing 800+2400/3200-sample timing protocol, starting empty to cover the
    # common case of writing the first proper name after recording starts.
    with capture(assets, directory, mode, budget, processes, initial="", paused=False) as s:
        def observe(stage):
            snapshots.append(dict(stage=stage, config=s.config()))
            write_json(directory / "snapshots.json", snapshots)
        observe("ready_empty")
        s.words(INITIAL)
        s.native.sleep(.15)
        s.pcm(bytes(800 * 2))
        s.native.sleep(.15)
        observe("incomplete_block_empty")
        s.pcm(bytes(2400 * 2))
        s.progress(200)
        observe("first_block_two")
        s.words(INITIAL + "\nPROJECT BETA")
        s.native.sleep(.4)
        s.pcm(bytes(3200 * 2))
        s.progress(400)
        observe("active_still_two")
        require(not any(e.get("event") == "boundary" for e in s.native.events), "Unexpected early endpoint")
        (s.audio / "capture.paused").touch()
        s.boundary()
        observe("pause_three")
        s.words("")
        s.native.sleep(.4)
        observe("paused_idle_three")
        (s.audio / "capture.paused").unlink()
        s.pcm(bytes(3200 * 2))
        s.progress(600)
        observe("resume_first_block_empty")
        s.words(INITIAL)
        s.native.sleep(.4)
        s.pcm(bytes(3200 * 2))
        s.progress(800)
        observe("resumed_active_empty")
        final = s.finish()
        observe("done_empty")
    require(not final["segments"] and final["samples"] == 12800, "Timing control changed PCM or hallucinated speech")
    for i, row in enumerate(snapshots):
        count = [0, 0, 2, 2, 3, 3, 0, 0, 0][i]
        loads = [1, 1, 2, 2, 3, 3, 4, 4, 4][i] if mode == "legacy" else [1, 1, 2, 2, 2, 2, 3, 3, 3][i]
        updates = int(mode == "stream" and i >= 4)
        check_config(row["config"], mode, count, loads, updates)
    return dict(snapshots=snapshots, final=final)


def source_identity(args):
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True, timeout=10).strip()
    return dict(source_revision=revision, core_and_harness_source_sha256=source_digest(), worker_sha256=sha256(args.worker))


class Assets:
    def __init__(self, args, root):
        self.args, self.root, self.owner = args, root, uuid.uuid4().hex
        self.worker, self.model = root / "runtime" / args.worker.name, root / "model"
        self.originals, self.copied, self.audio = {}, {}, []

    def inventory(self):
        paths = [self.args.worker, REPO / "model-catalog.json", REPO / "models.lock.json", self.args.freeze,
                 self.args.fixtures / "manifest.json"]
        catalog = read_json(REPO / "model-catalog.json")
        spec = next(x for x in catalog["models"] if x["id"] == "zipformer-small")
        require({x["name"] for x in spec["files"]} == MODEL_NAMES, "Unexpected pinned model members")
        for item in spec["files"]:
            path = self.args.model / item["name"]
            require(path.is_file() and not path.is_symlink(), "Model member missing/symlinked")
            require(path.stat().st_size == item["bytes"] and sha256(path) == item["sha256"], "Pinned model differs")
            paths.append(path)
        manifest = self.args.fixtures / "manifest.json"
        require(sha256(manifest) == FIXTURE_MANIFEST_SHA, "Use the existing fixed four-WAV manifest")
        for item in read_json(manifest)["cases"]:
            require(Path(item["file"]).name == item["file"], "Unsafe fixture path")
            path = self.args.fixtures / item["file"]
            require(sha256(path) == item["sha256"], "Public audio hash differs")
            paths.append(path)
        require(all(p.is_file() and not p.is_symlink() for p in paths), "Use regular original assets")
        return {str(path): file_identity(path) for path in paths}

    def prepare(self):
        self.originals = self.inventory()
        self.root.mkdir()
        write_json(self.root / "owner.json", {"owner": self.owner})
        copies = {self.worker: self.args.worker, **{self.model / name: self.args.model / name for name in MODEL_NAMES}}
        for target, source in copies.items():
            target.parent.mkdir(parents=True, exist_ok=True)
            self.copied[str(target)] = None
            shutil.copy2(source, target)  # Never hardlink installed models.
            a, b = source.stat(), target.stat()
            require((a.st_dev, a.st_ino) != (b.st_dev, b.st_ino), "Copy aliases original inode")
            require(sha256(target) == self.originals[str(source)]["sha256"], "Copied bytes differ")
            self.copied[str(target)] = file_identity(target)
        for item in read_json(self.args.fixtures / "manifest.json")["cases"]:
            with wave.open(str(self.args.fixtures / item["file"]), "rb") as wav:
                require((wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) == (1, 2, 16000)
                    and 0 < wav.getnframes() <= 16000 * 60, "Unexpected public WAV format/duration")
                payload = wav.readframes(wav.getnframes())
                require(len(payload) == wav.getnframes() * 2, "Truncated public WAV")
                self.audio.append((item["file"], payload))

    def cleanup(self, exited):
        result = dict(released=False, files_before_cleanup={})
        if not self.root.exists():
            return dict(released=True, files_before_cleanup={}, note="No clone was created")
        require(exited, "Unconfirmed owned process exit; retain asset copies")
        require(not self.root.is_symlink() and read_json(self.root / "owner.json") == {"owner": self.owner}, "Unknown asset owner")
        entries = list(self.root.rglob("*"))
        require(not any(p.is_symlink() for p in entries), "Unexpected owned symlink")
        require({p for p in entries if p.is_dir()} <= {self.root / "runtime", self.model}, "Unexpected owned directory")
        allowed = {Path(p) for p in self.copied} | {self.root / "owner.json"}
        require({p for p in entries if p.is_file()} <= allowed, "Unexpected owned file")
        for path, before in self.copied.items():
            current = file_identity(Path(path))
            require(before is not None and current == before, "Owned asset changed or copy incomplete; retain")
            result["files_before_cleanup"][path] = current
        for path in entries:
            if path.is_file():
                path.unlink()
        for path in sorted((p for p in entries if p.is_dir()), key=lambda p: len(p.parts), reverse=True):
            path.rmdir()
        self.root.rmdir()
        result["released"] = True
        return result


def run_protocols(assets, root, budget, report):
    for name, protocol in (("lifecycle", lifecycle), ("boundary", boundary),
                           ("rejected_updates", rejected_updates), ("fresh_context", fresh_context)):
        pair = {}
        report["protocols"][name] = pair
        for mode in MODES:
            budget.remaining()
            directory = root / name / mode
            pair[mode] = protocol(assets, directory, mode, budget, report["processes"])
            write_json(directory / "result.json", pair[mode])
        if name != "lifecycle":
            require(pair["legacy"]["final"] == pair["stream"]["final"], f"{name}: PCM/text/tail differs between modes")
        if name == "boundary":
            require([r["segments"] for r in pair["legacy"]["phases"]] == [r["segments"] for r in pair["stream"]["phases"]], "Boundary phase text/timing differs")


def run(args):
    budget, report = Budget(), dict(schema=1, kind="paired-dynamic-hotword-safety", passed=False, protocols={},
        processes=[], errors=[], default_promotion=False, release_admission=False,
        total_seconds=TOTAL_SECONDS, per_worker_seconds=ARM_SECONDS, reserve_seconds=RESERVE_SECONDS,
        limitations=["Existing public/synthetic protocols, not independent accuracy, real microphone or minutes admission.",
                     "Mac/Linux process-group harness; Windows hardware validation remains separate.",
                     "Worker bytes are caller-pinned; build provenance must be reviewed by the owner."])
    root = args.output.with_suffix(".artifacts")
    require(not args.output.exists() and not root.exists(), "Use a fresh output path; no retry/overwrite")
    root.mkdir(parents=True)
    assets, before = Assets(args, root / "owned-assets"), None
    write_json(args.output, report)
    try:
        require_process_sampler()  # Refuse before hashing/copying assets or launching any worker.
        before = source_identity(args)
        freeze = read_json(args.freeze)
        require(all(before[key] == freeze.get(key) for key in before), "Source/worker freeze differs")
        require(os.name == "posix", "This gate requires POSIX process groups")
        assets.prepare()
        report["identity_before"] = dict(source=before, originals=assets.originals, clones=assets.copied)
        run_protocols(assets, root, budget, report)
    except BaseException as error:
        report["errors"].append(f"{type(error).__name__}: {error}")
    finally:
        report["identities_unchanged"] = False
        if before is not None:
            try:
                after = dict(source=source_identity(args), originals=assets.inventory())
                report["identity_after"] = after
                require(after["source"] == before and after["originals"] == assets.originals, "Original/source identity changed")
                report["identities_unchanged"] = True
            except BaseException as error:
                report["errors"].append(f"Identity recheck: {type(error).__name__}: {error}")
        exited = all(p.get("exit_confirmed") is True for p in report["processes"])
        try:
            report["cleanup"] = assets.cleanup(exited)
        except BaseException as error:
            report["cleanup"] = dict(released=False, error=f"{type(error).__name__}: {error}")
            report["errors"].append("Owned copies retained; see cleanup")
        report["wall_seconds"] = round(time.monotonic() - budget.started, 3)
        report["budget_met"] = report["wall_seconds"] <= TOTAL_SECONDS
        report["passed"] = (not report["errors"] and report["budget_met"] and report["identities_unchanged"]
            and len(report["processes"]) == 8 and exited and report["cleanup"]["released"])
        write_json(args.output, report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("worker", "model", "fixtures", "freeze", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for name in vars(args):
        setattr(args, name, getattr(args, name).absolute())
    report = run(args)
    print(json.dumps({key: report[key] for key in ("passed", "errors", "wall_seconds", "budget_met")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
