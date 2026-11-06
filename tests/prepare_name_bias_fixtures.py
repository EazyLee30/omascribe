#!/usr/bin/env python3
"""Prepare only the selected, pinned public FLEURS recordings; never run ASR.

The selection and references are frozen separately before recognition. Download
is explicit; evaluate_name_bias.py is offline and cannot fetch missing assets.
"""
import argparse
import csv
import json
import pathlib
import re
import subprocess
import tarfile
import tempfile
import unicodedata
import wave

from harness_support import read_json, sha256, write_json
from pinned_download import download
from prepare_asr_fixtures import ARCHIVES, BASE, REVISION

LANGUAGE = "en_us"
SAFE_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,99}")


def tokens(text):
    words = re.findall(r"[^\W_]+(?:'[^\W_]+)*", unicodedata.normalize("NFKC", text).casefold())
    # Entity identity includes the English possessive (China's still mentions
    # China). Do not strip a bare final s or alter negation/contractions. WER
    # uses evaluate_asr's separate, unchanged reference normalization.
    return [word[:-2] if word.endswith("'s") and len(word) > 2 else word for word in words]


def occurs(label, text):
    needle, haystack = tokens(label), tokens(text)
    expected = "".join(needle)
    if not expected:
        return False
    # Whitespace/initialism punctuation may change, but a match must consume
    # complete tokens. York must not match Yorkshire or a suffix of NewYork.
    for start in range(len(haystack)):
        candidate = ""
        for word in haystack[start:start + 16]:
            candidate += word
            if candidate == expected:
                return True
            if len(candidate) >= len(expected):
                break
    return False


def selection_rows(selection, metadata, require_count=100):
    if selection.get("schema") != 1 or selection.get("dataset") != "google/fleurs" or selection.get("revision") != REVISION:
        raise ValueError("Expected the pinned public FLEURS selection")
    if selection.get("license") != "CC-BY-4.0" or sha256(metadata) != selection.get("metadata_sha256"):
        raise ValueError("FLEURS metadata identity changed")
    cases = selection.get("cases", [])
    if not cases or require_count is not None and len(cases) != require_count:
        raise ValueError("Calibration requires 100 fixed positive/negative audio clusters")
    with metadata.open(encoding="utf-8", newline="") as stream:
        rows = {row[1]: row for row in csv.reader(stream, delimiter="\t")}
    ids, clusters, members = set(), set(), set()
    for case in cases:
        name, member = case.get("name", ""), case.get("source_filename", "")
        if not SAFE_ID.fullmatch(name) or name in ids:
            raise ValueError("Duplicate or unsafe calibration case ID")
        if not re.fullmatch(r"[0-9]+\.wav", member) or member in members:
            raise ValueError("Duplicate or unsafe public recording filename")
        row = rows.get(member)
        if not row or str(case.get("sentence_id")) != row[0] or row[0] in clusters:
            raise ValueError("Calibration needs distinct source sentence clusters")
        if case.get("raw_reference") != row[2] or case.get("reference") != row[3]:
            raise ValueError("Reference differs from pinned source transcript")
        positive, negative = case.get("positive", {}), case.get("negative", {})
        name_label, wrong_label = positive.get("expected_name", ""), negative.get("forbidden_name", "")
        if not occurs(name_label, row[2]) or not wrong_label or occurs(wrong_label, row[2]) or occurs(wrong_label, row[3]):
            raise ValueError("Positive/negative name annotation is not bound to its reference")
        for record, label in [(positive, name_label), (negative, wrong_label)]:
            if not isinstance(record.get("note"), str) or record["note"] != f"**{label}**":
                raise ValueError("Use exactly the annotated name as the public document note")
            if not 1 <= len(label) <= 80 or not all(c.isalnum() or c == ' ' for c in label):
                raise ValueError("Annotated name must have a complete decoder-compatible spelling")
        ids.add(name); clusters.add(row[0]); members.add(member)
    return cases


