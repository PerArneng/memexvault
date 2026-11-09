import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Locator, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";
import { fiftyResults } from "./fixture";
import { isLatin, makeTextPdf } from "./golden-pdf";

// Spec 043, on a real server with the real CSP and models. Item 1's response alone is a fixture, as that item says, and a few states
// no server can be made to reach on demand are replaced at the network, each saying so.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const CORPUS = process.env.MEMEX_E2E_CORPUS;

let vault = "";

const api = (path: string) => `/api/v1/vaults/${vault}${path}`;
const searchUrl = (query = "") => `/vaults/${vault}/search${query}`;
const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load averages:")[1]?.trim() ?? "unknown";

const post = async <T>(request: APIRequestContext, path: string, data: unknown, status = 201): Promise<T> => {
  const res = await request.post(path, { headers: bearer, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const get = async <T>(request: APIRequestContext, path: string): Promise<T> => {
  const res = await request.get(path, { headers: bearer });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};

interface Source { readonly id: string; readonly name: string }
interface Segment { readonly id: string; readonly text: string; readonly page_from?: number | null; readonly page_to?: number | null }
interface Word { readonly text: string; readonly page: number; readonly rect: { readonly x0: number; readonly y0: number; readonly x1: number; readonly y1: number } }

const upload = async (request: APIRequestContext, name: string, data: Buffer | string): Promise<Source> => {
  const res = await request.post(api(`/sources?name=${encodeURIComponent(name)}`), { headers: { ...bearer, "content-type": "application/octet-stream" }, data });
  expect(res.status(), `${name}: ${await res.text()}`).toBeLessThan(300);
  return ((await res.json()) as { source: Source }).source;
};

const renditionOf = async (request: APIRequestContext, source: string, minutes = 15): Promise<string> => {
  const deadline = Date.now() + minutes * 60_000;
  for (;;) {
    const { items } = await get<{ items: { id: string; superseded_by?: string | null }[] }>(request, api(`/sources/${source}/renditions`));
    const current = items.find((r) => !r.superseded_by);
    if (current !== undefined) return current.id;
    if (Date.now() > deadline) throw new Error(`no rendition for ${source}`);
    await new Promise((done) => setTimeout(done, 500));
  }
};

const segmentsOf = async (request: APIRequestContext, source: string): Promise<readonly Segment[]> => {
  const rendition = await renditionOf(request, source);
  return (await get<{ items: Segment[] }>(request, api(`/renditions/${rendition}/segments?limit=200`))).items;
};

const normal = (text: string) => text.split(/\s+/).filter((w) => w !== "").join(" ");

const addClaim = async (request: APIRequestContext, id: string, segment: string, subject: string, predicate: string, value: string, quote: string) => {
  const answer = await post<{ state?: string }>(request, api("/proposals"), {
    idempotency_key: `${id}-key`,
    rationale: "the search screen's checks",
    ops: [{ op: "add_assertion", id, subject, predicate, value, epistemic: "asserted", machine: false, confidence_permille: 900, valid_from_ms: null, valid_to_ms: null, evidence: [[`${id}-e`, { segment_id: segment, quote, stance: "supports" }]] }],
  }, 200);
  return answer;
};

const signIn = async (page: Page) => {
  const res = await page.context().request.post("/api/v1/session", { headers: { "x-memex-csrf": "1" }, data: { token: adminToken } });
  expect(res.ok()).toBe(true);
};

const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") problems.push(m.text());
  });
  return problems;
};
const cspProblems = (problems: readonly string[]) => problems.filter((p) => /content security policy/i.test(p));

const box = (page: Page) => page.getByLabel("Search this vault");
const results = (page: Page, q?: string): Locator => page.locator(q === undefined ? "[data-search-results]:not([data-search-stale])" : `[data-search-results]:not([data-search-stale])[data-search-query="${q.replace(/"/g, '\\"')}"]`);

/** Search through the API until it finds what's wanted, as a new document's words take a moment to reach the indexes. */
const untilFound = async (request: APIRequestContext, q: string, found: (body: { items: { id: string; kind: string; text: string; citations: { source_id?: string | null }[] }[]; debug?: { signals: { id: string; vector_rank?: number | null }[] } }) => boolean, minutes = 15) => {
  const deadline = Date.now() + minutes * 60_000;
  for (;;) {
    const res = await request.get(api(`/search?q=${encodeURIComponent(q)}&limit=50&budget_tokens=20000&debug=true`), { headers: bearer });
    if (res.status() === 200) {
      const body = await res.json();
      if (found(body)) return body as { items: { id: string }[]; debug?: { signals: { id: string; vector_rank?: number | null }[] }; degraded: string[] };
    }
    if (Date.now() > deadline) throw new Error(`never found: ${q}`);
    await new Promise((done) => setTimeout(done, 1000));
  }
};

interface Language { readonly language: string; readonly q: string; readonly text: string; readonly terms: readonly string[]; readonly quote: string }

