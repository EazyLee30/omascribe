"""Reference/identity controls for the real-audio name-bias harness; no inference."""
import copy
import csv
import hashlib
import pathlib
import json
import sqlite3
import tempfile
import unittest

from prepare_name_bias_fixtures import REVISION, occurs, selection_rows
from evaluate_name_bias import database_evidence, observed_pair


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
        with sqlite3.connect(self.root / "meetings.sqlite3") as db:
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
        with sqlite3.connect(self.root / "meetings.sqlite3") as db:
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


if __name__ == "__main__":
    unittest.main()
