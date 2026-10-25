#!/usr/bin/env python3
"""Accelerated long-recording regression on repeated public audio, not a diverse-meeting benchmark."""
import argparse, hashlib, pathlib, time, wave
from harness_support import REPO, command, isolated_env, write_json

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--cli',type=pathlib.Path,default=REPO/'target/debug/omascribe-cli')
    p.add_argument('--wav',type=pathlib.Path,required=True)
    p.add_argument('--minutes',type=int,default=30)
    p.add_argument('--output',type=pathlib.Path,required=True)
    a=p.parse_args()
    if not 1<=a.minutes<=120:p.error('Stress duration must be 1–120 minutes')
    folder=a.output.resolve().with_suffix('.artifacts')
    if folder.exists():p.error('Use a fresh output path')
    env=isolated_env(folder/'workspace')
    write_json(folder/'workspace/preferences.json',dict(auto_notes=False,diarization=False,threads=1))
    errors=[];start=time.monotonic();report=dict(passed=False,errors=errors)
    try:
        with wave.open(str(a.wav)) as source:
            assert (source.getnchannels(),source.getsampwidth(),source.getframerate())==(1,2,16000)
            clip=source.readframes(source.getnframes())+b'\0'*(16000*2)
        total=a.minutes*60*16000*2
        repeated=folder/'repeated-public.wav'
        with wave.open(str(repeated),'wb') as target:
            target.setparams((1,2,16000,0,'NONE','not compressed'))
            remaining=total
            while remaining:
                part=clip[:remaining];target.writeframes(part);remaining-=len(part)
        snapshot=command([a.cli.resolve(),'import',repeated,'Public repeated-audio stress'],env,folder/'import',timeout=max(300,a.minutes*60))
        mid=snapshot['meeting']['id'];audio=folder/'workspace/meetings'/mid/'audio'
        with wave.open(str(folder/'workspace/meetings'/mid/'files/recording-001.wav')) as wav:
            recorded=wav.readframes(wav.getnframes())
        pcm=b''.join(p.read_bytes() for p in sorted(audio.glob('*.pcm')))
        if len(pcm)!=total or recorded!=pcm:errors.append('Duration or sample mismatch between archived PCM and WAV')
        segments=snapshot['segments'];previous=0
        for segment in segments:
            if not previous<=segment['start_ms']<=segment['end_ms']<=a.minutes*60000:errors.append('Timestamp discontinuity')
            previous=segment['end_ms']
        if len(segments)<a.minutes*60/(len(clip)/(2*16000))*.7:errors.append('Speech missing from repeated recording')
        if snapshot['meeting']['status']!='ready' or snapshot['runtime']['error']:errors.append('Recording did not finish cleanly')
        report.update(duration_minutes=a.minutes,segments=len(segments),asr_rtf=snapshot['runtime']['asr_rtf'],
            archived_pcm_sha256=hashlib.sha256(pcm).hexdigest(),archive_samples=len(pcm)//2,
            limitation='Repeated public sample; accelerated import, not a physical microphone or diverse 30-minute meeting.')
    except Exception as error:errors.append(f'{type(error).__name__}: {error}')
    report.update(passed=not errors,seconds=round(time.monotonic()-start,3))
    write_json(a.output,report);print(report,flush=True)
    raise SystemExit(0 if report['passed'] else 1)

if __name__=='__main__':main()