def prepare(selection_path, metadata, output, archive=None, smoke=None):
    selection = read_json(selection_path)
    cases = selection_rows(selection, metadata)
    if smoke is not None:
        if not 1 <= smoke <= len(cases):
            raise ValueError("Smoke count outside the frozen selection")
        cases = cases[:smoke]
    if output.exists():
        raise ValueError("Use a fresh fixture directory; existing evidence is never replaced")
    output.mkdir(parents=True)
    size, digest = ARCHIVES[LANGUAGE]
    source = f"{BASE}/{LANGUAGE}/audio/test.tar.gz"
    manifest = dict(schema=1, dataset="google/fleurs", revision=REVISION, license="CC-BY-4.0",
                    source=source, source_archive_sha256=digest,
                    selection_sha256=sha256(selection_path), metadata_sha256=sha256(metadata),
                    distinct_audio_clusters=len(cases), positive_cases=len(cases), negative_cases=len(cases),
                    full_calibration=len(cases) == 100, audio=[], status="preparing",
                    limitations=["English read speech, not natural Mandarin or code-switching.",
                                 "Positive/negative contexts reuse the same recordings; bootstrap by sentence cluster.",
                                 "Source transcripts are public reference labels; entity annotations require independent human audit.",
                                 "Unheard-name controls do not establish coverage of all near-homophone errors."])
    write_json(output / "manifest.json", manifest)
    owned_archive = archive is None
    archive = output / "en_us-test.tar.gz" if owned_archive else archive.resolve()
    try:
        if owned_archive:
            download(source, archive, size, digest)
        if archive.is_symlink() or archive.stat().st_size != size or sha256(archive) != digest:
            raise ValueError("Public audio archive differs from pinned bytes")
        selected = {case["source_filename"]: case for case in cases}
        with tempfile.TemporaryDirectory(prefix="selected-audio-", dir=output) as temporary:
            with tarfile.open(archive, "r:gz") as tar:
                for member in tar:
                    basename = pathlib.PurePosixPath(member.name).name
                    case = selected.get(basename)
                    if case is None:
                        continue
                    if not member.isfile() or not 0 < member.size <= 10_000_000:
                        raise ValueError("Unexpected selected audio archive member")
                    reader = tar.extractfile(member)
                    if reader is None:
                        raise ValueError("Missing selected audio data")
                    # Do not extract paths or links from the archive. Only a
                    # fixed basename is materialized in this owned directory.
                    original = pathlib.Path(temporary) / basename
                    with original.open("xb") as stream:
                        data = reader.read(member.size + 1)
                        if len(data) != member.size:
                            raise ValueError("Unexpected selected audio length")
                        stream.write(data)
                    converted = output / (case["name"] + ".wav")
                    part = converted.with_suffix(".part.wav")
                    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-protocol_whitelist", "file,pipe",
                                    "-i", str(original), "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(part)],
                                   check=True, timeout=60, capture_output=True)
                    with wave.open(str(part), "rb") as wav:
                        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getcomptype()) != (1, 2, 16000, "NONE"):
                            raise ValueError("Prepared audio format changed")
                        count = wav.getnframes()
                        if not 0 < count <= 16000 * 120:
                            raise ValueError("Prepared audio duration is outside the calibration budget")
                    part.replace(converted)
                    manifest["audio"].append(dict(id=case["name"], file=converted.name, sha256=sha256(converted),
                                                  reference=case["reference"], samples=count, sample_rate=16000,
                                                  source_filename=basename, source_member=member.name,
                                                  source_audio_sha256=sha256(original)))
                    original.unlink()
                    del selected[basename]
                    write_json(output / "manifest.json", manifest)
                    if not selected:
                        break
        if selected:
            raise ValueError("Missing selected recordings in the pinned archive")
        manifest["audio"].sort(key=lambda item: next(i for i, c in enumerate(cases) if c["name"] == item["id"]))
        manifest["status"] = "complete"
        write_json(output / "manifest.json", manifest)
    except BaseException as error:
        manifest.update(status="failed", error=f"{type(error).__name__}: {error}")
        write_json(output / "manifest.json", manifest)
        raise
    finally:
        # Only the archive created by this invocation may be removed. A supplied
        # cached archive belongs to its caller and is never deleted here.
        if owned_archive and archive.is_file() and not archive.is_symlink():
            archive.unlink()
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", type=pathlib.Path, required=True)
    parser.add_argument("--metadata", type=pathlib.Path, required=True)
    parser.add_argument("--directory", type=pathlib.Path, required=True)
    parser.add_argument("--archive", type=pathlib.Path, help="Reuse a verified archive without modifying it")
    parser.add_argument("--smoke", type=int, help="Prepare only the first N cases; does not close the 100/100 gate")
    args = parser.parse_args()
    manifest = prepare(args.selection.resolve(), args.metadata.resolve(), args.directory.resolve(), args.archive, args.smoke)
    print(json.dumps(dict(status=manifest["status"], clusters=manifest["distinct_audio_clusters"],
                          full_calibration=manifest["full_calibration"], manifest_sha256=sha256(args.directory / "manifest.json"))))


if __name__ == "__main__":
    main()
