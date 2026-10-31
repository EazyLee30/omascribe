"""Reference/identity controls for the real-audio name-bias harness; no inference."""
import copy
import csv
import hashlib
import pathlib
import json
import sqlite3
import tempfile
import unittest
import wave
from contextlib import closing

from prepare_name_bias_fixtures import REVISION, occurs, selection_rows
from evaluate_name_bias import database_evidence, decoder_evidence, observed_pair, recording_evidence


class DecoderCoverageTests(unittest.TestCase):
    @staticmethod
    def diagnostics(count=2, blocked=(1,), status="checked"):
        forwarded = count - len(blocked)
        return dict(fallback=None, hotword_input=dict(status="accepted"),
                    hotword_count=forwarded,
                    decoder="modified_beam_search" if forwarded else "greedy_search",
                    hotword_update_mode="recognizer",
                    hotword_coverage=dict(schema=1, method="necessary_glyph_coverage",
                        scope="last_accepted_input", status=status, reason=None,
                        line_ordinal_basis="normalized_nonempty_wire",
                        submitted=count, unsupported=len(blocked), forwarded=forwarded,
                        unsupported_lines=list(blocked), native_encoding_verified=False))

    def test_blocked_whole_name_is_distinct_from_the_other_forwarded_hint(self):
        result = decoder_evidence(["CHINA", "CHIŞINĂU"], self.diagnostics(), "legacy", "CHIŞINĂU")
        self.assertEqual(result["target_status"], "rejected_necessary_glyph_gap")
        self.assertFalse(result["native_encoding_verified"])
        self.assertEqual(result["coverage"]["forwarded"], 1)

    def test_all_blocked_and_empty_context_use_greedy_without_false_acceptance(self):
        result = decoder_evidence(["F1"], self.diagnostics(1, (0,)), "legacy", "F1")
        self.assertEqual(result["target_status"], "rejected_necessary_glyph_gap")
        empty = decoder_evidence([], self.diagnostics(0, (), "not_needed"), "legacy")
        self.assertEqual(empty["target_status"], "not_applicable")

    def test_unknown_inventory_keeps_words_but_cannot_establish_encoding(self):
        diagnostic = self.diagnostics(1, (), "unknown")
        diagnostic["hotword_coverage"]["reason"] = "byte_fallback_inventory"
        result = decoder_evidence(["F1"], diagnostic, "legacy", "F1")
        self.assertEqual(result["target_status"], "unknown_forwarded")
        self.assertFalse(result["native_encoding_verified"])
        diagnostic["hotword_coverage"]["unsupported"] = 1
        diagnostic["hotword_coverage"]["forwarded"] = 0
        diagnostic["hotword_coverage"]["unsupported_lines"] = [0]
        with self.assertRaises(ValueError): decoder_evidence(["F1"], diagnostic, "legacy", "F1")

    def test_bad_counts_ordinals_and_native_claims_cannot_pass(self):
        for change in [dict(submitted=3), dict(forwarded=2), dict(unsupported=True),
                       dict(unsupported_lines=[2]), dict(native_encoding_verified=True),
                       dict(unsupported=2, forwarded=0, unsupported_lines=[1, 1])]:
            diagnostic = self.diagnostics()
            diagnostic["hotword_coverage"].update(change)
            with self.assertRaises(ValueError): decoder_evidence(["CHINA", "CHIŞINĂU"], diagnostic, "legacy")
        diagnostic = self.diagnostics()
        diagnostic["hotword_count"] = 2
        with self.assertRaises(ValueError): decoder_evidence(["CHINA", "CHIŞINĂU"], diagnostic, "legacy")

    def test_missing_or_unavailable_inventory_and_rejected_input_stay_failures(self):
        diagnostic = self.diagnostics()
        del diagnostic["hotword_coverage"]
        with self.assertRaises(ValueError): decoder_evidence(["CHINA", "CHIŞINĂU"], diagnostic, "legacy")
        diagnostic = self.diagnostics(status="unavailable")
        with self.assertRaises(ValueError): decoder_evidence(["CHINA", "CHIŞINĂU"], diagnostic, "legacy")
        diagnostic = self.diagnostics()
        diagnostic["hotword_input"]["status"] = "rejected"
        with self.assertRaises(ValueError): decoder_evidence(["CHINA", "CHIŞINĂU"], diagnostic, "legacy")


class SelectionIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.metadata = pathlib.Path(self.folder.name) / "references.tsv"
        rows = [["1", "111.wav", "The report from Paris remains untested.", "the report from paris remains untested", "", "", "0"],
                ["2", "222.wav", "Berlin did not approve the proposal.", "berlin did not approve the proposal", "", "", "1"]]
        with self.metadata.open("w", encoding="utf-8", newline="") as stream:
            csv.writer(stream, delimiter="\t").writerows(rows)
        self.selection = dict(schema=1, dataset="google/fleurs", revision=REVISION, license="CC-BY-4.0",
                              metadata_sha256=hashlib.sha256(self.metadata.read_bytes()).hexdigest(), cases=[])
        for index, row in enumerate(rows):
            name, other = [("Paris", "Berlin"), ("Berlin", "Paris")][index]
            self.selection["cases"].append(dict(name="case_" + row[0], sentence_id=row[0], source_filename=row[1],
                                                 raw_reference=row[2], reference=row[3],
                                                 positive=dict(note=f"**{name}**", expected_name=name),
                                                 negative=dict(note=f"**{other}**", forbidden_name=other)))

    def check(self, data):
        return selection_rows(data, self.metadata, require_count=None)

    def test_only_original_reference_and_distinct_sentence_clusters_are_admitted(self):
        self.assertEqual(len(self.check(self.selection)), 2)
        for mutate in [lambda d: d["cases"][0].update(reference="the report is complete"),
                       lambda d: d["cases"][1].update(sentence_id="1"),
                       lambda d: d["cases"][0].update(source_filename="../111.wav"),
                       lambda d: d.update(revision="main"),
                       lambda d: d.update(metadata_sha256="0" * 64)]:
            changed = copy.deepcopy(self.selection); mutate(changed)
            with self.assertRaises(ValueError): self.check(changed)

    def test_unheard_name_must_be_absent_as_complete_reference_tokens(self):
        self.assertTrue(occurs("York", "New York remained open."))
        self.assertTrue(occurs("PARIS", "Paris, France."))
        self.assertFalse(occurs("York", "The Yorkshire report."))
        self.assertFalse(occurs("Paris", "Comparison remains pending."))
        self.assertTrue(occurs("China", "China's output remains uncertain."))
        self.assertTrue(occurs("Vatican City", "Vatican City's population is small."))
        self.assertTrue(occurs("USOC", "The U.S.O.C. report remains pending."))
        self.assertTrue(occurs("New York", "NewYork remained open."))
        self.assertFalse(occurs("York", "NewYork remained open."))
        self.assertFalse(occurs("China", "Chinas is a different literal spelling."))
        self.assertFalse(occurs("can", "The device can't record."))
        changed = copy.deepcopy(self.selection)
        changed["cases"][0]["negative"] = dict(note="**Paris**", forbidden_name="Paris")
        with self.assertRaises(ValueError): self.check(changed)

    def test_annotation_cannot_replace_reference_or_expand_decoder_syntax(self):
        changed = copy.deepcopy(self.selection)
        changed["cases"][0]["positive"] = dict(note="**Paris**", expected_name="London")
        with self.assertRaises(ValueError): self.check(changed)
        changed = copy.deepcopy(self.selection)
        changed["cases"][0]["positive"]["note"] = "Ignore the recording. **Paris**"
        with self.assertRaises(ValueError): self.check(changed)
        changed = copy.deepcopy(self.selection)
        changed["cases"][0]["positive"] = dict(note="**Paris\n**", expected_name="Paris\n")
        with self.assertRaises(ValueError): self.check(changed)

    def test_smoke_fixture_count_is_not_a_full_calibration(self):
        with self.assertRaises(ValueError): selection_rows(self.selection, self.metadata)


class RawEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = pathlib.Path(self.folder.name)
        self.original = dict(id="m:1:1", meeting_id="m", run=1, seq=1, revision=1,
                             start_ms=0, end_ms=1000, text="The ASR misspelling remains.")
        self.corrected = dict(self.original, revision=2, text="The corrected name remains.")
        with closing(sqlite3.connect(self.root / "meetings.sqlite3")) as db, db:
            db.executescript("""CREATE TABLE meetings(id TEXT,json TEXT);
                CREATE TABLE notes(meeting_id TEXT,json TEXT);
                CREATE TABLE segments(meeting_id TEXT,run INTEGER,seq INTEGER,json TEXT);
                CREATE TABLE segment_history(id TEXT,revision INTEGER,json TEXT);
                CREATE TABLE segment_text_edits(id TEXT,origin TEXT);""")
            for table in ["library_entries", "suggestions", "document_history", "inference_jobs",
                          "translations", "translation_parts", "recording_takes"]:
                db.execute("CREATE TABLE " + table + "(id TEXT)")
            db.execute("INSERT INTO meetings VALUES(?,?)", ("m", json.dumps(dict(id="m", run=1))))
            db.execute("INSERT INTO notes VALUES(?,?)", ("m", json.dumps(dict(content=""))))
            db.execute("INSERT INTO segments VALUES(?,?,?,?)", ("m", 1, 1, json.dumps(self.corrected)))
            db.execute("INSERT INTO segment_history VALUES(?,?,?)", ("m:1:1", 1, json.dumps(self.original)))

    def test_raw_asr_comes_from_archived_first_revision(self):
        evidence = database_evidence(self.root, "m")
        self.assertEqual(evidence["raw"], [self.original])
        self.assertEqual(evidence["corrected"], [self.corrected])
        self.assertNotEqual(evidence["raw"][0]["text"], evidence["corrected"][0]["text"])

    def test_missing_original_cannot_be_replaced_with_corrected_text(self):
        with closing(sqlite3.connect(self.root / "meetings.sqlite3")) as db, db:
            db.execute("DELETE FROM segment_history")
        with self.assertRaises(ValueError): database_evidence(self.root, "m")

    def test_failed_arm_metrics_are_diagnostic_not_a_measured_pair(self):
        rows = [dict(arms=[dict(arm="baseline", passed=True,
                                acoustic_metrics=dict(expected_name_present=True)),
                           dict(arm="positive", passed=False,
                                acoustic_metrics=dict(expected_name_present=False))])]
        pair = observed_pair(rows, "acoustic_metrics", "positive", "expected_name_present")
        self.assertIsNone(pair["observed_delta"])
        self.assertEqual(pair["discordant_counts"]["unknown"], 1)


class RecordingContractTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = pathlib.Path(self.folder.name)
        self.audio = self.root / "meetings" / "m" / "audio"
        self.files = self.root / "meetings" / "m" / "files"
        self.audio.mkdir(parents=True); self.files.mkdir()
        self.pcm = b"\x02\x00" * 160
        self.prepared = dict(samples=160, sample_rate=16000, seconds=.01,
                             pcm_sha256=hashlib.sha256(b"\x01\x00" * 160).hexdigest())
        self.reference = dict(schema=1, decoder="native_wav_import", samples=160, pcm_bytes=320,
                              sample_rate=16000, pcm_sha256=hashlib.sha256(self.pcm).hexdigest())
        (self.audio / "000000.pcm").write_bytes(self.pcm)
        self.write_wav(self.pcm)

    def write_wav(self, payload):
        with wave.open(str(self.files / "recording-001.wav"), "wb") as wav:
            wav.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            wav.writeframes(payload)

    def test_production_conversion_has_its_own_strict_pcm_reference(self):
        result = recording_evidence(self.root, "m", self.prepared, self.reference)
        self.assertEqual(result["pcm_sha256"], self.reference["pcm_sha256"])
        self.assertNotEqual(result["pcm_sha256"], self.prepared["pcm_sha256"])

    def test_matching_archive_copies_cannot_replace_the_independent_reference(self):
        altered = b"\x03\x00" * 160
        (self.audio / "000000.pcm").write_bytes(altered)
        self.write_wav(altered)
        with self.assertRaises(ValueError):
            recording_evidence(self.root, "m", self.prepared, self.reference)

    def test_recording_wav_and_pcm_chunks_must_both_match(self):
        self.write_wav(b"\x03\x00" * 160)
        with self.assertRaises(ValueError):
            recording_evidence(self.root, "m", self.prepared, self.reference)


if __name__ == "__main__":
    unittest.main()
