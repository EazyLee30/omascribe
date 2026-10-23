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


if __name__ == "__main__":
    unittest.main()