// Written for this test. What each must keep is in `quote` and `terms`.
const LANGUAGES: readonly Language[] = [
  { language: "en", q: "naïve résumé", text: "The naïve café owner's résumé was quoted in “Pride & Prejudice” with a 100% ‘exact’ citation.", terms: ["naïve", "résumé"], quote: "naïve café owner's résumé" },
  { language: "sv", q: "räksmörgås blåbär", text: "På Ångermanälven bjöd Åsa på räksmörgås och blåbär, och öländska gäster åt för två.", terms: ["räksmörgås", "blåbär"], quote: "räksmörgås och blåbär" },
  { language: "de", q: "Straße Grüße", text: "Die Straße über den Fluß führt zur Bäckerei in München, wo Grüße und Brötchen warten.", terms: ["Straße", "Grüße"], quote: "Straße über den Fluß" },
  { language: "es", q: "niño montaña", text: "El niño de España comió piñas en la montaña mientras soñaba con el año nuevo.", terms: ["niño", "montaña"], quote: "niño de España comió piñas" },
];

const HOSTILE_QUERY = '<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script> zorblaxian';
const HOSTILE_TEXT = 'The zorblaxian report said <b>bold</b> and <img src=x onerror="window.__pwned=3"> and <script>window.__pwned=4</script> plainly.';

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  console.log(`server started in ${String(await startServer())} ms`);
  vault = (await post<{ id: string }>(request, "/api/v1/vaults", { name: `search-${run}` })).id;
  for (const l of LANGUAGES) {
    const source = await upload(request, `språk ${l.language}.txt`, l.text);
    const [segment] = await segmentsOf(request, source.id);
    if (segment === undefined) throw new Error("no segment");
    await addClaim(request, `lang-${l.language}-${run}`, segment.id, l.q, "says", l.text, l.quote);
    await untilFound(request, l.q, (b) => b.items.some((i) => i.kind === "segment" && i.citations.some((c) => c.source_id === source.id)));
  }
  await upload(request, "hostile <b>name</b>.txt", HOSTILE_TEXT);
  await untilFound(request, "zorblaxian", (b) => b.items.some((i) => i.kind === "segment"));
});

test.afterAll(async () => {
  await stopServer();
});

test("item 1: from a response of 50 results to their first paint", async ({ page }) => {
  test.setTimeout(600_000);
  const body = JSON.stringify(fiftyResults());
  await signIn(page);
  await page.route("**/api/v1/vaults/*/search?*", (route) => route.fulfill({ status: 200, contentType: "application/json", body }));
  await page.goto(searchUrl());
  await expect(box(page)).toBeVisible();
  const WARM = 5;
  const COUNTED = 50;
  const before = loadAverage();
  // All of it runs in the page, as Playwright's own checks use the page's main thread, and on an earlier version of this test they were most of the script running while it was timed.
  const samples = await page.evaluate(async ({ total }) => {
    const input = document.getElementById("search-box") as HTMLInputElement;
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    const out: { added: number; commit: number; toFrame: number; layout: number; items: number }[] = [];
    for (let n = 0; n < total; n += 1) {
      const q = `measured river ${String(n)}`;
      const painted = new Promise<{ committed: number; frame: number; painted: number; items: number }>((done) => {
        const observer = new MutationObserver(() => {
          const element = document.querySelector("[data-search-results]:not([data-search-stale])");
          if (element?.getAttribute("data-search-query") !== q) return;
          observer.disconnect();
          const committed = performance.now();
          requestAnimationFrame(() => {
            const frame = performance.now();
            const channel = new MessageChannel();
            channel.port1.onmessage = () => done({ committed, frame, painted: performance.now(), items: element.querySelectorAll("[data-item-id]").length });
            channel.port2.postMessage(0);
          });
        });
        observer.observe(document, { subtree: true, childList: true, attributes: true });
      });
      setValue?.call(input, q);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.form?.requestSubmit();
      const paint = await painted;
      const entry = performance.getEntriesByType("resource").filter((e) => e.name.includes("/search?") && decodeURIComponent(e.name.replace(/\+/g, " ")).includes(q)).at(-1) as PerformanceResourceTiming | undefined;
      if (entry === undefined) throw new Error(`no timing for ${q}`);
      out.push({ added: paint.painted - entry.responseEnd, commit: paint.committed - entry.responseEnd, toFrame: paint.frame - paint.committed, layout: paint.painted - paint.frame, items: paint.items });
      // A pause, as a person's next search doesn't come in the same frame.
      await new Promise((done) => setTimeout(done, 100));
    }
    return out;
  }, { total: WARM + COUNTED });
  const counted = samples.slice(WARM);
  for (const s of counted) expect(s.items).toBe(50);
  const pct = (values: readonly number[], p: number) => [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil((p / 100) * values.length) - 1)] ?? 0;
  const round = (n: number) => Number(n.toFixed(1));
  const of = (pick: (s: (typeof counted)[number]) => number) => counted.map(pick);
  const p95 = pct(of((s) => s.added), 95);
  record("item 1: added ms", { samples: counted.length, p50: round(pct(of((s) => s.added), 50)), p95: round(p95), max: round(Math.max(...of((s) => s.added))), commitP50: round(pct(of((s) => s.commit), 50)), commitP95: round(pct(of((s) => s.commit), 95)), toFrameP50: round(pct(of((s) => s.toFrame), 50)), layoutPaintP50: round(pct(of((s) => s.layout), 50)), bodyBytes: body.length, loadBefore: before, loadAfter: loadAverage() });
  expect(p95).toBeLessThanOrEqual(50);
});

