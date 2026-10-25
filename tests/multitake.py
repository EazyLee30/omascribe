#!/usr/bin/env python3
"""Real ASR regression using the public sherpa sample; never opens the user's workspace."""

import argparse, hashlib, json, os, pathlib, sqlite3, subprocess, tempfile, wave, shutil
from contextlib import closing

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--cli", type=pathlib.Path, default=pathlib.Path("target/debug/omascribe-cli")
)
parser.add_argument(
    "--output", type=pathlib.Path, default=pathlib.Path("/tmp/omascribe-multitake.json")
)
args = parser.parse_args()
cli = args.cli.resolve()
data = pathlib.Path.home() / ".local/share/omascribe"
with tempfile.TemporaryDirectory(prefix="omascribe-takes-") as directory:
    root = pathlib.Path(directory)
    env = dict(
        os.environ,
        OMASCRIBE_DATA_DIR=directory,
        OMASCRIBE_MODELS_DIR=os.environ.get(
            "OMASCRIBE_MODELS_DIR", str(data / "models")
        ),
        OMASCRIBE_RUNTIME_DIR=os.environ.get("OMASCRIBE_RUNTIME_DIR", str(cli.parent)),
    )

    if os.environ.get("OMASCRIBE_TEST_MODEL_STORE"):
        def reuse_model(source, target):
            try: return os.link(source, target)
            except OSError: return shutil.copy2(source, target)
        shutil.copytree(os.environ["OMASCRIBE_TEST_MODEL_STORE"], pathlib.Path(root) / "model-store", copy_function=reuse_model)

    def call(*arguments):
        return json.loads(
            subprocess.check_output([str(cli), *arguments], env=env, timeout=180)
        )

    sample = pathlib.Path(
        os.environ.get(
            "OMASCRIBE_TEST_WAV", str(data / "models/zipformer/test_wavs/0.wav")
        )
    )
    first = call("import", str(sample), "Public sample · two separate takes")
    mid = first["meeting"]["id"]
    assert first["meeting"]["status"] == "ready", first["runtime"]["error"]
    assert first["segments"]
    folder = root / "meetings" / mid
    first_audio = (folder / "files/recording-001.wav").read_bytes()
    first_pcm = b"".join(
        p.read_bytes() for p in sorted((folder / "audio").glob("*.pcm"))
    )
    # Simulate a saved human correction, then restart and append. It must survive unchanged.
    corrected = dict(first["segments"][0], text="人工修订 · public sample", revision=2)
    # A connection context commits/rolls back; closing also releases the Windows file handle.
    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
        db.execute(
            "update segments set json=? where id=?",
            (json.dumps(corrected), corrected["id"]),
        )
    call("note", mid, "OmaScribe public sample", "focus")
    second = call("append", mid, str(sample))
    assert second["meeting"]["status"] == "ready", second["runtime"]["error"]
    assert (
        second["meeting"]["id"] == mid
        and second["meeting"]["run"] == first["meeting"]["run"]
    )
    n = len(first["segments"])
    assert second["segments"][:n] == [corrected, *first["segments"][1:]]
    assert len(second["segments"]) > n
    assert second["segments"][n]["seq"] == first["segments"][-1]["seq"] + 1
    assert second["segments"][n]["start_ms"] == first["meeting"]["duration_ms"]
    assert second["meeting"]["duration_ms"] == first["meeting"]["duration_ms"] * 2
    assert (folder / "files/recording-001.wav").read_bytes() == first_audio
    pcm = b"".join(p.read_bytes() for p in sorted((folder / "audio").glob("*.pcm")))
    assert pcm == first_pcm * 2
    waves = []
    for number in [1, 2]:
        with wave.open(str(folder / f"files/recording-{number:03}.wav")) as wav:
            waves.append(wav.readframes(wav.getnframes()))
    assert b"".join(waves) == pcm
    files = call("files", mid)
    assert len(files["files"]) == 5
    assert "人工修订" in (folder / "files/transcript-001.txt").read_text(
        encoding="utf-8"
    )
    assert "人工修订" in (folder / "files/transcript.md").read_text(encoding="utf-8")
    assert (
        (folder / "files/transcript-002.txt")
        .read_text(encoding="utf-8")
        .startswith("[00:00:00]")
    )
    retranscribed = call("retranscribe", mid)
    boundary = first["meeting"]["duration_ms"]
    assert retranscribed["meeting"]["status"] == "ready"
    assert retranscribed["meeting"]["run"] == first["meeting"]["run"] + 1
    assert any(s["start_ms"] == boundary for s in retranscribed["segments"])
    assert not any(
        s["start_ms"] < boundary < s["end_ms"] for s in retranscribed["segments"]
    )
    assert (folder / "files/recording-001.wav").read_bytes() == first_audio
    assert (
        (folder / "files/transcript-002.txt")
        .read_text(encoding="utf-8")
        .startswith("[00:00:00]")
    )
    result = dict(
        passed=True,
        takes=2,
        samples=len(pcm) // 2,
        segments=len(second["segments"]),
        duration_ms=second["meeting"]["duration_ms"],
        first_take_sha256=hashlib.sha256(first_audio).hexdigest(),
        checks=[
            "Sample-exact append across app restart",
            "Stable original segments and human revision",
            "Monotonic segment IDs and meeting timestamps",
            "Separate playable WAV and text files",
            "Updated complete transcript mirror",
            "Full retranscription respects take boundaries without rewriting WAVs",
        ],
    )
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
