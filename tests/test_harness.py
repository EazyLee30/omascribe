import importlib.util, pathlib, unittest

spec = importlib.util.spec_from_file_location(
    "evaluate", pathlib.Path(__file__).with_name("evaluate.py")
)
evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluate)


class HarnessTests(unittest.TestCase):
    def test_false_success_and_forged_citation_are_detected(self):
        case = dict(required=["pending"], forbidden=["3000"])
        snapshot = dict(
            suggestions=[
                dict(
                    status="pending",
                    text="Approved 3000",
                    sources=[dict(segment_id="s", revision=1, quote="invented")],
                )
            ],
            segments=[dict(id="s", revision=1, text="Budget pending")],
            runtime=dict(notes_status="done"),
        )
        self.assertEqual(len(evaluate.assess(case, snapshot)), 3)

    def test_empty_generation_is_not_a_pass(self):
        self.assertTrue(
            evaluate.assess(
                dict(required=[], forbidden=[]),
                dict(suggestions=[], segments=[], runtime=dict(notes_status="failed")),
            )
        )


# Metric correctness matters: a broken scorer can make a quality regression look green.
spec_asr = importlib.util.spec_from_file_location("evaluate_asr", pathlib.Path(__file__).with_name("evaluate_asr.py"))
asr = importlib.util.module_from_spec(spec_asr)
spec_asr.loader.exec_module(asr)

class AsrMetricTests(unittest.TestCase):
    def test_normalization_does_not_hide_missing_words_or_numbers(self):
        self.assertEqual(asr.units("会议，２０２６。", "cmn_hans_cn"), list("会议2026"))
        self.assertEqual(asr.units("Don't re-record.", "en_us"), ["don't", "re", "record"])
        self.assertEqual(asr.edit_distance(["budget", "not", "approved"], ["budget", "approved"]), 1)
        self.assertEqual(asr.edit_distance(list("预算未定"), []), 4)

    def test_baseline_missing_or_regressed_scores_fail(self):
        self.assertTrue(asr.regression({}, {"model/en": {"error_rate":.1}}, .02))
        self.assertTrue(asr.regression({"model/en": {"error_rate":.1}}, {"model/en": {"error_rate":.13}}, .02))
        self.assertFalse(asr.regression({"model/en": {"error_rate":.1}}, {"model/en": {"error_rate":.09}}, .02))

if __name__ == "__main__":
    unittest.main()