test("item 2: 20 golden results each open the right region", async ({ page, request }) => {
  test.skip(CORPUS === undefined, "not run: MEMEX_E2E_CORPUS isn't set");
  test.setTimeout(3_000_000);
  const lines = readFileSync(CORPUS ?? "", "utf8").split("\n");
  const golden = readFileSync(join(repo, "crates/memexvault/tests/fixtures/golden/golden.jsonl"), "utf8").split("\n").filter((l) => l !== "").map((l) => JSON.parse(l) as { id: string; question: string; language: string; answer?: string; line?: number });
  const chosen = ["en", "sv", "de", "es"].flatMap((language) => golden.filter((g) => g.language === language && g.line !== undefined && g.answer !== undefined && (lines[g.line] ?? "").includes(g.answer)).slice(0, 5));
  expect(chosen).toHaveLength(20);

  const filler = (from: number, words: number): string => {
    const out: string[] = [];
    let count = 0;
    for (let i = from; count < words && i < lines.length; i += 1) {
      const text = lines[i] ?? "";
      if (!isLatin(text)) continue;
      out.push(text);
      count += text.split(/\s+/).length;
    }
    return out.join(" ");
  };

  interface Case { readonly id: string; readonly question: string; readonly answer: string; readonly passage: string; readonly kind: "pdf_segment" | "pdf_claim" | "text"; readonly source: Source; readonly claim: string | null }
  const cases: Case[] = [];
  for (const [index, g] of chosen.entries()) {
    const line = g.line ?? 0;
    const passage = lines[line] ?? "";
    const answer = g.answer ?? "";
    const wanted = (["pdf_segment", "pdf_claim", "text"] as const)[index % 3] ?? "text";
    const kind = wanted !== "text" && !isLatin(passage) ? "text" : wanted;
    const name = `golden ${g.id} ${run}${kind === "text" ? ".txt" : ".pdf"}`;
    const source = kind === "text"
      ? await upload(request, name, `${lines[line + 1] ?? ""}\n\n${passage}\n\n${lines[line + 2] ?? ""}\n`)
      : await upload(request, name, makeTextPdf([filler(line + 1, 170), passage, filler(line + 40, 170)]));
    cases.push({ id: g.id, question: g.question, answer, passage, kind, source, claim: null });
  }
  const startedWaiting = Date.now();
  for (const [i, c] of cases.entries()) {
    const segments = await segmentsOf(request, c.source.id);
    if (c.kind === "pdf_claim") {
      const segment = segments.find((s) => normal(s.text).includes(normal(c.answer)) && normal(s.text).includes(normal(c.passage)));
      if (segment === undefined) throw new Error(`${c.id}: no segment holds its passage`);
      const claim = `golden-claim-${c.id}-${run}`;
      await addClaim(request, claim, segment.id, c.question, "is answered by", c.answer, c.answer);
      cases[i] = { ...c, claim };
    }
  }
  let withVector = 0;
  for (const c of cases) {
    const body = await untilFound(request, c.question, (b) => {
      const hit = c.claim !== null ? b.items.find((x) => x.id === c.claim) : b.items.find((x) => x.citations.some((s) => s.source_id === c.source.id) && normal(x.text).includes(normal(c.answer)));
      return hit !== undefined;
    });
    const id = c.claim ?? body.items.find((x) => (x as { citations?: { source_id?: string | null }[] }).citations?.some((s) => s.source_id === c.source.id))?.id;
    if (body.debug?.signals.find((s) => s.id === id)?.vector_rank !== null && body.debug?.signals.find((s) => s.id === id)?.vector_rank !== undefined) withVector += 1;
  }
  record("item 2: ready", { seconds: Math.round((Date.now() - startedWaiting) / 1000), withVectorRank: withVector, load: loadAverage() });

  await signIn(page);
  const problems = problemsOf(page);
  const outcomes: Record<string, unknown>[] = [];
  for (const c of cases) {
    await page.goto(searchUrl(`?q=${encodeURIComponent(c.question)}&limit=50`));
    await expect(results(page, c.question)).toBeVisible();
    const article = c.claim !== null
      ? results(page).locator(`article[data-item-id="${c.claim}"]`)
      : results(page).locator("article[data-item-kind='segment']").filter({ hasText: normal(c.answer) }).filter({ has: page.locator(`a[data-link-state="citation"]`, { hasText: c.source.name }) }).first();
    await expect(article, `${c.id}: its result isn't on screen`).toBeVisible();
    await article.locator('a[data-link-state="citation"]').first().click();
    await expect(page).toHaveURL(new RegExp(`/documents/${c.source.id}\\?`));
    if (c.kind === "text") {
      const cited = page.locator("[data-segment-cited]");
      await expect(cited).toBeVisible();
      await expect(cited).toBeInViewport();
      expect(normal((await cited.textContent()) ?? "")).toContain(normal(c.answer));
      outcomes.push({ id: c.id, kind: c.kind, ok: true, onScreen: true });
    } else {
      await expect(page.locator("[data-highlight]").first()).toBeVisible();
      await expect(page.locator("[data-highlight]").first()).toBeInViewport();
      // The passage is on page 2, and its link says so.
      expect(new URL(page.url()).searchParams.get("page"), `${c.id}: the cited page`).toBe("2");
      const rendition = await renditionOf(request, c.source.id);
      const table = await get<{ word_table: { words: Word[] } }>(request, api(`/renditions/${rendition}`));
      // The answer's words, wherever they sit on the passage's page, the second.
      const words = table.word_table.words.filter((w) => w.page === 1);
      const flat = words.map((w) => w.text).join("");
      const wanted = normal(c.answer).replace(/ /g, "");
      const at = flat.indexOf(wanted);
      expect(at, `${c.id}: the answer isn't on page 2`).toBeGreaterThanOrEqual(0);
      const covered: Word[] = [];
      let offset = 0;
      for (const w of words) {
        if (offset + w.text.length > at && offset < at + wanted.length) covered.push(w);
        offset += w.text.length;
      }
      const cell = page.locator('[data-page-index="1"]');
      await expect(cell.locator("[data-highlight]").first()).toBeAttached();
      await cell.scrollIntoViewIfNeeded();
      const cellBox = await cell.boundingBox();
      const boxes = await cell.locator("[data-highlight]").evaluateAll((all) => all.map((el) => {
        const b = el.getBoundingClientRect();
        return { left: b.left, top: b.top, right: b.right, bottom: b.bottom };
      }));
      if (cellBox === null) throw new Error("no page cell");
      const TOLERANCE = 2;
      const shown = covered.map((w) => ({ left: cellBox.x + w.rect.x0 / 100, top: cellBox.y + w.rect.y0 / 100, right: cellBox.x + w.rect.x1 / 100, bottom: cellBox.y + w.rect.y1 / 100 }));
      const inside = shown.every((w) => boxes.some((b) => w.left >= b.left - TOLERANCE && w.right <= b.right + TOLERANCE && w.top >= b.top - TOLERANCE && w.bottom <= b.bottom + TOLERANCE));
      expect(inside, `${c.id}: the answer's words aren't all highlighted`).toBe(true);
      const sides = (list: readonly { left: number; top: number; right: number; bottom: number }[]) => ({ left: Math.min(...list.map((b) => b.left)), top: Math.min(...list.map((b) => b.top)), right: Math.max(...list.map((b) => b.right)), bottom: Math.max(...list.map((b) => b.bottom)) });
      let worst: number | null = null;
      if (c.kind === "pdf_claim") {
        const a = sides(shown);
        const b = sides(boxes);
        worst = Math.max(Math.abs(a.left - b.left), Math.abs(a.top - b.top), Math.abs(a.right - b.right), Math.abs(a.bottom - b.bottom));
        expect(worst, `${c.id}: the claim's highlight isn't the answer's outline`).toBeLessThanOrEqual(TOLERANCE);
      } else {
        await expect(page.locator('[data-segment-state="located"]')).toBeVisible();
      }
      outcomes.push({ id: c.id, kind: c.kind, ok: true, words: covered.length, highlights: boxes.length, worstSidePx: worst === null ? null : Number(worst.toFixed(2)) });
    }
  }
  record("item 2: outcomes", outcomes);
  record("item 2: load", loadAverage());
  expect(outcomes).toHaveLength(20);
  expect(cspProblems(problems)).toEqual([]);
});

