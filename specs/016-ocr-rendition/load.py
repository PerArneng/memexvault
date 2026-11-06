"""Spec 016: object lookup p95 through the API, idle and with OCR running. Usage: load.py <base-url> <token> <vault> <source> <pdf-dir>"""
import http.client, json, statistics, sys, threading, time, urllib.parse
base, token, vault, source, pdfdir = sys.argv[1:6]
u = urllib.parse.urlparse(base); H = {"Authorization": f"Bearer {token}"}
def measure(seconds, conc=4):
    out = []; stop = time.time() + seconds
    def w():
        c = http.client.HTTPConnection(u.hostname, u.port)
        while time.time() < stop:
            t = time.perf_counter(); c.request("GET", f"/api/v1/vaults/{vault}/sources/{source}", headers=H); r = c.getresponse(); r.read(); assert r.status == 200
            out.append((time.perf_counter() - t) * 1e3); time.sleep(0.005)
    ts = [threading.Thread(target=w) for _ in range(conc)]; [t.start() for t in ts]; [t.join() for t in ts]
    out.sort(); return dict(n=len(out), p50=round(statistics.median(out), 2), p95=round(out[int(len(out) * .95)], 2))
def api(m, p, b=None):
    c = http.client.HTTPConnection(u.hostname, u.port); c.request(m, p, body=b, headers=H); r = c.getresponse(); return r.status, json.loads(r.read() or b"null")
idle = measure(15); print("idle", idle)
# Many distinct scanned documents: same page with bytes appended after the end, so each is new to dedup.
pdf = open(f"{pdfdir}/mixed_text_image.pdf", "rb").read()
for i in range(40):
    s, _ = api("POST", f"/api/v1/vaults/{vault}/sources?name=d{i}.pdf", pdf + b"\n%" + str(i).encode()); assert s == 201, s
def pending():
    _, d = api("GET", f"/api/v1/vaults/{vault}/jobs?state=queued&limit=1"); _, e = api("GET", f"/api/v1/vaults/{vault}/jobs?state=running&limit=1"); return bool(d["items"]), bool(e["items"])
time.sleep(1); print("queue", pending())
loaded = measure(40); print("during OCR", loaded, "pending", pending(), "ratio", round(loaded["p95"] / idle["p95"], 2))
_, d = api("GET", f"/api/v1/vaults/{vault}/jobs?state=done&limit=200"); print("jobs done so far", len(d["items"]))
