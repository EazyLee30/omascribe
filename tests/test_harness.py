import contextlib, importlib.util, io, itertools, json, pathlib, sys, tempfile, unittest, wave
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "evaluate", pathlib.Path(__file__).with_name("evaluate.py")
)
evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluate)


class HarnessTests(unittest.TestCase):
    def test_lexical_benchmark_refuses_changed_inputs_rankings_or_builds(self):
        import copy
        from benchmark_retrieval import MODES, validate_trial
        results = [dict(mode=mode, schema=1, kind="lexical-only", initial_documents=1000,
                        source_unchanged=True, rank_ms=[1.] * 9, inputs_sha256="a" * 64,
                        rankings_sha256="b" * 64, build=dict(source_sha256="c" * 64),
                        accounted_retained_bytes=0, budget_bytes=8*1024**2,
                        retained_documents=0, indexed_documents_built=0) for mode in MODES]
        validate_trial(results, 1000)
        for key, value in [("inputs_sha256", "d"*64), ("rankings_sha256", "d"*64),
                           ("build", dict(source_sha256="d"*64)), ("source_unchanged", False),
                           ("rank_ms", [float("nan")] * 9), ("accounted_retained_bytes", 9*1024**2)]:
            bad = copy.deepcopy(results)
            bad[1][key] = value
            with self.assertRaises(AssertionError, msg=key):
                validate_trial(bad, 1000)
        with self.assertRaises(AssertionError):
            validate_trial(results[:-1], 1000)

    def test_worker_resource_probe_captures_failures_and_reaps_timeouts(self):
        from benchmark_hotword_lifecycle import invoke
        with tempfile.TemporaryDirectory() as folder:
            root=pathlib.Path(folder)
            result=invoke([sys.executable,'-c','print("{}")'],None,root/'success')
            self.assertGreater(result['wall_ms'],0)
            if hasattr(__import__('os'),'wait4'):
                self.assertIsNotNone(result['worker_cpu_seconds'])
                self.assertGreaterEqual(result['worker_cpu_seconds'],0)
                self.assertGreater(result['worker_peak_rss_mib'],0)
            with self.assertRaisesRegex(RuntimeError,'exited 3'):
                invoke([sys.executable,'-c','raise SystemExit(3)'],None,root/'failed')
            with self.assertRaises(TimeoutError):
                invoke([sys.executable,'-c','import time;time.sleep(10)'],None,root/'timeout',timeout=.02)

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
    def test_offline_scorer_reports_mer_and_refuses_incompatible_baseline(self):
        from harness_support import sha256
        with tempfile.TemporaryDirectory() as folder:
            root=pathlib.Path(folder); corpus=root/'corpus'; corpus.mkdir()
            audio=corpus/'fictional.wav'
            with wave.open(str(audio),'wb') as out:
                out.setparams((1,2,16000,0,'NONE','not compressed'));out.writeframes(b'\0'*640)
            reference='预算OmaScribe尚未批准'
            (corpus/'manifest.json').write_text(json.dumps(dict(cases=[dict(name='fictional_mixed',language='mixed',file=audio.name,reference=reference,sha256=sha256(audio),kind='fictional-scorer-unit')],schema=2)),encoding='utf-8')
            def response(argv,env,directory,timeout):
                text=reference if str(argv[-1])=='fictional_mixed' else ''
                return dict(meeting=dict(id='fictional-meeting',status='ready',duration_ms=1000),
                            runtime=dict(error=None,asr_rtf=.1),segments=[dict(text=text,start_ms=0,end_ms=1000)] if text else [])
            def run(output,baseline=None):
                args=['evaluate_asr','--fixtures',str(corpus),'--output',str(output),'--models','fictional']
                if baseline:args+=['--baseline',str(baseline)]
                with patch('sys.argv',args), patch.object(asr,'command',side_effect=response), patch.object(asr,'environment',return_value={'kind':'fictional-scorer-unit'}), patch.object(asr,'isolated_env',return_value={}), contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(SystemExit) as exit_state:asr.main()
                return exit_state.exception.code,json.loads(output.read_text())
            code,report=run(root/'first.json')
            self.assertEqual(code,0);self.assertTrue(report['passed'])
            self.assertEqual(report['cases'][0]['metric'],'MER')
            self.assertEqual(report['aggregates']['fictional/mixed']['error_rate'],0)
            self.assertIsNone(report['aggregates']['fictional/mixed']['gpu_memory_mib'])
            report['scorer']='incompatible-scorer';baseline=root/'baseline.json';baseline.write_text(json.dumps(report))
            code,report=run(root/'second.json',baseline)
            self.assertEqual(code,1);self.assertFalse(report['passed'])
            self.assertTrue(any('scorer differs' in e for e in report['errors']))

    def test_mixed_units_keep_names_numbers_and_non_ascii_letters(self):
        self.assertEqual(asr.units("测试OmaScribe，预算３０００；don't visit café。", "mixed"),
                         ["测","试","omascribe","预","算","3000","don't","visit","café"])
        self.assertEqual(asr.error_counts(["budget","not","approved"],["budget","approved"]),
                         dict(substitutions=0,deletions=1,insertions=0))
        self.assertEqual(asr.error_counts(["a","b","c"],["a","x","c","d"]),
                         dict(substitutions=1,deletions=0,insertions=1))

    def test_edit_breakdown_matches_minimum_distance_exhaustively(self):
        from functools import lru_cache
        @lru_cache(None)
        def distance(left, right):
            if not left: return len(right)
            if not right: return len(left)
            return min(distance(left[1:],right)+1,distance(left,right[1:])+1,
                       distance(left[1:],right[1:])+(left[0]!=right[0]))
        sequences = [p for n in range(4) for p in itertools.product("ab",repeat=n)]
        for left in sequences:
            for right in sequences:
                self.assertEqual(sum(asr.error_counts(left,right).values()),distance(left,right))
        with self.assertRaisesRegex(ValueError,'Scoring budget'):
            asr.error_counts(['a']*2001,['a']*2001)

    def test_latency_and_error_intervals_are_deterministic_and_unknown_is_not_zero(self):
        self.assertIsNone(asr.percentile([None,float('nan')],.95))
        self.assertAlmostEqual(asr.percentile([10,20,30],.95),29)
        data = [dict(edits=0,reference_units=10),dict(edits=10,reference_units=10)]
        self.assertEqual(asr.error_interval(data),asr.error_interval(data))
        self.assertEqual(asr.error_interval(data),[0,1])
        self.assertIsNone(asr.error_interval([]))
    def test_normalization_does_not_hide_missing_words_or_numbers(self):
        self.assertEqual(asr.units("会议，２０２６。", "cmn_hans_cn"), list("会议2026"))
        self.assertEqual(asr.units("Don't re-record.", "en_us"), ["don't", "re", "record"])
        self.assertEqual(asr.edit_distance(["budget", "not", "approved"], ["budget", "approved"]), 1)
        self.assertEqual(asr.edit_distance(list("预算未定"), []), 4)

    def test_baseline_missing_or_regressed_scores_fail(self):
        self.assertTrue(asr.regression({}, {"model/en": {"error_rate":.1}}, .02))
        self.assertTrue(asr.regression({"model/en": {"error_rate":.1}}, {"model/en": {"error_rate":.13}}, .02))
        self.assertFalse(asr.regression({"model/en": {"error_rate":.1}}, {"model/en": {"error_rate":.09}}, .02))