test("item 3: two conflicting claims show both values with their sources", async ({ page, request }) => {
  const a = await upload(request, `almanac ${run}.txt`, "The Quillfeather bridge opened in 1911, according to the almanac.");
  const b = await upload(request, `city records ${run}.txt`, "City records state the Quillfeather bridge opened in 1923.");
  const [sa] = await segmentsOf(request, a.id);
  const [sb] = await segmentsOf(request, b.id);
  if (sa === undefined || sb === undefined) throw new Error("no segments");
  const [ca, cb] = [`quill-a-${run}`, `quill-b-${run}`];
  await addClaim(request, ca, sa.id, "Quillfeather bridge", "opened in", "1911", "opened in 1911");
  await addClaim(request, cb, sb.id, "Quillfeather bridge", "opened in", "1923", "opened in 1923");
  await untilFound(request, "Quillfeather bridge", (r) => [ca, cb].every((id) => r.items.some((i) => i.id === id)));
  await signIn(page);
  const problems = problemsOf(page);
  await page.goto(searchUrl("?q=Quillfeather%20bridge&kind=claim"));
  await expect(results(page)).toBeVisible();
  for (const [own, ownValue, ownSource, rival, rivalValue, rivalSource] of [[ca, "1911", a, cb, "1923", b], [cb, "1923", b, ca, "1911", a]] as const) {
    const article = results(page).locator(`article[data-item-id="${own}"]`);
    await expect(article.locator("[data-item-text]")).toContainText(ownValue);
    await expect(article.locator('a[data-link-state="citation"]')).toContainText(ownSource.name);
    const conflict = article.locator(`[data-conflict="${rival}"]`);
    await expect(conflict.locator("[data-conflict-text]")).toContainText(rivalValue);
    await expect(conflict.getByRole("link", { name: rivalSource.name })).toBeVisible();
    await expect(article.getByText("1 conflict")).toBeVisible();
  }
  record("item 3", { claims: [ca, cb], sources: [a.name, b.name] });
  // Each source opens from the conflict.
  await results(page).locator(`article[data-item-id="${ca}"] [data-conflict="${cb}"]`).getByRole("link", { name: b.name }).click();
  await expect(page).toHaveURL(new RegExp(`/documents/${b.id}$`));
  await expect(page.getByRole("heading", { name: b.name })).toBeVisible();
  expect(cspProblems(problems)).toEqual([]);
});

