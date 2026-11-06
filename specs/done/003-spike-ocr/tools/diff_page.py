# /// script
# requires-python = ">=3.12"
# dependencies = ["rapidfuzz"]
# ///
import json,sys,re,unicodedata
from rapidfuzz.distance import Levenshtein as L
sys.path
cfg,stem=sys.argv[1:3]
rec=[json.loads(l) for l in open(f"out/ocr-{cfg}.jsonl")]
r=[x for x in rec if ("/"+stem+".") in x.get("path","")][0]
print(open(r["path"].rsplit(".",1)[0]+".txt").read()[:500]); print("-----")
print("\n".join(x["t"] for x in sorted(r["regions"],key=lambda x:x["poly"][0][1]))[:600]); print("-----")
truth=re.sub(r"[\s\-]+","",unicodedata.normalize("NFC",open(r["path"].rsplit(".",1)[0]+".txt").read()))
ocr=re.sub(r"[\s\-]+","","".join(x["t"] for x in sorted(r["regions"],key=lambda x:(round(x["poly"][0][1]/12),x["poly"][0][0]))))
ocr=unicodedata.normalize("NFC",ocr)
print(len(truth),len(ocr));n=0
for op in L.opcodes(truth,ocr):
    if op.tag!="equal" and n<30: print(op.tag,repr(truth[max(0,op.src_start-6):op.src_end+3]),"=>",repr(ocr[op.dest_start:op.dest_end])); n+=1
