import contextlib, importlib.util, io, itertools, json, pathlib, sys, tempfile, unittest, wave
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "evaluate", pathlib.Path(__file__).with_name("evaluate.py")
)
evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluate)


class HarnessTests(unittest.TestCase):
    def test_late_note_gate_rejects_old_case_even_when_regex_names_match(self):
        source = dict(id="s", seq=1, revision=2, text="CedarLedger remains incomplete.")
        citation = dict(segment_id="s", revision=2, quote=source["text"])
        snapshot = dict(segments=[source], suggestions=[dict(status="pending", text=source["text"], sources=[citation])], runtime=dict(notes_status=""))
        case = dict(required=["CedarLedger"], required_verbatim=["CedarLedger"], forbidden_verbatim=["CEDARLEDGER"])
        self.assertEqual(evaluate.assess(case, snapshot), [])
        snapshot["suggestions"][0]["text"] = "CEDARLEDGER remains incomplete."
        self.assertTrue(evaluate.assess(case, snapshot))

    def test_retention_assertions_do_not_count_notes_alone_as_recording(self):
        cases=json.loads((pathlib.Path(__file__).parent/'fixtures/document-cases.json').read_text())
        case=next(c for c in cases if c['name']=='zh_windows_uppercase_asr')
        check=next(c for c in case['translation_checks'] if c['seq']==3)
        source=dict(id='s',seq=3,revision=1,text=case['transcript'][2])
        snapshot=dict(segments=[source],suggestions=[],runtime=dict(notes_status=''),translations={'s':dict(target='zh',revision=1,text='')})
        narrowed=dict(expect_empty=True,translation_checks=[check])
        snapshot['translations']['s']['text']='星期五不是截止日期，会议笔记中的录音应留在此电脑上。'
        self.assertEqual(evaluate.assess(narrowed,snapshot,True),[])
        snapshot['translations']['s']['text']='星期五不是截止日期，会议记录应留在此电脑上。'
        self.assertTrue(evaluate.assess(narrowed,snapshot,True))

    def test_weekday_caption_does_not_add_an_unspoken_week(self):
        cases=json.loads((pathlib.Path(__file__).parent/'fixtures/document-cases.json').read_text())
        case=next(c for c in cases if c['name']=='en_incremental_separate_topics')
        check=next(c for c in case['translation_checks'] if c['seq']==2)
        source=dict(id='s',seq=2,revision=1,text=case['phases'][1]['transcript'][0])
        snapshot=dict(segments=[source],suggestions=[],runtime=dict(notes_status=''),translations={'s':dict(target='zh',revision=1,text='')})
        narrowed=dict(expect_empty=True,translation_checks=[check])
        for weekday in ['周二','星期二']:
            snapshot['translations']['s']['text']=f'Riley将在{weekday}测试键盘可访问性，尚未完成任何测试。'
            self.assertEqual(evaluate.assess(narrowed,snapshot,True),[])
        for weekday in ['本周二','这个星期二','下周二']:
            snapshot['translations']['s']['text']=f'Riley在{weekday}将测试键盘可访问性，尚未完成任何测试。'
            self.assertTrue(evaluate.assess(narrowed,snapshot,True))

    def test_uppercase_caption_assertions_reject_reversed_actor_object(self):
        cases=json.loads((pathlib.Path(__file__).parent/'fixtures/document-cases.json').read_text())
        case=next(c for c in cases if c['name']=='zh_windows_uppercase_asr')
        source=dict(id='s',seq=5,revision=1,text=case['transcript'][4])
        snapshot=dict(segments=[source],suggestions=[],runtime=dict(notes_status=''),translations={'s':dict(target='zh',revision=1,text='')})
        check=dict(expect_empty=True,translation_checks=[c for c in case['translation_checks'] if c['seq']==5])
        for text in ['尚未获批。ALICE将检查UN VIDEO CARD。','尚未获批。UN VIDEO CARD将由ALICE检查。']:
            snapshot['translations']['s']['text']=text
            self.assertEqual(evaluate.assess(check,snapshot,True),[])
        snapshot['translations']['s']['text']='它尚未批准UN VIDEO CARD将检查ALICE'
        self.assertTrue(evaluate.assess(check,snapshot,True))

    def test_submission_caption_assertions_reject_completion_without_rejecting_future_passive(self):
        cases=json.loads((pathlib.Path(__file__).parent/'fixtures/document-cases.json').read_text())
        case=next(c for c in cases if c['name']=='zh_named_speakers')
        source=dict(id='s',seq=1,revision=1,text=case['transcript'][0]['text'])
        snapshot=dict(segments=[source],suggestions=[],runtime=dict(notes_status=''),translations={'s':dict(target='en',revision=1,text='')})
        check=dict(expect_empty=True,translation_checks=case['translation_checks'])
        wrong='The real-time transcription delay test I am responsible for was submitted on Thursday.'
        snapshot['translations']['s']['text']=wrong
        self.assertTrue(evaluate.assess(check,snapshot,True))
        for passive in ['will be submitted','are to be submitted']:
            snapshot['translations']['s']['text']='I am responsible for the latency test. The results '+passive+' on Thursday.'
            self.assertEqual(evaluate.assess(check,snapshot,True),[])
    def test_future_owner_consent_assertion_accepts_equivalent_negation_but_not_obtained_consent(self):
        cases=json.loads((pathlib.Path(__file__).parent/'fixtures/document-cases.json').read_text())
        case=next(c for c in cases if c['name']=='en_future_named_task_owner')
        source=dict(id='s',seq=1,text=case['transcript'][0],revision=1)
        citation=dict(segment_id='s',revision=1,quote=source['text'])
        def assess(text):
            snapshot=dict(segments=[source],suggestions=[dict(status='pending',text=text,sources=[citation])],runtime=dict(notes_status=''),translations={'s':dict(revision=1,target='en',text=text)})
            return evaluate.assess(case,snapshot,True)
        for clause in ['currently no user consent has been obtained','user consent has not yet been obtained','user consent is still pending','the user has not given consent']:
            self.assertEqual(assess('Model download will be handled by 陈明, but '+clause+'.'),[])
        self.assertTrue(assess('Model download will be handled by 陈明, and user consent has been obtained.'))
    def test_caption_assertions_catch_wrong_names_language_and_revision_independently_of_minutes(self):
        source = dict(id="s", seq=1, text="Alice will check the model.", revision=2)
        snapshot = dict(suggestions=[], segments=[source], runtime=dict(notes_status=""), translations={})
        case = dict(expect_empty=True, translation_checks=[dict(seq=1,target="zh",required=["Alice", "检查"],forbidden=["艾莉丝"])])
        self.assertEqual(evaluate.assess(case, snapshot), [])  # Explicit minutes-only mode.
        self.assertIn("Missing translation", evaluate.assess(case, snapshot, True)[0])
        snapshot["translations"]["s"] = dict(target="zh",revision=2,text="Alice将检查模型。")
        self.assertEqual(evaluate.assess(case, snapshot, True), [])
        for text in ["艾莉丝将检查模型。", source["text"]]:
            snapshot["translations"]["s"]["text"] = text
            self.assertTrue(evaluate.assess(case, snapshot, True))
        snapshot["translations"]["s"].update(text="Alice将检查模型。", revision=1)
        self.assertIn("revision/target", evaluate.assess(case, snapshot, True)[0])
    def test_gpu_fact_gate_preserves_negation_and_types_while_accepting_weekday_case(self):
        from benchmark_windows_gpu import correct_facts
        valid = dict(day="thursday", rejected_day="friday", budget=3000, approved=False)
        self.assertTrue(correct_facts(valid))
        for change in [dict(rejected_day="Thursday"), dict(day="Friday"), dict(budget=3001),
                       dict(approved=True), dict(approved=0), dict(budget="3000"), dict(extra="invented")]:
            self.assertFalse(correct_facts({**valid, **change}))
    def test_apple_comparison_needs_per_command_offload_and_counts_shortcuts_separately(self):
        from compare_minutes_models import backend_evidence
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            path = root / "fixture/phase-1/batch-1/llm-runtime.log"
            path.parent.mkdir(parents=True)
            report = dict(cases=[dict(case="fixture", runs=[dict(phase=1, batch=1, generation=dict(backend="metal"))])])
            self.assertFalse(backend_evidence(report, root, "metal")["passed"])
            path.write_text("load_tensors: offloaded 29/29 layers to GPU\n")
            self.assertTrue(backend_evidence(report, root, "metal")["passed"])
            self.assertFalse(backend_evidence(report, root, "cpu")["passed"])
            path.write_text("load_tensors: offloaded 0/29 layers to GPU\n")
            self.assertFalse(backend_evidence(report, root, "metal")["passed"])
            report["cases"][0]["runs"][0]["generation"] = dict(backend="cpu")
            self.assertTrue(backend_evidence(report, root, "cpu")["passed"])
            report["cases"][0]["runs"][0]["generation"] = dict(reason="non_substantive_only", attempt=0)
            self.assertFalse(backend_evidence(report, root, "metal")["passed"])

    def test_apple_pair_cannot_silently_fall_back_or_compare_different_weights(self):
        import copy
        from compare_minutes_models import PREFERENCES, validate_pair
        variants = [("qwen3-1.7b", backend) for backend in ["cpu", "metal"]]
        environment = dict(cli_sha256="same", cli_build={}, core_and_harness_source_sha256="same")
        results = [dict(model=model, backend=backend, exit_code=0, report=dict(schema=3, fixture_sha256="same", fixture_cases=["fixture"],
                   cases=[dict(case="fixture", passed=True, runs=[dict(generation=dict(model=model, backend=backend))])],
                   environment=environment, preferences={**PREFERENCES, "llm_model": model, "inference_backend": backend}, passed=True)) for model, backend in variants]
        validate_pair(results, ["fixture"], variants=variants)
        for change in [lambda r: r.update(model="qwen3-0.6b"),
                       lambda r: r["report"]["cases"][0]["runs"][0]["generation"].update(backend="cpu")]:
            bad = copy.deepcopy(results)
            change(bad[1])
            with self.assertRaises(AssertionError):
                validate_pair(bad, ["fixture"], variants=variants)

    def test_model_comparison_counts_rejected_and_failed_work_and_missing_cost(self):
        from compare_minutes_models import summarize
        valid = dict(attempt=2, elapsed_ms=2000, usage=dict(prompt_tokens=20, completion_tokens=4))
        rejected = dict(attempt=1, elapsed_ms=1000, validation_error="fixture", usage=dict(prompt_tokens=10, completion_tokens=3))
        partial = dict(attempt=1, elapsed_ms=500, failure="incomplete_local_response")
        report = dict(cases=[dict(passed=True, seconds=4., runs=[dict(generation=dict(attempts=[rejected, valid]),
                      process=dict(seconds=3.5, resources=dict(cpu_seconds_sampled=2., peak_tree_rss_mib=100.)))]),
                      dict(passed=False, seconds=1., runs=[dict(generation=dict(attempts=[partial]),
                      process=dict(seconds=.8, resources={}))])])
        result = summarize(report)
        self.assertEqual(result["attempts"], 3)
        self.assertEqual(result["repairs"], 1)
        self.assertEqual(result["rejected_attempts"], 2)
        self.assertEqual(result["attempt_seconds"], 3.5)
        self.assertEqual(result["case_seconds"], 5.)
        self.assertEqual(result["process_seconds"], 4.3)
        self.assertIsNone(result["sampled_cpu_seconds"])
        self.assertEqual(result["sampled_cpu_known_seconds"], 2.)
        self.assertEqual(result["commands_missing_cpu"], 1)
        self.assertIsNone(result["input_tokens"])
        self.assertEqual(result["passed_cases"], 1)

    def test_model_comparison_planned_source_stages_are_paid_requests_not_repairs(self):
        from compare_minutes_models import summarize
        attempts = [dict(attempt=1, stage="prepare_sources", elapsed_ms=1000,
                         usage=dict(prompt_tokens=10, completion_tokens=3)),
                    dict(attempt=2, stage="compose_prepared_sources", elapsed_ms=2000,
                         usage=dict(prompt_tokens=20, completion_tokens=4))]
        report = dict(cases=[dict(passed=True, seconds=4., runs=[dict(generation=dict(attempts=attempts),
                      process=dict(seconds=3.5, resources={}))])])
        result = summarize(report)
        self.assertEqual((result["attempts"], result["repairs"]), (2, 0))
        self.assertEqual((result["attempt_seconds"], result["input_tokens"], result["output_tokens"]), (3., 30, 7))

    def test_model_comparison_stage_overrides_attempt_and_legacy_keeps_its_meaning(self):
        from compare_minutes_models import summarize
        for stage, number, expected in (("prepare_sources", 2, 0), ("compose", 2, 0),
                                        ("compose_prepared_sources", 2, 0), ("repair", 1, 1),
                                        (None, 2, 1)):
            with self.subTest(stage=stage):
                attempt = dict(attempt=number, elapsed_ms=1000, failure="fixture",
                               usage=dict(prompt_tokens=10, completion_tokens=3))
                if stage is not None:
                    attempt["stage"] = stage
                report = dict(cases=[dict(passed=False, seconds=1., runs=[dict(generation=dict(attempts=[attempt]), process={})])])
                result = summarize(report)
                self.assertEqual(result["repairs"], expected)
                self.assertEqual((result["attempts"], result["rejected_attempts"], result["attempt_seconds"]), (1, 1, 1.))
        attempt["stage"] = "unknown_stage"
        with self.assertRaises(ValueError):
            summarize(report)

    def test_model_comparison_refuses_partial_cases_and_different_settings(self):
        import copy
        from compare_minutes_models import MODELS, PREFERENCES, validate_pair
        environment = dict(cli_sha256="same", cli_build=dict(source_sha256="same"), core_and_harness_source_sha256="same")
        results = [dict(model=model, exit_code=0, report=dict(schema=3, fixture_sha256="same", fixture_cases=["fixture"],
                   cases=[dict(case="fixture", passed=True)], environment=environment,
                   preferences={**PREFERENCES, "llm_model": model}, passed=True)) for model in MODELS]
        validate_pair(results, ["fixture"])
        for change in [lambda r: r["report"]["cases"].clear(),
                       lambda r: r["report"]["preferences"].update(threads=4),
                       lambda r: r["report"]["environment"].update(cli_sha256="other"),
                       lambda r: r.update(exit_code=3)]:
            bad = copy.deepcopy(results)
            # Keep each report's environment independent even in this fixture.
            bad[1]["report"]["environment"] = copy.deepcopy(environment)
            change(bad[1])
            with self.assertRaises(AssertionError):
                validate_pair(bad, ["fixture"])

    def test_process_tree_resources_are_sampled_or_explicitly_unknown(self):
        import process_resources
        from harness_support import command
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            child = "import time; payload=bytearray(12*1024*1024); time.sleep(.8)"
            program = "import subprocess,sys; subprocess.run([sys.executable,'-c'," + repr(child) + "]); print('{}')"
            result = command([sys.executable, "-c", program], None, root / "live", timeout=5)
            self.assertEqual(result, {})
            report = json.loads((root / "live/process.json").read_text())["resources"]
            self.assertTrue(report["sampler_stopped"])
            self.assertIsNone(report["gpu_memory_mib"])
            if process_resources._psutil is not None:
                self.assertGreater(report["samples"], 0)
                self.assertGreaterEqual(report["processes_seen"], 2)
                self.assertGreater(report["peak_tree_rss_mib"], 0)
                self.assertGreaterEqual(report["cpu_seconds_sampled"], 0)
            with patch.object(process_resources, "_psutil", None):
                unknown = process_resources.ProcessTreeSampler(0).start().finish()
                self.assertEqual(unknown["samples"], 0)
                for key in ["cpu_seconds_sampled", "peak_tree_rss_mib", "peak_tree_pss_mib"]:
                    self.assertIsNone(unknown[key])

    def test_minutes_failure_keeps_its_attempt_and_resource_cost(self):
        from types import SimpleNamespace
        from harness_support import read_json, write_json
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            cli = root / "fictional-cli"
            cli.touch()
            artifacts = root / "artifacts"
            case = dict(name="fictional", focus="budget", transcript=["Budget pending"])
            def invoke(argv, env, directory, timeout=650):
                if argv[1] == "generate":
                    directory.mkdir(parents=True)
                    write_json(directory / "process.json", dict(code=3, seconds=2., failure=None,
                               resources=dict(cpu_seconds_sampled=.5, peak_tree_rss_mib=20.)))
                    write_json(artifacts / "workspace/last-generation.json", dict(attempts=[dict(elapsed_ms=1000, validation_error="fixture rejection")]))
                    raise RuntimeError("fictional process failure")
                return dict(suggestions=[], segments=[], meeting=dict(notes_cursor=0), runtime=dict(notes_status="failed"))
            def read(path):
                return [case] if pathlib.Path(path).name == "document-cases.json" else read_json(path)
            args = SimpleNamespace(cli=cli, output=root / "report.json", artifacts=artifacts, preferences=None, case=None)
            with patch.object(evaluate, "command", invoke), patch.object(evaluate, "environment", return_value={}), \
                 patch.object(evaluate, "read_json", read), patch.object(evaluate, "seed"), patch.object(evaluate, "append", return_value=1), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(evaluate.evaluate(args))
            result = read_json(args.output)["cases"][0]
            self.assertFalse(result["passed"])
            self.assertEqual(len(result["runs"]), 1)
            run = result["runs"][0]
            self.assertEqual(run["status"], "process-failed")
            self.assertEqual(run["generation"]["attempts"][0]["elapsed_ms"], 1000)
            self.assertEqual(run["process"]["resources"]["cpu_seconds_sampled"], .5)

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
    def test_missing_speech_cannot_disappear_from_quality_or_cost_totals(self):
        scored = dict(edits=1,reference_units=10,substitutions=1,deletions=0,insertions=0,
                      wall_seconds=2.,audio_seconds=10.,rtf=.1,
                      process=dict(resources=dict(cpu_seconds_sampled=.5,peak_tree_rss_mib=50.)))
        failed = dict(wall_seconds=3.,errors=["worker failed"],process=dict(resources={}))
        summary=asr.aggregate_cases([scored,failed])
        self.assertEqual(summary['clips'],2)
        self.assertEqual(summary['scored_clips'],1)
        self.assertIsNone(summary['error_rate'])
        self.assertIsNone(summary['error_rate_interval_95'])
        self.assertEqual(summary['wall_seconds'],5.)
        self.assertIsNone(summary['audio_seconds'])
        self.assertIsNone(summary['process_tree_cpu_seconds_sampled'])
        self.assertEqual(summary['process_tree_cpu_known_seconds'],.5)
        self.assertEqual(summary['missing_cpu_clips'],1)
        self.assertEqual(summary['peak_process_tree_rss_mib'],50.)
        self.assertTrue(asr.regression({'missing/model':{'error_rate':.1}}, {}, .02))
        self.assertTrue(asr.regression({'m/zh':{'error_rate':.1}}, {'m/zh':{'error_rate':None}}, .02))

    def test_asr_assets_are_pinned_and_worker_identity_is_part_of_evidence(self):
        from harness_support import sha256
        with tempfile.TemporaryDirectory() as folder:
            root=pathlib.Path(folder); model=root/'model-store/fixture';model.mkdir(parents=True)
            weights=model/'encoder.onnx';weights.write_bytes(b'fictional-weight')
            worker=root/('omascribe-asr.exe' if sys.platform=='win32' else 'omascribe-asr');worker.write_bytes(b'fixture-worker')
            spec=dict(id='fixture',kind='asr',bundled=False,files=[dict(name=weights.name,bytes=weights.stat().st_size,sha256=sha256(weights))])
            with patch.object(asr,'read_json',return_value={'models':[spec]}):
                first=asr.asset_identity(root/'cli',root,'fixture',{'OMASCRIBE_RUNTIME_DIR':str(root)})
                self.assertEqual(first['worker']['sha256'],sha256(worker))
                worker.write_bytes(b'changed-worker')
                self.assertNotEqual(first,asr.asset_identity(root/'cli',root,'fixture',{'OMASCRIBE_RUNTIME_DIR':str(root)}))
                weights.write_bytes(b'tamper')
                with self.assertRaisesRegex(ValueError,'pinned catalog'):
                    asr.asset_identity(root/'cli',root,'fixture',{'OMASCRIBE_RUNTIME_DIR':str(root)})

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
                with patch('sys.argv',args), patch.object(asr,'command',side_effect=response), patch.object(asr,'environment',return_value={'kind':'fictional-scorer-unit'}), patch.object(asr,'isolated_env',return_value={}), patch.object(asr,'asset_identity',return_value={'kind':'fictional-scorer-unit'}), contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(SystemExit) as exit_state:asr.main()
                return exit_state.exception.code,json.loads(output.read_text())
            code,report=run(root/'first.json')
            self.assertEqual(code,0);self.assertTrue(report['passed'])
            self.assertFalse((corpus/'silence.wav').exists())
            self.assertFalse((corpus/'quiet-noise.wav').exists())
            self.assertEqual(report['cases'][0]['audio_sha256'],sha256(audio))
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
