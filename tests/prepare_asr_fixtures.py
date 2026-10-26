#!/usr/bin/env python3
"""Explicit network preparation only; evaluation itself never downloads anything.
FLEURS, Google, CC BY 4.0: https://huggingface.co/datasets/google/fleurs
"""
import argparse, csv, hashlib, io, json, pathlib, subprocess, tarfile, urllib.request, wave

REVISION = "70bb2e84b976b7e960aa89f1c648e09c59f894dd"
BASE = f"https://huggingface.co/datasets/google/fleurs/resolve/{REVISION}/data"
ARCHIVES = {
    "cmn_hans_cn": (525346466, "09d19ad18f5d7e91076880807e866cd16abd924c7052b55f71cdae91714fc166"),
    "en_us": (289851356, "d9c2e37b41aacd41bc283554a0a82b5476b36887049774ecb2819dcaaa55a356"),
}

def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def add_mixed(root, manifest):
    """Splice verified public clips; this is not natural code-switching or speaker gold."""
    if manifest.get("dataset") != "google/fleurs" or manifest.get("revision") != REVISION or manifest.get("license") != "CC-BY-4.0":
        raise ValueError("Mixed fixtures require the pinned public FLEURS manifest")
    base = [c for c in manifest["cases"] if c["language"] != "mixed"]
    zh = sorted((c for c in base if c["language"] == "cmn_hans_cn"), key=lambda c:c["name"])
    en = sorted((c for c in base if c["language"] == "en_us"), key=lambda c:c["name"])
    if min(len(zh), len(en)) < 3: raise ValueError("Need three clips of each language")
    cases = []
    for index, selected in enumerate([(zh[0], en[0], zh[1]), (en[1], zh[2], en[2])], 1):
        name = f"fleurs_mixed_zh_en_{index:02}"
        output = root / (name + ".wav")
        part = output.with_suffix(".wav.part")
        pieces, cursor = [], 0
        try:
            with wave.open(str(part), "wb") as out:
                out.setparams((1,2,16000,0,"NONE","not compressed"))
                for number, case in enumerate(selected):
                    if pathlib.Path(case["file"]).name != case["file"] or "\\" in case["file"]:
                        raise ValueError("Fixture path must be a basename")
                    path = root / case["file"]
                    if digest(path) != case["sha256"]: raise ValueError("Mixed source checksum mismatch")
                    with wave.open(str(path), "rb") as audio:
                        if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype()) != (1,2,16000,"NONE"):
                            raise ValueError("Expected 16 kHz mono PCM fixture")
                        count = audio.getnframes()
                        if not 0 < count <= 16000*300: raise ValueError("Unexpected fixture duration")
                        pieces.append(dict(case=case["name"], file=case["file"], sha256=case["sha256"], start_sample=cursor, end_sample=cursor+count))
                        while block := audio.readframes(16000): out.writeframes(block)
                        cursor += count
                    if number + 1 < len(selected):
                        out.writeframes(b"\0" * (4000*2)); cursor += 4000
            part.replace(output)
        finally:
            part.unlink(missing_ok=True)
        cases.append(dict(name=name, language="mixed", file=output.name,
                          reference=" ".join(c["reference"] for c in selected), sha256=digest(output),
                          kind="constructed-language-switches", gap_ms=250, pieces=pieces,
                          limitation="Spliced held-out utterances, not natural code-switching or diarization labels"))
    manifest = dict(manifest, schema=2, cases=sorted(base+cases,key=lambda c:c["name"]))
    (root / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    return manifest

def prepare(root, with_mixed=False):
    root.mkdir(parents=True, exist_ok=True)
    cases = []
    for lang, (size, sha) in ARCHIVES.items():
        archive = root / f"{lang}-test.tar.gz"
        if not archive.exists() or archive.stat().st_size != size or digest(archive) != sha:
            part = archive.with_suffix(".part")
            print(f"Downloading pinned public {lang} test audio ({size // 1000000} MB)", flush=True)
            with urllib.request.urlopen(f"{BASE}/{lang}/audio/test.tar.gz", timeout=90) as response, part.open("wb") as output:
                while data := response.read(1024 * 1024): output.write(data)
            if part.stat().st_size != size or digest(part) != sha:
                part.unlink(missing_ok=True)
                raise ValueError("Public archive checksum mismatch")
            part.replace(archive)
        metadata = urllib.request.urlopen(f"{BASE}/{lang}/test.tsv", timeout=30).read()
        (root / f"{lang}-test.tsv").write_bytes(metadata)
        # First four distinct sentence IDs in the official test ordering, no cherry-picking.
        rows, seen = {}, set()
        for row in csv.reader(io.StringIO(metadata.decode()), delimiter="\t"):
            if row[0] not in seen:
                rows[row[1]] = row
                seen.add(row[0])
            if len(rows) == 4: break
        with tarfile.open(archive, "r:gz") as source:
            for member in source:
                name = pathlib.PurePosixPath(member.name).name
                if name not in rows: continue
                row = rows.pop(name)
                audio = source.extractfile(member)
                if not member.isfile() or member.size > 10000000 or audio is None:
                    raise ValueError("Unexpected public audio member")
                original = root / f"{lang}-{name}"
                original.write_bytes(audio.read())
                converted = root / f"{lang}-{name[:-4]}-16k.wav"
                subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(original), "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(converted)], check=True)
                cases.append(dict(name=f"fleurs_{lang}_{row[0]}", language=lang, file=converted.name,
                    reference=row[3], original_text=row[2], gender=row[6],
                    sha256=digest(converted), source_audio_sha256=digest(original), source_member=member.name,
                    source=f"{BASE}/{lang}/audio/test.tar.gz", archive_sha256=sha))
                if not rows: break
        if rows: raise ValueError(f"Missing public clips: {list(rows)}")
    cases.sort(key=lambda c: c["name"])
    manifest = dict(schema=1, dataset="google/fleurs", revision=REVISION, license="CC-BY-4.0",
        attribution="FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech, Google (2022)",
        selection="First four distinct sentence IDs in each official test.tsv; one recording per sentence.",
        cases=cases)
    (root / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    if with_mixed: manifest = add_mixed(root, manifest)
    print(f"Prepared {len(manifest['cases'])} clips at {root}", flush=True)

if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--directory", type=pathlib.Path, required=True)
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--with-mixed", action="store_true", help="Also construct language-switching clips")
    mode.add_argument("--mixed-only", action="store_true", help="Offline: add mixed clips to an existing pinned corpus")
    args = p.parse_args()
    root = args.directory.resolve()
    if args.mixed_only:
        add_mixed(root, json.loads((root / "manifest.json").read_text(encoding="utf-8")))
    else:
        prepare(root, args.with_mixed)