test("item 4: four languages render, and quotes keep their exact text", async ({ page }) => {
  await signIn(page);
  const problems = problemsOf(page);
  for (const l of LANGUAGES) {
    await page.goto(searchUrl());
    const answer = page.waitForResponse((r) => r.url().includes("/search?") && r.status() === 200);
    await box(page).pressSequentially(l.q, { delay: 5 });
    const wire = (await (await answer).json()) as { items: { id: string; kind: string; text: string; citations: { quote?: string | null }[] }[] };
    await expect(results(page, l.q)).toBeVisible();
    expect(new URL(page.url()).searchParams.get("q")).toBe(l.q);

    const segment = results(page).locator("article[data-item-kind='segment']").filter({ hasText: l.terms[0] ?? "" }).first();
    const text = await segment.locator("[data-item-text]").textContent();
    expect(text, `${l.language}: a passage's text`).toBe(l.text);
    expect(wire.items.some((i) => i.kind === "segment" && i.text === text)).toBe(true);
    for (const term of l.terms) await expect(segment.locator("mark").filter({ hasText: new RegExp(`^${term}$`) }).first()).toBeVisible();

    const claim = results(page).locator(`article[data-item-id="lang-${l.language}-${run}"]`);
    await expect(claim).toBeVisible();
    const quote = await claim.locator("[data-quote]").textContent();
    expect(quote, `${l.language}: a claim's quote`).toBe(l.quote);
    expect(await claim.locator("[data-item-text]").textContent(), `${l.language}: a claim's text`).toBe(wire.items.find((i) => i.id === `lang-${l.language}-${run}`)?.text);

    // A link holds it as well, and the viewer shows the same text.
    await page.reload();
    await expect(box(page)).toHaveValue(l.q);
    await expect(results(page, l.q)).toBeVisible();
    await results(page).locator("article[data-item-kind='segment']").filter({ hasText: l.terms[0] ?? "" }).first().locator('a[data-link-state="citation"]').click();
    const cited = page.locator("[data-segment-cited]");
    await expect(cited).toBeVisible();
    expect(await cited.locator("p").textContent()).toBe(l.text);
    await page.goBack();
    await expect(box(page)).toHaveValue(l.q);
    await results(page).locator(`article[data-item-id="lang-${l.language}-${run}"] a[data-link-state="citation"]`).click();
    await expect(page.locator("[data-segment-cited] mark")).toHaveText(l.quote);
    record(`item 4: ${l.language}`, { q: l.q, chars: l.text.length, quote: l.quote });
  }
  expect(cspProblems(problems)).toEqual([]);
});

test("item 6: an empty query sends nothing, and says what to do", async ({ page }) => {
  await signIn(page);
  const searches: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/search?")) searches.push(r.url());
  });
  await page.goto(searchUrl());
  await expect(page.locator("[data-search-empty]")).toBeVisible();
  await box(page).fill("     ");
  await box(page).press("Enter");
  // Enter again, on what's already in the URL, is the case with nothing to run again.
  await box(page).press("Enter");
  await page.waitForTimeout(600);
  await expect(page.locator("[data-search-empty]")).toBeVisible();
  expect(searches).toEqual([]);
  await page.goto(searchUrl("?q="));
  await expect(page.locator("[data-search-empty]")).toBeVisible();
  // Clearing the box goes back to the same state.
  await page.goto(searchUrl("?q=zorblaxian"));
  await expect(results(page)).toBeVisible();
  await box(page).fill("");
  await expect(page.locator("[data-search-empty]")).toBeVisible();
  expect(new URL(page.url()).search).toBe("");
});

test("item 6: nothing relevant says so", async ({ page }) => {
  await signIn(page);
  await page.goto(searchUrl("?q=xqzjvkwpqz%20qqqvvvkkkzzz"));
  await expect(page.locator("[data-nothing-relevant]")).toBeVisible();
  await expect(page.locator("[data-nothing-relevant]")).toContainText("Nothing relevant was found.");
  // No result at all, as a filter leaves none, and the way out of the filter.
  await page.goto(searchUrl("?q=zorblaxian&tier=verified"));
  const none = page.locator("[data-nothing-relevant]");
  await expect(none).toContainText("Nothing relevant was found.");
  await none.getByRole("link", { name: "Search without them" }).click();
  await expect(results(page).locator("article").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("tier")).toBeNull();
});

