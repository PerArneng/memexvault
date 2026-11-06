import json,sys,numpy as np
from importlib.machinery import SourceFileLoader
s=SourceFileLoader('score','tools/score.py').load_module()
from rapidfuzz.distance import Levenshtein
for l,n in [('en','en_scotus'),('sv','sv_librivox'),('de','de_librivox'),('es','es_librivox')]:
    a=json.load(open(f'out/gemini-real/{n}_10m.mp3.json'))['words']; b=[w for w in json.load(open(f'out/vad-real/{n}.mp3.json'))['words'] if w['s']<600000]
    a=[(s.norm(w['w']),w) for w in a];a=[x for x in a if x[0]];b=[(s.norm(w['w']),w) for w in b];b=[x for x in b if x[0]]
    d=[]
    for o in Levenshtein.opcodes([x for x,_ in a],[x for x,_ in b]):
        if o.tag=='equal' and o.src_end-o.src_start>=2:
            d+=[a[i][1]['s']-b[j][1]['s'] for i,j in zip(range(o.src_start,o.src_end),range(o.dest_start,o.dest_end))]
    d=np.array(d,float); print(n,'matched',len(d),'of',len(a),'abs med',np.median(abs(d)),'p90',np.percentile(abs(d),90),'p99',np.percentile(abs(d),99),'signed',np.median(d),'>1s %.1f'%(100*(abs(d)>1000).mean()))
