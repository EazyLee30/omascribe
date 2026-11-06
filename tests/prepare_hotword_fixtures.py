#!/usr/bin/env python3
"""Explicit preparation of four pinned upstream test clips; evaluation stays offline."""
import argparse
import hashlib
import json
import pathlib
import time
import urllib.request
import wave

REPO='csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20'
REVISION='98590b7ed6443e77b714204da2757d75e1a642f4'
FILES=[
    ('0.wav',321744,'7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b'),
    ('1.wav',163244,'8bfb42c963e623ebab31b81ff4404867d07d3102507c87ac14577c4c61663b8c'),
    ('2.wav',150124,'20805bcc696b9b65f3357a2508a419d4f33b81006d47e1312e219b0fad934d1d'),
    ('3.wav',282604,'5851e31f3e8e5f741635da9466e19fccfbabd117ebcdc92db242c92483aee064'),
]


def prepare(root):
    root.mkdir(parents=True,exist_ok=True)
    cases=[]
    for name,size,sha in FILES:
        path=root/name;url=f'https://huggingface.co/{REPO}/resolve/{REVISION}/test_wavs/{name}'
        valid=path.is_file() and path.stat().st_size==size and hashlib.sha256(path.read_bytes()).hexdigest()==sha
        if not valid:
            for attempt in range(3):
                try:
                    with urllib.request.urlopen(url,timeout=30) as response:data=response.read(size+1)
                    if len(data)!=size or hashlib.sha256(data).hexdigest()!=sha:raise ValueError('Public audio checksum mismatch')
                    part=path.with_suffix('.wav.part');part.write_bytes(data);part.replace(path)
                    break
                except Exception:
                    if attempt==2:raise
                    time.sleep(.25)
        with wave.open(str(path)) as audio:
            assert (audio.getnchannels(),audio.getsampwidth(),audio.getframerate())==(1,2,16000)
            cases.append(dict(file=name,url=url,sha256=sha,bytes=size,rate=audio.getframerate(),seconds=audio.getnframes()/audio.getframerate()))
    manifest=dict(schema=1,source_repo=REPO,revision=REVISION,selection='first four numbered public upstream test_wavs',cases=cases)
    (root/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(dict(prepared=len(cases),revision=REVISION,directory=str(root))))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--directory',type=pathlib.Path,required=True)
    prepare(parser.parse_args().directory.resolve())