test("item 6: a min_seq the log hasn't reached", async ({ page }) => {
  await signIn(page);
  await page.goto(searchUrl("?q=zorblaxian&min_seq=999999999"));
  const problem = page.locator('[data-search-problem="min_seq_ahead"]');
  await expect(problem).toContainText("event 999999999");
  await expect(problem).toContainText("request id");
  await problem.getByRole("link", { name: "Search without waiting" }).click();
  await expect(results(page).locator("article").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("min_seq")).toBeNull();
});

test("item 6: a min_seq the indexes haven't reached, replaced at the network, as a real one lasts only a moment", async ({ page }) => {
  await signIn(page);
  let refused = 0;
  await page.route("**/api/v1/vaults/*/search?*", async (route) => {
    if (refused === 0) {
      refused += 1;
      await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { code: "not_caught_up", message: "the indexes haven't reached that event", request_id: "r1" } }) });
    } else await route.continue();
  });
  await page.goto(searchUrl("?q=zorblaxian&min_seq=1"));
  const problem = page.locator('[data-search-problem="not_caught_up"]');
  await expect(problem).toBeVisible();
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(results(page).locator("article").first()).toBeVisible();
});

test("item 6: an unreadable cursor and one past the end", async ({ page }) => {
  await signIn(page);
  // The API's cursors hold a rank and can't expire, so these are what a stale link can really meet. See the spec's Result.
  await page.goto(searchUrl("?q=zorblaxian&cursor=not-a-cursor"));
  const problem = page.locator('[data-search-problem="bad_cursor"]');
  await expect(problem).toBeVisible();
  await problem.getByRole("link", { name: "Go to the first page" }).click();
  await expect(results(page).locator("article").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("cursor")).toBeNull();

  await page.goto(searchUrl("?q=zorblaxian&cursor=r999999"));
  const none = page.locator("[data-no-more]");
  await expect(none).toBeVisible();
  await none.getByRole("link", { name: "Go to the first page" }).click();
  await expect(results(page).locator("article").first()).toBeVisible();
});

test("item 6: HTML in a query and in a document stays text", async ({ page }) => {
  await signIn(page);
  const problems = problemsOf(page);
  const requests: string[] = [];
  page.on("request", (r) => requests.push(r.url()));
  await page.goto(searchUrl(`?q=${encodeURIComponent(HOSTILE_QUERY)}`));
  await expect(box(page)).toHaveValue(HOSTILE_QUERY);
  await expect(results(page)).toBeVisible();
  const passage = results(page).locator("article[data-item-kind='segment']").filter({ hasText: "plainly" }).first();
  expect(await passage.locator("[data-item-text]").textContent()).toBe(HOSTILE_TEXT);
  await expect(passage.locator("[data-item-text] b, [data-item-text] img, [data-item-text] script")).toHaveCount(0);
  await expect(passage.locator('a[data-link-state="citation"]')).toContainText("<b>name</b>");
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  expect(requests.filter((r) => /\/x($|\?)/.test(new URL(r).pathname))).toEqual([]);
  // Typing it behaves the same as linking to it.
  await box(page).fill("");
  await box(page).fill(HOSTILE_QUERY);
  await expect(results(page).locator("article").first()).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  expect(cspProblems(problems)).toEqual([]);
});

test("item 6: a broken link says what it left out, and the server being out of reach says that", async ({ page }) => {
  await signIn(page);
  await page.goto(searchUrl("?q=zorblaxian&kind=wormhole&as_of=soon&limit=9000&rerank=maybe"));
  const note = page.locator("[data-search-ignored]");
  await expect(note).toHaveAttribute("data-search-ignored", "kind,as_of,rerank,limit");
  await expect(results(page).locator("article").first()).toBeVisible();
  await note.getByRole("link", { name: "Use the link without them" }).click();
  await expect(note).toHaveCount(0);

  await page.goto(searchUrl("?q=%E0%A4%A"));
  await expect(box(page)).toBeVisible();

  await page.route("**/api/v1/vaults/*/search?*", (route) => route.abort("connectionrefused"));
  await page.goto(searchUrl("?q=unreachable"));
  await expect(page.getByText("Can't load search results while the server is out of reach.")).toBeVisible();
});

test("item 7: rapid typing runs one search, for the last text", async ({ page }) => {
  await signIn(page);
  await page.goto(searchUrl());
  const asked: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/search?")) asked.push(new URL(r.url()).searchParams.get("q") ?? "");
  });
  await box(page).pressSequentially("zorblaxian", { delay: 20 });
  await expect(results(page, "zorblaxian")).toBeVisible();
  record("item 7: requests while typing 10 characters 20 ms apart", { asked, load: loadAverage() });
  expect(asked.at(-1)).toBe("zorblaxian");
  expect(asked.length).toBeLessThanOrEqual(2);
});

