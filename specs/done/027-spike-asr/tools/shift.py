import json,glob,numpy as np
from importlib.machinery import SourceFileLoader
s=SourceFileLoader('score','tools/score.py').load_module()
for f in sorted(glob.glob('out/vad/*.json'))+sorted(glob.glob('out/vad-long/*.json')):
    stem=f.split('/')[-1].split('.')[0]; t=json.load(open(f'corpus/synth/{stem}.json'))
    tw=[(s.norm(w['w']),w) for w in t['words']];tw=[x for x in tw if x[0]]
    h=[(s.norm(w['w']),w) for w in json.load(open(f))['words']];h=[x for x in h if x[0]]
    _,p,_e=s.align([a for a,_ in tw],[a for a,_ in h])
    d=np.array([h[j][1]['s']-tw[i][1]['s'] for i,j in p],float)
    print(f,'med',np.median(abs(d)),'p90',np.percentile(abs(d),90),'after -170: med',np.median(abs(d-170)),'p90',np.percentile(abs(d-170),90),'early>100ms %.0f%%'%(100*((d-170)<-100).mean()))
