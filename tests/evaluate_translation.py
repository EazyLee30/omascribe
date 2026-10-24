#!/usr/bin/env python3
"""Real local-model translation regression using invented passages only."""
import json, os, pathlib, sqlite3, subprocess, tempfile, time, uuid
REPO=pathlib.Path(__file__).resolve().parents[1]
CLI=REPO/'target/debug/omascribe-cli'
with tempfile.TemporaryDirectory(prefix='omascribe-translation-test-') as folder:
    root=pathlib.Path(folder)
    env=dict(os.environ,OMASCRIBE_DATA_DIR=folder)
    prefs={'translation_mode':'bilingual','auto_notes':False}
    (root/'preferences.json').write_text(json.dumps(prefs))
    def cli(*args):
        return json.loads(subprocess.check_output([str(CLI),*args],env=env,text=True,timeout=180))
    cli('diagnose')
    mid=str(uuid.uuid4())
    meeting=dict(id=mid,title='Example · Translation',created_at='2026-10-24T00:00:00Z',status='ready',duration_ms=20000,run=1,notes_cursor=0,auto_notes=False)
    originals=['预算还没有确定，周五再讨论。','Morgan will send the report on Thursday. The launch date is not approved.']
    with sqlite3.connect(root/'meetings.sqlite3') as db:
        db.execute('INSERT INTO meetings VALUES(?,?)',(mid,json.dumps(meeting)))
        for i,text in enumerate(originals):
            segment=dict(id=str(uuid.uuid4()),meeting_id=mid,run=1,seq=i+1,start_ms=i*10000,end_ms=(i+1)*10000,text=text,revision=1)
            db.execute('INSERT INTO segments VALUES(?,?,?,?,?)',(segment['id'],mid,1,i+1,json.dumps(segment)))
    start=time.monotonic();snapshot=cli('translate',mid)
    assert len(snapshot['translations'])==2,snapshot
    values=list(snapshot['translations'].values())
    en=next(t['text'] for t in values if t['target']=='en');zh=next(t['text'] for t in values if t['target']=='zh')
    assert 'not' in en.lower() and 'friday' in en.lower(),en
    assert any(w in zh for w in ['未','不','没有']) and any(day in zh for day in ['周四','星期四','礼拜四']),zh
    assert [s['text'] for s in snapshot['segments']]==originals
    for mode,target in [('zh-en','en'),('en-zh','zh'),('off',None)]:
        prefs['translation_mode']=mode;(root/'preferences.json').write_text(json.dumps(prefs))
        current=cli('snapshot',mid)
        assert len(current['translations'])==(1 if target else 0)
        if target:assert list(current['translations'].values())[0]['target']==target
    report=dict(passed=True,seconds=round(time.monotonic()-start,2),translations=values,checks=['real Chinese-to-English and English-to-Chinese','negation and weekday preserved','originals unchanged','four modes select correct cached target'])
    pathlib.Path('/tmp/omascribe-032-translation.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
    print(json.dumps(report,ensure_ascii=False))