test("item 7: slower requests are cancelled, and a late answer never shows", async ({ page }) => {
  await signIn(page);
  // Each of the first two answers takes far longer than the last, so without cancelling they would land after it.
  const held: { q: string; release: () => void; route: Parameters<Parameters<Page["route"]>[1]>[0] }[] = [];
  const aborted: string[] = [];
  const fulfilled: string[] = [];
  page.on("requestfailed", (r) => {
    if (r.url().includes("/search?")) aborted.push(`${new URL(r.url()).searchParams.get("q") ?? ""}: ${r.failure()?.errorText ?? ""}`);
  });
  await page.route("**/api/v1/vaults/*/search?*", async (route) => {
    const q = new URL(route.request().url()).searchParams.get("q") ?? "";
    if (q === "zorblaxian") {
      fulfilled.push(q);
      await route.continue();
      return;
    }
    await new Promise<void>((release) => held.push({ q, release, route }));
    try {
      const response = await route.fetch();
      fulfilled.push(q);
      await route.fulfill({ response });
    } catch {
      // The page has gone from this request, so there's nowhere to answer.
    }
  });
  await page.addInitScript(() => {
    const seen: string[] = [];
    (window as unknown as { __shown: string[] }).__shown = seen;
    new MutationObserver(() => {
      for (const el of document.querySelectorAll("[data-search-results]")) {
        const q = el.getAttribute("data-search-query") ?? "";
        if (seen.at(-1) !== q) seen.push(q);
      }
    }).observe(document, { subtree: true, childList: true, attributes: true });
  });
  await page.goto(searchUrl());
  await box(page).fill("quillfeather");
  await expect.poll(() => held.length).toBe(1);
  await box(page).fill("räksmörgås");
  await expect.poll(() => held.length).toBe(2);
  await box(page).fill("zorblaxian");
  await expect(results(page, "zorblaxian")).toBeVisible();
  await expect.poll(() => aborted.length).toBe(2);
  // Now the earlier answers are let go, and nothing may change.
  for (const h of held) h.release();
  await page.waitForTimeout(1500);
  await expect(results(page, "zorblaxian")).toBeVisible();
  const shown = await page.evaluate(() => (window as unknown as { __shown: string[] }).__shown);
  record("item 7: late answers", { aborted, shown, fulfilledAfterRelease: fulfilled, load: loadAverage() });
  expect(shown).toEqual(["zorblaxian"]);
  expect(aborted.every((a) => a.includes("ERR_ABORTED"))).toBe(true);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText("Can't load")).toHaveCount(0);
});

test("the URL holds the state: filters, facets, as_of, debug and rerank", async ({ page, request }) => {
  test.setTimeout(300_000);
  await signIn(page);
  const problems = problemsOf(page);
  await page.goto(searchUrl("?q=zorblaxian"));
  await expect(results(page).locator("article").first()).toBeVisible();

  // A filter, with its facet's count, goes in the URL and survives a reload.
  const kind = page.locator('[data-filter="Kind:segment"]');
  await expect(kind).toContainText(/\d/);
  await kind.click();
  await expect(page).toHaveURL(/kind=segment/);
  await expect(kind).toHaveAttribute("aria-pressed", "true");
  await page.reload();
  await expect(page.locator('[data-filter="Kind:segment"]')).toHaveAttribute("aria-pressed", "true");
  await expect(results(page).locator("article[data-item-kind='claim']")).toHaveCount(0);
  await page.goBack();
  await expect(page).not.toHaveURL(/kind=segment/);

  // Debug: per-signal scores and the planner's path, with an item's id as a row.
  await page.goto(searchUrl("?q=zorblaxian"));
  await page.getByLabel("Show debug details").check();
  await expect(page).toHaveURL(/debug=1/);
  const panel = page.locator("[data-debug-panel]");
  await expect(panel.locator("[data-debug-path]")).toHaveText(/natural|identifier|exact/);
  await expect(panel.locator("[data-signal]").first()).toBeVisible();

  // Rerank on the real model, whose score appears in the same table.
  await page.getByLabel("Rerank the top 20").check();
  await expect(page).toHaveURL(/rerank=1/);
  await expect(panel.locator("[data-signal]").first()).toBeVisible({ timeout: 120_000 });
  await expect.poll(async () => (await panel.locator("[data-signal] td").nth(6).textContent()) ?? "-", { timeout: 120_000 }).not.toBe("-");
  record("rerank", { score: await panel.locator("[data-signal] td").nth(6).textContent(), load: loadAverage() });

  // as_of: a claim added after that event isn't there, and is back in the present.
  const head = (await get<{ head?: number | null }>(request, api("/activity/documents"))).head ?? 0;
  const source = await upload(request, `past ${run}.txt`, "The Wintermoss archive records one late addition.");
  const [segment] = await segmentsOf(request, source.id);
  if (segment === undefined) throw new Error("no segment");
  const claim = `past-${run}`;
  await addClaim(request, claim, segment.id, "Wintermoss archive", "records", "one late addition", "one late addition");
  await untilFound(request, "Wintermoss archive", (r) => r.items.some((i) => i.id === claim));
  await page.goto(searchUrl("?q=Wintermoss%20archive"));
  await expect(results(page).locator(`article[data-item-id="${claim}"]`)).toBeVisible();
  await page.getByLabel("As of (an event number, or a date)").fill(String(head));
  await page.getByLabel("As of (an event number, or a date)").press("Enter");
  await expect(page).toHaveURL(new RegExp(`as_of=${String(head)}`));
  await expect(results(page).getByText(`how the vault stood at event ${String(head)}`)).toBeVisible();
  await expect(results(page).locator(`article[data-item-id="${claim}"]`)).toHaveCount(0);
  await expect(page.getByLabel("Rerank the top 20")).toBeDisabled();
  await results(page).getByRole("link", { name: "Back to the present" }).click();
  await expect(results(page).locator(`article[data-item-id="${claim}"]`)).toBeVisible();
  expect(new URL(page.url()).searchParams.get("as_of")).toBeNull();

  // A bad date stays in its field.
  await page.getByLabel("Valid at").fill("not a date");
  await page.getByLabel("Valid at").press("Enter");
  await expect(page.getByText("Use a date such as")).toBeVisible();
  expect(new URL(page.url()).searchParams.get("valid_at")).toBeNull();
  expect(cspProblems(problems)).toEqual([]);
});

