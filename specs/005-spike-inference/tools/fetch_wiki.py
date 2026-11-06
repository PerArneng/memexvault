# /// script
# requires-python = ">=3.12"
# dependencies = ["requests"]
# ///
"""Spec 005: labelled retrieval set. For articles with pages in all four languages, the passage is the article's lead and the query is
Wikidata's short description in each language, with no article title. Query in language A must find the passage in language B. CC BY-SA text, not committed."""
import json, pathlib, re, time, requests
OUT = pathlib.Path(__file__).parent.parent / "corpus"; OUT.mkdir(exist_ok=True)
H = {"User-Agent": "memexvault-spec005/0.1 (research benchmark; low rate)"}; LANGS = ["en", "sv", "de", "es"]; WANT = 600
def get(url, **p):
    for _ in range(8):
        r = requests.get(url, params={"format": "json", **p}, headers=H, timeout=60)
        if r.status_code == 200: return r.json()
        wait = int(r.headers.get("Retry-After", 30)); print("wait", r.status_code, wait, url, flush=True); time.sleep(min(wait, 120))
    raise SystemExit(f"gave up: {r.status_code} {r.text[:200]}")
# PagePile 68383 is Meta's list of 10,000 articles every Wikipedia should have, as Wikidata items.
d = requests.get("https://pagepile.toolforge.org/api.php", params=dict(id=68383, action="get_data", format="json"), headers=H, timeout=120).json()
qids = sorted(set(re.findall(r"\bQ\d+\b", json.dumps(d)))); import random; random.Random(5).shuffle(qids); print("qids", len(qids), flush=True)
items = []; t_i = 0
while len(items) < WANT and t_i < len(qids):
    batch = qids[t_i:t_i + 50]; t_i += 50
    ents = get("https://www.wikidata.org/w/api.php", action="wbgetentities", ids="|".join(batch), props="sitelinks|descriptions", languages="|".join(LANGS), sitefilter="|".join(l + "wiki" for l in LANGS)).get("entities", {})
    for e in ents.values():
        sl, de = e.get("sitelinks", {}), e.get("descriptions", {})
        if all(l + "wiki" in sl and l in de for l in LANGS): items.append(dict(id=e["id"], titles={l: sl[l + "wiki"]["title"] for l in LANGS}, q={l: de[l]["value"] for l in LANGS}))
    time.sleep(.5)
print("items", len(items))
for l in LANGS:
    its = {i["titles"][l]: i for i in items}; names = list(its)
    for k in range(0, len(names), 20):
        d = get(f"https://{l}.wikipedia.org/w/api.php", action="query", prop="extracts", exintro=1, explaintext=1, exlimit=20, redirects=1, titles="|".join(names[k:k + 20]))
        for p in d["query"]["pages"].values():
            ext = re.sub(r"\s+", " ", p.get("extract", "")).strip()
            if p["title"] in its: its[p["title"]].setdefault("p", {})[l] = ext
        time.sleep(.3)
keep = [i for i in items if all(len(i.get("p", {}).get(l, "")) > 200 for l in LANGS) and all(len(i["q"][l]) >= 15 for l in LANGS)]
(OUT / "wiki.json").write_text(json.dumps(keep, ensure_ascii=False)); print("kept", len(keep), keep[:1] and keep[0]["q"])
for l in LANGS[:0]: print(l, keep[0]["titles"][l], "|", keep[0]["q"][l])
