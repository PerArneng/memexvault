# /// script
# requires-python = ">=3.12"
# dependencies = ["requests"]
# ///
"""Spec 003: fetch validated Wikisource page scans with their proofread text into ../corpus/real/<lang>/.
Public-domain scans. Text is the community's proofread transcription. Usage: uv run fetch_real.py"""
import json, pathlib, random, re, time, requests
OUT = pathlib.Path(__file__).parent.parent / "corpus/real"
HEAD = {"User-Agent": "memexvault-spec003/0.1 (research benchmark; low rate)"}
SITES = {"en": ("en", 104, 8), "sv": ("sv", 104, 8), "de": ("de", 102, 7), "es": ("es", 102, 7)}
def clean(t):
    t = re.sub(r"<noinclude>.*?</noinclude>", "", t, flags=re.S)
    t = re.sub(r"<ref.*?(</ref>|/>)", "", t, flags=re.S)
    t = re.sub(r"<[^>]+>", " ", t)
    return t
def plain(t): return not re.search(r"\{\{|\[\[|\{\||^:|^\*|<", t, flags=re.M)
def fetch(lang):
    site, ns, want = SITES[lang]; api = f"https://{site}.wikisource.org/w/api.php"; d = OUT / lang; d.mkdir(parents=True, exist_ok=True)
    got = len(list(d.glob("*.txt"))); tries = 0
    while got < want and tries < 200:
        tries += 1
        r = requests.get(api, params=dict(action="query", format="json", generator="random", grnnamespace=ns, grnlimit=50, prop="proofread|revisions|imageforpage", rvprop="content", rvslots="main", prppifpprop="fullsize|filename|size"), headers=HEAD, timeout=60)
        if r.status_code != 200: time.sleep(10); continue
        for p in r.json().get("query", {}).get("pages", {}).values():
            pr = p.get("proofread", {})
            if pr.get("quality", 0) < 3 or got >= want: continue
            txt = clean(p["revisions"][0]["slots"]["main"]["*"])
            if not plain(txt) or not (900 <= len(txt) <= 4500) or txt.count("\n") < 3: continue
            url = p.get("imageforpage", {}).get("fullsize") or p.get("imagesforpage", {}).get("fullsize")
            if not url: continue
            url = "https:" + url if url.startswith("//") else url
            img = requests.get(url, headers=HEAD, timeout=120)
            if img.status_code != 200 or len(img.content) < 30000: time.sleep(3); continue
            stem = d / f"{lang}_{got:02d}"
            ext = ".jpg" if img.content[:2] == b"\xff\xd8" else ".png"
            (stem.with_suffix(ext)).write_bytes(img.content)
            stem.with_suffix(".txt").write_text(txt.strip())
            stem.with_suffix(".json").write_text(json.dumps(dict(title=p["title"], site=site, quality=pr.get("quality"), url=url), ensure_ascii=False))
            print(lang, got, p["title"], pr.get("quality"), url[-50:], flush=True)
            got += 1; time.sleep(2)
        time.sleep(1)
    print(lang, "got", got, "tries", tries)
for l in SITES: fetch(l)