test("identifiers go through as typed, and each kind of result opens where it lives", async ({ page, request }) => {
  await signIn(page);
  // An object's id goes straight to it, as the API decides from the text, which the screen sends as it was typed.
  const found = await untilFound(request, "zorblaxian", (r) => r.items.some((i) => i.kind === "segment"));
  const id = (found.items as { id: string; kind: string }[]).find((i) => i.kind === "segment")?.id ?? "";
  expect(id).toMatch(/^[0-9a-f]{32}$|^[0-9a-f-]{36}$/);
  const asked: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/search?")) asked.push(new URL(r.url()).searchParams.get("q") ?? "");
  });
  await page.goto(searchUrl(`?q=${encodeURIComponent(`  ${id} `)}`));
  await expect(results(page).getByText("found through an exact lookup")).toBeVisible();
  await expect(results(page).locator(`article[data-item-id="${id}"]`)).toBeVisible();
  expect(asked.at(-1)).toBe(`  ${id} `);
  // A word that looks like one goes on the identifier path.
  await page.goto(searchUrl("?q=XK-2024%2F17"));
  await expect(results(page).getByText("found through an identifier")).toBeVisible();
  expect(asked.at(-1)).toBe("XK-2024/17");

  // A claim's history.
  const claim = `lang-sv-${run}`;
  await page.goto(searchUrl(`?q=${encodeURIComponent("räksmörgås blåbär")}`));
  await results(page).locator(`article[data-item-id="${claim}"]`).getByRole("link", { name: "See its history" }).click();
  await expect(page).toHaveURL(new RegExp(`/documents/-/claims/${claim}`));

  // A real page.
  await post(request, api("/pages"), { title: `Fernwick notes ${run}`, body: "The Fernwick lighthouse keeps a daily log.", rationale: "search checks", idempotency_key: `page-${run}` }, 200);
  await untilFound(request, "Fernwick lighthouse", (r) => r.items.some((i) => i.kind === "page" || i.kind === "block"));
  await page.goto(searchUrl("?q=Fernwick%20lighthouse"));
  const pageItem = results(page).locator("article[data-item-kind='page'], article[data-item-kind='block']").first();
  await expect(pageItem).toBeVisible();
  await pageItem.getByRole("link", { name: "Open the page" }).click();
  await expect(page).toHaveURL(new RegExp(`/vaults/${vault}/pages/`));
  await expect(page.getByText("The Fernwick lighthouse keeps a daily log.")).toBeVisible();

  // An entity's link, with the answer replaced at the network, as making one needs the entity pipeline and only the link is this screen's.
  await page.route("**/api/v1/vaults/*/search?*", (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ path: "exact", items: [{ id: "ent-1", kind: "entity", text: "Fernwick", heading: "place", trust_tiers: [], estimated_tokens: 1, citations: [], conflicts: [] }], low_relevance: false, notices: [], budget: { limit: 1, used: 1, truncated: false }, degraded: [] }) }));
  await page.goto(searchUrl("?q=entity"));
  await results(page).locator("article[data-item-kind='entity']").getByRole("link", { name: "Open the entity" }).click();
  await expect(page).toHaveURL(new RegExp(`/vaults/${vault}/entities/ent-1`));
});

test("the screen has no accessibility violations", async ({ page }) => {
  await signIn(page);
  await page.goto(searchUrl("?q=r%C3%A4ksm%C3%B6rg%C3%A5s&debug=1"));
  await expect(results(page).locator("article").first()).toBeVisible();
  const report = await new AxeBuilder({ page }).analyze();
  expect(report.violations.map((v) => `${v.id}: ${v.nodes.length}: ${v.nodes[0]?.html ?? ""}`)).toEqual([]);
  // By keyboard: the box takes focus, and a citation is reachable.
  await expect(box(page)).toBeFocused();
  await results(page).locator('a[data-link-state="citation"]').first().focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/documents\//);
});