class CompletionTests(unittest.TestCase):
    def test_partial_and_empty_runs_cannot_report_success(self):
        from harness_support import suite_passed
        self.assertFalse(suite_passed([], 0))
        self.assertFalse(suite_passed([{"passed": True}], 2))
        self.assertFalse(suite_passed([{"passed": True}, {"passed": False}], 2))
        self.assertTrue(suite_passed([{"passed": True}, {"passed": True}], 2))


class MixedCorpusTests(unittest.TestCase):
    def test_splice_records_provenance_and_detects_changed_source_audio(self):
        from prepare_asr_fixtures import add_mixed, digest, REVISION
        with tempfile.TemporaryDirectory() as folder:
            root, cases = pathlib.Path(folder), []
            for language in ["cmn_hans_cn","en_us"]:
                for index in range(3):
                    name=f"{language}-{index}"
                    path=root/(name+".wav")
                    with wave.open(str(path),"wb") as out:
                        out.setparams((1,2,16000,0,"NONE","not compressed"));out.writeframes(b'\0'*640)
                    cases.append(dict(name=name,language=language,file=path.name,reference="虚构测试" if language.startswith("cmn") else "fictional test",sha256=digest(path)))
            manifest=dict(dataset="google/fleurs",revision=REVISION,license="CC-BY-4.0",cases=cases)
            result=add_mixed(root,manifest)
            self.assertEqual(len(result["cases"]),8)
            mixed=[c for c in result["cases"] if c["language"]=="mixed"]
            self.assertEqual(len(mixed),2)
            for case in mixed:
                self.assertEqual(case["kind"],"constructed-language-switches")
                self.assertEqual(case["pieces"][1]["start_sample"],4320)
                with wave.open(str(root/case["file"]),"rb") as audio:
                    self.assertEqual(audio.getnframes(),8960)
                self.assertEqual(digest(root/case["file"]),case["sha256"])
            self.assertEqual(add_mixed(root,result),result)
            before=(root/"manifest.json").read_bytes()
            (root/cases[0]["file"]).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError,"checksum"):
                add_mixed(root,manifest)
            self.assertEqual((root/"manifest.json").read_bytes(),before)
            self.assertEqual(list(root.glob('*.part')),[])

if __name__ == "__main__":
    unittest.main()
