#!/usr/bin/env python3
"""Paired complete-workload article-state experiment. Default policy is unchanged.

Keeps failed drafts/repairs/costs. Existing installed assets only, no downloads.
"""
import argparse, os, pathlib, platform, shutil, subprocess, sys, tempfile, time
from compare_minutes_models import PREFERENCES, backend_evidence, runtime_inventory, summarize
from harness_support import REPO, environment, read_json, sha256, write_json


def promotion_gate(results):
    """Both quality and total cost must pass; a cheap failed arm is not an optimization."""
    by_mode = {r['mode']: r for r in results}
    if set(by_mode) != {'legacy', 'incremental'} or len(results) != 2:
        return False
    if not all(r['report']['passed'] and r['exit_code'] == 0 and r['backend_evidence']['passed'] for r in results):
        return False
    old, new = (by_mode[k]['summary'] for k in ['legacy', 'incremental'])
    keys = ['input_tokens', 'output_tokens', 'process_seconds']
    if any(type(s.get(k)) not in [int, float] for s in [old, new] for k in keys):
        return False
    return new['input_tokens'] < old['input_tokens'] and new['input_tokens'] + new['output_tokens'] < old['input_tokens'] + old['output_tokens'] and new['process_seconds'] <= old['process_seconds']


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli',type=pathlib.Path,default=REPO/'target/debug/omascribe-cli')
    parser.add_argument('--model-dir',type=pathlib.Path,required=True)
    parser.add_argument('--runtime',type=pathlib.Path,required=True)
    parser.add_argument('--output',type=pathlib.Path,required=True)
    parser.add_argument('--backend',choices=['metal','cpu'],default='metal')
    parser.add_argument('--trials',type=int,choices=[1,2],default=2)
    args=parser.parse_args()
    if args.backend=='metal' and (platform.system(),platform.machine())!=('Darwin','arm64'):
        parser.error('Metal requires native Apple Silicon')
    cli,output,runtime=args.cli.resolve(),args.output.resolve(),args.runtime.resolve()
    artifacts=output.with_suffix('.artifacts')
    if output.exists() or artifacts.exists(): parser.error('Use fresh output paths')
    artifacts.mkdir(parents=True)
    model=next(m for m in read_json(REPO/'model-catalog.json')['models'] if m['id']=='qwen3-1.7b')
    native=runtime_inventory(runtime)
    if not native: parser.error('Missing llama runtime')
    expected=[c['name'] for c in read_json(REPO/'tests/fixtures/document-cases.json')]
    initial=environment(cli)
    report=dict(schema=1,kind='incremental-source-backed-article',passed=False,environment=initial,runtime_files=native,model_files=model['files'],
        expected_cases=expected,trials=[],errors=[],promotion_eligible=False,
        limitation='Experimental verified-article state, not a structured semantic fact ledger. Complete fictional pattern/citation/human-edit regression, not natural meeting comprehension. Includes all repairs and failed work; sampled CPU/RSS do not prove energy or unique unified memory. New policy remains off by default.')
    with tempfile.TemporaryDirectory(prefix='omascribe-article-models-') as temp:
        store=pathlib.Path(temp);(store/model['id']).mkdir()
        for item in model['files']:
            path=args.model_dir.resolve()/item['name']
            if path.stat().st_size!=item['bytes'] or sha256(path)!=item['sha256']: raise ValueError('Pinned model changed')
            try: os.link(path,store/model['id']/item['name'])
            except OSError: shutil.copy2(path,store/model['id']/item['name'])
        for trial in range(1,args.trials+1):
            results=[]
            for mode in (['legacy','incremental'] if trial%2 else ['incremental','legacy']):
                folder=artifacts/f'{trial}-{mode}';folder.mkdir()
                prefs={**PREFERENCES,'llm_model':model['id'],'inference_backend':args.backend}
                write_json(folder/'preferences.json',prefs)
                env=dict(os.environ,OMASCRIBE_TEST_MODEL_STORE=str(store),OMASCRIBE_MODELS_DIR=str(folder/'no-bundled-models'),OMASCRIBE_RUNTIME_DIR=str(runtime),
                    OMASCRIBE_RETRIEVAL_CACHE='0',OMASCRIBE_MINUTES_CATCHUP='0',OMASCRIBE_INCREMENTAL_MINUTES='1' if mode=='incremental' else '0')
                env.pop('LLAMA_ARG_LOG_VERBOSITY',None)
                destination=folder/'evaluation.json'
                print(f'Trial {trial} {mode}: {len(expected)} cases',flush=True)
                started=time.monotonic()
                process=subprocess.run([sys.executable,str(REPO/'tests/evaluate.py'),'--cli',str(cli),'--preferences',str(folder/'preferences.json'),'--output',str(destination)],cwd=REPO,env=env)
                measured=read_json(destination)
                if not measured: raise ValueError('Missing evaluation; raw process output retained')
                result=dict(mode=mode,exit_code=process.returncode,wall_seconds=time.monotonic()-started,report=measured,summary=summarize(measured),
                    backend_evidence=backend_evidence(measured,destination.with_suffix('.artifacts'),args.backend))
                result['state_events']=[row['metrics'] for case in measured['cases'] for run in case['runs'] for row in run['task_trace'] if row['phase']=='article_state']
                result['prompt_events']=[row['metrics'] for case in measured['cases'] for run in case['runs'] for row in run['task_trace'] if row['phase']=='article_prompt']
                results.append(result)
                write_json(output,{**report,'in_progress':dict(trial=trial,results=results)})
            comparable=True
            for r in results:
                v=r['report']
                if v['fixture_cases']!=expected or [c['case'] for c in v['cases']]!=expected or v['preferences']!=prefs or v['incremental_enabled']!=(r['mode']=='incremental'):
                    comparable=False
                if any(v['environment'][k]!=initial[k] for k in ['cli_build','cli_sha256','core_and_harness_source_sha256']): comparable=False
            delta=next(r for r in results if r['mode']=='incremental')
            if not any(e['mode']=='delta' for e in delta['prompt_events']): comparable=False
            report['trials'].append(dict(trial=trial,results=results,comparable=comparable,promotion_eligible=comparable and promotion_gate(results)))
            if not comparable: report['errors'].append(f'Trial {trial}: source/corpus/preferences or actual delta work mismatch')
            write_json(output,report)
        final=environment(cli)
        if any(final[k]!=initial[k] for k in ['cli_build','cli_sha256','core_and_harness_source_sha256','model_catalog_sha256','models_lock_sha256']) or runtime_inventory(runtime)!=native:
            report['errors'].append('Source/binary/runtime changed during measurement')
        for item in model['files']:
            if sha256(args.model_dir.resolve()/item['name'])!=item['sha256']: report['errors'].append('Model changed during measurement')
    report['passed']=not report['errors'] and len(report['trials'])==args.trials and all(r['report']['passed'] and r['backend_evidence']['passed'] for t in report['trials'] for r in t['results'])
    report['promotion_eligible']=report['passed'] and args.trials==2 and all(t['promotion_eligible'] for t in report['trials'])
    write_json(output,report)
    raise SystemExit(0 if report['passed'] else 1)


if __name__=='__main__': main()
