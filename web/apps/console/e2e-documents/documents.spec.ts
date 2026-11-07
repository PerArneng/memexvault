import { execFileSync } from "node:child_process";
import { appendFileSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";
import { makePdf } from "./make-pdf";
import { changedBounds, decodePng, inkIn } from "./png";

// Spec 042. Everything runs on a real server with the real CSP. Timings are recorded rather than asserted here, except where a pass rule says so,
// because the machine's load can move them by far more than the viewer does. The spec's Result has what they came to, beside the load average.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const RECORDING = process.env.MEMEX_E2E_RECORDING;
const DATA_DIR = process.env.MEMEX_DATA_DIR ?? "";
const PG_CONTAINER = process.env.MEMEX_E2E_PG_CONTAINER;

interface Source { readonly id: string; readonly version: number }
interface Rect { readonly page: number; readonly x0: number; readonly y0: number; readonly x1: number; readonly y1: number }
interface Label { readonly file: string; readonly page: number; readonly quote: string; readonly x0: number; readonly y0: number; readonly x1: number; readonly y1: number; readonly rotation: number }
interface Segment { readonly id: string; readonly text: string; readonly page_from?: number | null; readonly page_to?: number | null; readonly t_start_ms?: number | null; readonly t_end_ms?: number | null }

let vault = "";
let readToken = "";
let agentToken = "";
let proposeToken = "";

const baseURL = () => test.info().project.use.baseURL ?? "";
const api = (path: string) => `/api/v1/vaults/${vault}${path}`;
const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load averages:")[1]?.trim() ?? "unknown";
const docsPath = (rest: string) => `/vaults/${vault}/documents${rest}`;

const post = async <T>(request: APIRequestContext, path: string, data: unknown, status = 201, token = adminToken): Promise<T> => {
  const res = await request.post(path, { headers: { authorization: `Bearer ${token}` }, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const get = async <T>(request: APIRequestContext, path: string): Promise<T> => {
  const res = await request.get(path, { headers: bearer });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};

const upload = async (request: APIRequestContext, name: string, data: Buffer | string, query = ""): Promise<Source> => {
  const res = await request.post(api(`/sources?name=${encodeURIComponent(name)}${query}`), { headers: { ...bearer, "content-type": "application/octet-stream" }, data });
  expect(res.status(), `${name}: ${await res.text()}`).toBeLessThan(300);
  return ((await res.json()) as { source: Source }).source;
};

interface Rendition { readonly id: string; readonly superseded_by?: string | null; readonly segment_count: number }

/** Waits for a current rendition with segments. A long document or a recording takes minutes. */
const renditionOf = async (request: APIRequestContext, source: string, minutes = 10): Promise<Rendition> => {
  const deadline = Date.now() + minutes * 60_000;
  for (;;) {
    const { items } = await get<{ items: Rendition[] }>(request, api(`/sources/${source}/renditions`));
    const current = items.find((r) => !r.superseded_by);
    if (current !== undefined) return current;
    if (Date.now() > deadline) throw new Error(`no rendition for ${source}`);
    await new Promise((done) => setTimeout(done, 500));
  }
};

const segmentsOf = async (request: APIRequestContext, rendition: string): Promise<readonly Segment[]> => {
  const all: Segment[] = [];
  let cursor: string | null | undefined = null;
  do {
    const query: string = cursor === null || cursor === undefined ? "" : `&cursor=${encodeURIComponent(cursor)}`;
    const page: { items: Segment[]; next_cursor?: string | null } = await get(request, api(`/renditions/${rendition}/segments?limit=200${query}`));
    all.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor);
  return all;
};

const normal = (text: string) => text.split(/\s+/).filter((w) => w !== "").join(" ");

const addClaim = async (request: APIRequestContext, id: string, segment: string, quote: string, value: string = quote, token = adminToken): Promise<{ claim: string; evidence: string }> => {
  await post(request, api("/proposals"), {
    idempotency_key: `${id}-key`,
    rationale: "the viewer's checks",
    ops: [{ op: "add_assertion", id, subject: `subject ${id}`, predicate: "states", value, epistemic: "asserted", machine: false, confidence_permille: 900, valid_from_ms: null, valid_to_ms: null, evidence: [[`${id}-e`, { segment_id: segment, quote, stance: "supports" }]] }],
  }, 200, token);
  return { claim: id, evidence: `${id}-e` };
};

/** Only this run's database, named by the same URL the server uses, and only for states no route can make yet. */
const sql = (statement: string): string => {
  const database = new URL(process.env.MEMEX_DATABASE_URL ?? "").pathname.slice(1);
  return execFileSync("docker", ["exec", PG_CONTAINER ?? "", "psql", "-U", "postgres", "-d", database, "-Atc", statement]).toString().trim();
};
const vaultSchema = () => sql(`select nspname from pg_namespace where nspname like 'vault\\_${vault}%'`);

const signIn = async (page: Page, token = adminToken) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
};

const signedInContext = async (browser: Browser, token = adminToken): Promise<BrowserContext> => {
  const context = await browser.newContext({ baseURL: baseURL(), colorScheme: "dark", viewport: { width: 1400, height: 1200 }, deviceScaleFactor: 1 });
  const res = await context.request.post("/api/v1/session", { headers: { "x-memex-csrf": "1" }, data: { token } });
  expect(res.ok()).toBe(true);
  return context;
};

/** A CSP refusal, or anything else a browser complains about. A status a bad case asks for is expected, and listed by the test. */
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") problems.push(m.text());
  });
  return problems;
};
const cspProblems = (problems: readonly string[]) => problems.filter((p) => /content security policy/i.test(p));

const pageCell = (page: Page, index: number) => page.locator(`[data-page-index="${String(index)}"]`);

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  console.log(`server started in ${String(await startServer())} ms`);
  const created = await post<{ id: string }>(request, "/api/v1/vaults", { name: `documents-${run}` });
  vault = created.id;
  const makeToken = async (name: string, kind: string, scope: string) => {
    const principal = await post<{ id: string }>(request, "/api/v1/principals", { name: `${name}-${run}`, kind });
    return (await post<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: principal.id, grants: [{ vault_id: vault, scope }] })).token;
  };
  readToken = await makeToken("reader", "human", "read");
  proposeToken = await makeToken("proposer", "human", "propose");
  agentToken = await makeToken("agent", "agent", "propose");
});

test.afterAll(async () => {
  await stopServer();
});

/** The labelled fixtures, uploaded once for the tests that use them. */
const fixtureCache = new Map<string, { source: Source; segments: readonly Segment[] }>();
const fixture = async (request: APIRequestContext, name: string) => {
  const known = fixtureCache.get(name);
  if (known !== undefined) return known;
  const source = await upload(request, `${name}.pdf`, readFileSync(join(repo, "crates/memex-pdf/tests/fixtures", `${name}.pdf`)));
  const rendition = await renditionOf(request, source.id);
  const found = { source, segments: await segmentsOf(request, rendition.id) };
  fixtureCache.set(name, found);
  return found;
};

const screenshotPixels = async (page: Page) => decodePng(await page.screenshot({ animations: "disabled", caret: "hide" }));

interface LabelResult {
  readonly file: string;
  readonly page: number;
  readonly rotation: number;
  readonly quote: string;
  readonly worst: number;
  readonly sides: readonly number[];
  readonly inkShare: number;
}

/** Item 1: the overlay as the browser really paints it, found by the pixels it changes, against the label's box. */
const measureLabel = async (page: Page, label: Label, link: string, zoom: number): Promise<LabelResult> => {
  await page.goto(docsPath(`${link}${zoom === 100 ? "" : `&zoom=${String(zoom)}`}`));
  const cell = pageCell(page, label.page);
  await expect(cell).toHaveAttribute("data-page-state", "loaded");
  const highlights = cell.locator("[data-highlight]");
  await expect(highlights.first()).toBeAttached();
  await expect(page.locator(`[data-page-state="waiting"]`)).toHaveCount(0);
  const shown = await screenshotPixels(page);
  await highlights.evaluateAll((all) => {
    for (const el of all) (el as HTMLElement).style.visibility = "hidden";
  });
  const plain = await screenshotPixels(page);
  await highlights.evaluateAll((all) => {
    for (const el of all) (el as HTMLElement).style.visibility = "";
  });
  const box = await cell.boundingBox();
  if (box === null) throw new Error("no page on screen");
  const k = zoom / 100;
  const expected = { left: box.x + label.x0 * k, top: box.y + label.y0 * k, right: box.x + label.x1 * k, bottom: box.y + label.y1 * k };
  const changed = changedBounds(shown, plain, 6);
  if (changed === null) throw new Error(`${label.file} ${label.quote}: the overlay changed no pixels`);
  const sides = [changed.left - expected.left, changed.top - expected.top, changed.right - expected.right, changed.bottom - expected.bottom];
  const { ink: inside } = inkIn(plain, changed, 128);
  const around = { left: changed.left - 4, top: changed.top - 4, right: changed.right + 4, bottom: changed.bottom + 4 };
  const { ink: nearby } = inkIn(plain, around, 128);
  return { file: label.file, page: label.page, rotation: label.rotation, quote: label.quote, worst: Math.max(...sides.map(Math.abs)), sides: sides.map((s) => Number(s.toFixed(2))), inkShare: nearby === 0 ? 0 : inside / nearby };
};

test("item 1: overlays land within 2 px of the labelled regions at 100% zoom, by pixels, through the real server", async ({ page, request }) => {
  test.setTimeout(3_600_000);
  const problems = problemsOf(page);
  const labels = JSON.parse(readFileSync(join(repo, "crates/memex-pdf/tests/fixtures/labels.json"), "utf8")) as Label[];
  expect(labels).toHaveLength(50);
  await signIn(page);
  const links: string[] = [];
  let index = 0;
  for (const label of labels) {
    index += 1;
    const { source, segments } = await fixture(request, label.file);
    const wanted = normal(label.quote);
    const candidates = segments.filter((s) => normal(s.text).includes(wanted));
    const onPage = candidates.filter((s) => (s.page_from ?? 0) <= label.page && label.page <= (s.page_to ?? s.page_from ?? 0));
    let link: string | null = null;
    let nearest = Number.MAX_VALUE;
    let tries = 0;
    for (const segment of onPage.length > 0 ? onPage : candidates) {
      tries += 1;
      const { claim, evidence } = await addClaim(request, `label-${run}-${String(index)}-${String(tries)}`, segment.id, label.quote);
      // The server, not the test, says where the quote is, as a repeated quote in a segment takes its first place.
      const lineage = await get<{ evidence: { rects: Rect[] }[] }>(request, api(`/assertions/${claim}/lineage`));
      const rects = lineage.evidence[0]?.rects ?? [];
      const first = rects[0];
      if (first === undefined || first.page !== label.page) continue;
      const distance = Math.abs(Math.min(...rects.map((r) => r.x0)) / 100 - label.x0) + Math.abs(Math.min(...rects.map((r) => r.y0)) / 100 - label.y0);
      if (distance < nearest) {
        nearest = distance;
        link = `/${source.id}?claim=${claim}&evidence=${evidence}`;
      }
    }
    if (link === null) throw new Error(`${label.file}: no segment gives a place on page ${String(label.page)} for "${label.quote}"`);
    links.push(link);
  }

  for (const zoom of [100, 125, 200]) {
    const load = loadAverage();
    const results: LabelResult[] = [];
    for (const [i, label] of labels.entries()) results.push(await measureLabel(page, label, links[i] ?? "", zoom));
    const misses = results.filter((r) => r.worst > 2);
    const byFile = [...new Set(results.map((r) => r.file))].map((file) => {
      const mine = results.filter((r) => r.file === file);
      return { file, labels: mine.length, worstPx: Number(Math.max(...mine.map((r) => r.worst)).toFixed(2)), lowestInkShare: Number(Math.min(...mine.map((r) => r.inkShare)).toFixed(3)) };
    });
    record(`item 1 at ${String(zoom)}%`, { labels: results.length, within2px: results.length - misses.length, worstPx: Number(Math.max(...results.map((r) => r.worst)).toFixed(2)), byFile, misses, load });
    if (zoom === 100) test.info().annotations.push({ type: "item 1 at 100%", description: `${String(results.length - misses.length)} of ${String(results.length)} within 2 px` });
  }
  expect(cspProblems(problems)).toEqual([]);
  // The pass rule is checked last, so the other zooms are still recorded when it fails.
  const misses100 = test.info().annotations.find((a) => a.type === "item 1 at 100%")?.description;
  expect(misses100).toBe("50 of 50 within 2 px");
});

test("the session cookie alone loads page images and thumbnails under the strict policy", async ({ page, request }) => {
  const problems = problemsOf(page);
  const { source } = await fixture(request, "rotate90");
  await signIn(page);
  const statuses: { url: string; status: number; csp: string | null }[] = [];
  page.on("response", (r) => {
    if (/\/(image|thumbnail)/.test(r.url())) statuses.push({ url: new URL(r.url()).pathname + new URL(r.url()).search, status: r.status(), csp: r.headers()["content-security-policy"] ?? null });
  });
  await page.goto(docsPath(`/${source.id}`));
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  await expect(page.locator("[data-thumbnail]").first()).toBeVisible();
  await expect.poll(() => statuses.filter((s) => s.url.includes("thumbnail")).length).toBeGreaterThan(0);
  expect(statuses.every((s) => s.status === 200)).toBe(true);
  // Without the cookie the same address is refused, so it's the cookie that let it through.
  const anonymous = await page.request.fetch(new URL(statuses[0]?.url ?? "", baseURL()).toString(), { headers: { cookie: "" } });
  record("page image with no cookie", anonymous.status());
  record("page responses", statuses.slice(0, 3));
  expect(cspProblems(problems)).toEqual([]);
});

test("item 3: a link to a time plays from it, and the transcript follows, on a real transcribed recording", async ({ page, request, browser }) => {
  test.setTimeout(1_800_000);
  test.skip(RECORDING === undefined, "set MEMEX_E2E_RECORDING to a recording Chromium can play");
  const started = Date.now();
  const source = await upload(request, "recording.mp3", readFileSync(RECORDING ?? ""), "&language=en");
  const rendition = await renditionOf(request, source.id, 25);
  record("transcribed end to end through serve", { ms: Date.now() - started, load: loadAverage(), parser: (await get<{ items: { parser: string }[] }>(request, api(`/sources/${source.id}/renditions`))).items[0]?.parser });
  const segments = await segmentsOf(request, rendition.id);
  expect(segments.length).toBeGreaterThan(0);
  const middle = segments[Math.floor(segments.length / 2)];
  const start = middle?.t_start_ms ?? 0;
  const end = middle?.t_end_ms ?? start + 5000;
  expect(start).toBeGreaterThan(30_000);
  record("the cited range", { start, end, segments: segments.length });
  // A real claim on a real segment, so the deep link goes from a claim to its moment.
  const words = normal(middle?.text ?? "").split(" ").slice(0, 6).join(" ");
  const cited = await addClaim(request, `said-${run}`, middle?.id ?? "", words);
  const lineage = await get<{ evidence: { t_start_ms?: number; t_end_ms?: number; broken: boolean }[] }>(request, api(`/assertions/${cited.claim}/lineage`));
  record("the claim's anchor on the recording", lineage.evidence[0]);
  expect(lineage.evidence[0]?.broken).toBe(false);

  const watch = async (p: Page) => {
    await p.addInitScript(() => {
      const w = window as unknown as { __media: { clickAt: number | null; playingAt: number | null; playingPos: number | null; advancedAt: number | null } };
      w.__media = { clickAt: null, playingAt: null, playingPos: null, advancedAt: null };
      document.addEventListener("click", () => { w.__media.clickAt = performance.now(); }, true);
      document.addEventListener("playing", (e) => {
        const el = e.target as HTMLMediaElement;
        if (w.__media.playingAt !== null) return;
        w.__media.playingAt = performance.now();
        w.__media.playingPos = el.currentTime;
        const step = () => {
          if (el.currentTime > (w.__media.playingPos ?? 0) + 0.01) w.__media.advancedAt = performance.now();
          else requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      }, true);
    });
  };
  const measured = (p: Page) => p.evaluate(() => (window as unknown as { __media: { clickAt: number | null; playingAt: number | null; playingPos: number | null; advancedAt: number | null } }).__media);
  const runs: Record<string, unknown>[] = [];

  // Through the page: a click on a segment's time, which is what a person does, so it has a user gesture.
  for (let i = 0; i < 5; i += 1) {
    const context = await signedInContext(browser);
    const p = await context.newPage();
    await watch(p);
    await p.goto(docsPath(`/${source.id}?tab=segments`));
    const link = p.locator(`[data-segment="${middle?.id ?? ""}"] a`).first();
    await expect(link).toBeVisible();
    const statuses: number[] = [];
    p.on("response", (r) => {
      if (r.url().endsWith("/stream")) statuses.push(r.status());
    });
    await link.click();
    await expect.poll(async () => (await measured(p)).advancedAt, { timeout: 60_000 }).not.toBeNull();
    const m = await measured(p);
    runs.push({ via: "click", positionErrorMs: Math.round(Math.abs((m.playingPos ?? 0) * 1000 - start)), toAdvancingMs: Math.round((m.advancedAt ?? 0) - (m.clickAt ?? 0)), statuses, load: loadAverage() });
    await context.close();
  }

  // A fresh load of the same link, as from another program. A browser may want a click first, and if it does, that's counted apart.
  for (let i = 0; i < 5; i += 1) {
    const context = await signedInContext(browser);
    const p = await context.newPage();
    await watch(p);
    await p.goto(docsPath(`/${source.id}?t=${String(start)}&end=${String(end)}`));
    const audio = p.locator("audio");
    await expect(audio).toBeAttached();
    const outcome = await Promise.race([
      expect.poll(async () => (await measured(p)).advancedAt, { timeout: 30_000 }).not.toBeNull().then(() => "played"),
      p.getByRole("status").filter({ hasText: "wants a click" }).waitFor().then(() => "blocked"),
    ]);
    if (outcome === "blocked") {
      const position = await audio.evaluate((el: HTMLAudioElement) => el.currentTime);
      runs.push({ via: "fresh load", blockedByAutoplay: true, readyAtErrorMs: Math.round(Math.abs(position * 1000 - start)), load: loadAverage() });
    } else {
      const m = await measured(p);
      runs.push({ via: "fresh load", positionErrorMs: Math.round(Math.abs((m.playingPos ?? 0) * 1000 - start)), toAdvancingMs: Math.round(m.advancedAt ?? 0), load: loadAverage() });
    }
    await context.close();
  }
  record("item 3 runs", runs);
  const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? Number.NaN;
  const of = (via: string, key: string) => runs.flatMap((r) => (r.via === via && typeof r[key] === "number" ? [r[key]] : []));
  record("item 3 medians", {
    click: { positionErrorMs: median(of("click", "positionErrorMs")), toAdvancingMs: median(of("click", "toAdvancingMs")) },
    freshLoad: { positionErrorMs: median(of("fresh load", "positionErrorMs")), toAdvancingMs: median(of("fresh load", "toAdvancingMs")), blocked: runs.filter((r) => r.blockedByAutoplay === true).length },
  });

  // A browser that insists on a click. Playwright's Chromium doesn't, whatever flags it gets, so the refusal is scripted, as a real one comes to the page.
  const strictContext = await signedInContext(browser);
  const strictPage = await strictContext.newPage();
  await strictPage.addInitScript(() => {
    const play = HTMLMediaElement.prototype.play;
    let refusals = 1;
    HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
      if (refusals > 0) {
        refusals -= 1;
        return Promise.reject(new DOMException("needs a click", "NotAllowedError"));
      }
      return play.call(this);
    };
  });
  await strictPage.goto(docsPath(`/${source.id}?t=${String(start)}&end=${String(end)}`));
  const notice = strictPage.getByRole("status").filter({ hasText: "wants a click" });
  await expect(notice).toBeVisible();
  const readyAt = await strictPage.locator("audio").evaluate((el: HTMLAudioElement) => ({ time: el.currentTime, paused: el.paused }));
  await notice.getByRole("button", { name: "Play" }).click();
  await expect.poll(() => strictPage.locator("audio").evaluate((el: HTMLAudioElement, at) => !el.paused && el.currentTime > at, start / 1000)).toBe(true);
  await expect(notice).toHaveCount(0);
  record("a scripted autoplay refusal", { ...readyAt, requested: start / 1000, afterClick: "playing" });
  await strictContext.close();

  // The rest of the player, on the same recording.
  const problems = problemsOf(page);
  await signIn(page);
  const ranges: string[] = [];
  page.on("request", (r) => {
    if (r.url().endsWith("/stream")) ranges.push(r.headers().range ?? "none");
  });
  const streamed: number[] = [];
  page.on("response", (r) => {
    if (r.url().endsWith("/stream")) streamed.push(r.status());
  });
  await page.goto(docsPath(`/${source.id}?claim=${cited.claim}&evidence=${cited.evidence}`));
  await expect(page.locator("[data-evidence-state='current']")).toBeVisible();
  await expect(page.locator("audio")).toBeAttached();
  const transcript = page.getByRole("list", { name: "Transcript text" });
  await expect(transcript.locator("li").first()).toBeVisible();
  await expect(transcript.locator("[data-part-in-range='true']").first()).toBeVisible();
  await expect(transcript.locator("[aria-current='true']")).toHaveCount(1);
  // Click-to-seek moves the player and the transcript with it.
  const target = transcript.locator("li").nth(1);
  const time = await target.getByRole("button").first().textContent();
  await target.getByRole("button").first().click();
  await expect(target).toHaveAttribute("aria-current", "true");
  const at = await page.locator("audio").evaluate((el: HTMLAudioElement) => el.currentTime);
  record("click-to-seek", { label: time, currentTime: at });
  await expect.poll(() => streamed.includes(206)).toBe(true);
  record("stream ranges", { ranges: ranges.slice(0, 6), statuses: [...new Set(streamed)] });
  // Playing a range stops at its end.
  await page.getByRole("button", { name: "Play the cited range again" }).click();
  await page.locator("audio").evaluate((el: HTMLAudioElement) => { el.muted = true; });
  await expect.poll(() => page.locator("audio").evaluate((el: HTMLAudioElement) => el.paused && el.currentTime > 0), { timeout: 120_000 }).toBe(true);
  const stoppedAt = await page.locator("audio").evaluate((el: HTMLAudioElement) => el.currentTime * 1000);
  record("a range stops at its end", { end: lineage.evidence[0]?.t_end_ms, stoppedAt: Math.round(stoppedAt) });
  expect(cspProblems(problems)).toEqual([]);
  const { violations } = await new AxeBuilder({ page }).analyze();
  expect(violations.map((v) => `${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
});

const HOSTILE = `<img src=x onerror="window.__pwned=1"><script>window.__pwned=1</script><b>bold-${run}</b>`;

const expectNoInjection = async (page: Page) => {
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(page.locator(`main img[src="x"]`)).toHaveCount(0);
  await expect(page.locator(`main script`)).toHaveCount(0);
  await expect(page.locator(`main b`, { hasText: `bold-${run}` })).toHaveCount(0);
};

test("item 4: markup in a document stays plain text in segments, quotes, claims, names, words and transcripts", async ({ page, request }) => {
  const problems = problemsOf(page);
  const name = `<img src=x onerror="window.__pwned=1">-${run}.txt`;
  const source = await upload(request, name, `Before. ${HOSTILE} After.`);
  const rendition = await renditionOf(request, source.id);
  const segments = await segmentsOf(request, rendition.id);
  const segment = segments[0];
  expect(normal(segment?.text ?? "")).toContain(`<b>bold-${run}</b>`);
  const { claim, evidence } = await addClaim(request, `hostile-${run}`, segment?.id ?? "", `<script>window.__pwned=1</script><b>bold-${run}</b>`);

  await signIn(page);
  await page.goto(docsPath(""));
  await expect(page.getByRole("link", { name })).toBeVisible();
  await expectNoInjection(page);

  // Segments.
  await page.goto(docsPath(`/${source.id}?tab=segments`));
  await expect(page.locator(`[data-segment="${segment?.id ?? ""}"]`)).toContainText(`<b>bold-${run}</b>`);
  await expect(page.getByRole("heading", { name })).toBeVisible();
  await expectNoInjection(page);

  // Quotes, in the deep link's banner and in lineage.
  await page.goto(docsPath(`/${source.id}?claim=${claim}&evidence=${evidence}`));
  await expect(page.getByText(`<script>window.__pwned=1</script><b>bold-${run}</b>`).first()).toBeVisible();
  await expectNoInjection(page);
  await page.goto(docsPath(`/-/claims/${claim}?evidence=${evidence}`));
  await expect(page.locator("[data-quote]")).toHaveText(`<script>window.__pwned=1</script><b>bold-${run}</b>`);
  await expectNoInjection(page);

  // Claims.
  await page.goto(docsPath(`/${source.id}?tab=claims`));
  await expect(page.getByText(`<b>bold-${run}</b>`).first()).toBeVisible();
  await expectNoInjection(page);

  // A PDF's words in the selectable layer.
  const pdf = await upload(request, `hostile-${run}.pdf`, makePdf(1, () => `<img src=x onerror=window.__pwned=1><b>bold-${run}</b>`));
  await renditionOf(request, pdf.id);
  await page.goto(docsPath(`/${pdf.id}`));
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  await page.getByRole("button", { name: "Select text" }).click();
  await expect(page.locator("[data-word]").first()).toBeAttached();
  await expect(page.locator(`[data-word*="<b>bold-${run}"]`).first()).toBeAttached();
  await expectNoInjection(page);

  // A transcript. A real transcriber won't write markup on demand, so the server's answer is replaced at the network, and the real screen, its CSP and React do the rest.
  const recording = await upload(request, `hostile-${run}.wav`, readFileSync(join(repo, "crates/memex-asr/tests/fixtures/tone.wav")));
  await page.route(`**/sources/${recording.id}/transcript*`, (route) =>
    route.fulfill({
      json: {
        rendition_id: "r", parser: "scripted", duration_ms: 2000,
        parts: [{ start: 0, end: 10, text: HOSTILE, t_start_ms: 0, t_end_ms: 2000, speaker: `<img src=x onerror="window.__pwned=1">`, words: [{ text: `<b>bold-${run}</b>`, start_ms: 0, end_ms: 2000 }] }],
      },
    }),
  );
  await page.goto(docsPath(`/${recording.id}`));
  const list = page.getByRole("list", { name: "Transcript text" });
  await expect(list).toContainText(`<b>bold-${run}</b>`);
  await expect(list).toContainText(`<img src=x onerror=`);
  await expectNoInjection(page);
  expect(cspProblems(problems)).toEqual([]);
});

test("item 5: a 400-page document opens with its first page within 1 s when warm, and fetches only what's visible", async ({ browser, request }) => {
  test.setTimeout(1_800_000);
  const built = Date.now();
  const source = await upload(request, `long-${run}.pdf`, makePdf(400, (page) => `Document page ${String(page + 1)} of the long document`));
  const rendition = await renditionOf(request, source.id, 25);
  record("the 400-page document", { ms: Date.now() - built, load: loadAverage(), bytes: (await get<{ size: number }>(request, api(`/sources/${source.id}`))).size, segments: rendition.segment_count });
  const pagesFact = await get<{ page_count: number }>(request, api(`/sources/${source.id}/pages`));
  expect(pagesFact.page_count).toBe(400);
  // The server has now rendered it, so its cache is warm. Each run below is a new browser with no copy of it.
  const warm = await request.get(api(`/sources/${source.id}/pages/0/image?dpi=72`), { headers: bearer });
  expect(warm.status()).toBe(200);
  const warm2 = Date.now();
  await request.get(api(`/sources/${source.id}/pages/0/image?dpi=72`), { headers: bearer });
  record("a warm page image, alone", { ms: Date.now() - warm2, load: loadAverage() });

  interface Seen { readonly images: readonly number[]; readonly thumbnails: readonly number[]; readonly rendition: boolean }
  const follow = (page: Page) => {
    const seen = { images: [] as number[], thumbnails: [] as number[], rendition: false };
    page.on("request", (r) => {
      const url = r.url();
      const image = /\/pages\/(\d+)\/image/.exec(url);
      const thumb = /\/pages\/(\d+)\/thumbnail/.exec(url);
      if (image !== null) seen.images.push(Number(image[1]));
      if (thumb !== null) seen.thumbnails.push(Number(thumb[1]));
      if (/\/renditions\/[^/?]+$/.test(new URL(url).pathname)) seen.rendition = true;
    });
    return seen as Seen;
  };
  const visibleNow = (page: Page) =>
    page.evaluate(() => {
      const scroller = document.querySelector('[aria-label="Pages"]');
      if (scroller === null) return [];
      const view = scroller.getBoundingClientRect();
      return [...document.querySelectorAll("[data-page-index]")].flatMap((cell) => {
        const box = cell.getBoundingClientRect();
        return box.bottom > view.top && box.top < view.bottom ? [Number((cell as HTMLElement).dataset.pageIndex)] : [];
      });
    });

  const times: number[] = [];
  const openRuns: Record<string, unknown>[] = [];
  for (let i = 0; i < 5; i += 1) {
    const context = await signedInContext(browser);
    const page = await context.newPage();
    await page.addInitScript(() => {
      const w = window as unknown as { __firstPage: number | null };
      w.__firstPage = null;
      new MutationObserver(() => {
        if (w.__firstPage === null && document.querySelector('[data-page-index="0"][data-page-state="loaded"]') !== null) w.__firstPage = performance.now();
      }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-page-state"] });
    });
    const seen = follow(page);
    await page.goto(docsPath(`/${source.id}`));
    await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
    const ms = await page.evaluate(() => (window as unknown as { __firstPage: number }).__firstPage);
    const onScreen = await visibleNow(page);
    // The rail comes after the first page, so give it its chance before counting.
    await expect(page.locator("[data-thumbnail]").first()).toBeAttached();
    await page.waitForLoadState("networkidle");
    openRuns.push({ ms: Math.round(ms), load: loadAverage(), images: [...seen.images], onScreen, thumbnails: seen.thumbnails.length, thumbnailMax: Math.max(...seen.thumbnails), wordTable: seen.rendition, cellsInDom: await page.locator("[data-page-index]").count() });
    times.push(ms);
    expect([...new Set(seen.images)].sort((a, b) => a - b)).toEqual(onScreen);
    expect(seen.rendition).toBe(false);
    await context.close();
  }
  const sorted = [...times].sort((a, b) => a - b);
  const median = sorted[2] ?? Number.NaN;
  record("item 5 opens", { runs: openRuns, medianMs: Math.round(median), worstMs: Math.round(sorted[4] ?? 0) });

  // In a page already running, from the list: the case a person sees most.
  {
    const context = await signedInContext(browser);
    const page = await context.newPage();
    const inApp: number[] = [];
    await page.goto(docsPath(`?q=long-${run}`));
    await page.waitForLoadState("networkidle");
    for (let i = 0; i < 5; i += 1) {
      const link = page.getByRole("link", { name: `long-${run}.pdf` });
      await expect(link).toBeVisible();
      const started = Date.now();
      await link.click();
      await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
      inApp.push(Date.now() - started);
      await page.goBack();
    }
    record("item 5 opens from the list, in the same browser", { runs: inApp, load: loadAverage() });
    await context.close();
  }

  // Jumping into the middle, and then scrolling.
  const context = await signedInContext(browser);
  const page = await context.newPage();
  const seen = follow(page);
  await page.goto(docsPath(`/${source.id}?page=200`));
  await expect(pageCell(page, 199)).toHaveAttribute("data-page-state", "loaded");
  await expect(page.locator("[data-thumbnail]").first()).toBeAttached();
  await page.waitForLoadState("networkidle");
  const onScreen = await visibleNow(page);
  record("a link to page 200", { images: [...seen.images], onScreen, thumbnails: [...new Set(seen.thumbnails)].length, thumbnailRange: [Math.min(...seen.thumbnails), Math.max(...seen.thumbnails)] });
  expect([...new Set(seen.images)].sort((a, b) => a - b)).toEqual(onScreen);
  expect(seen.images).not.toContain(0);

  const before = seen.images.length;
  const scroller = page.getByRole("group", { name: "Pages" });
  await scroller.hover();
  for (let i = 0; i < 40; i += 1) {
    await page.mouse.wheel(0, 300);
  }
  await page.waitForLoadState("networkidle");
  await expect(page.locator("[data-page-state='waiting']")).toHaveCount(0);
  const after = await visibleNow(page);
  const fetched = seen.images.slice(before);
  record("scrolling 12,000 px, about 14 pages, in one go", { fetched, onScreenAfter: after, url: page.url().split("?")[1] });
  // It settled, and only then asked. What it passed through while moving isn't fetched.
  expect(fetched.length).toBeLessThanOrEqual(4);
  await context.close();

  // The rule is last, so everything above is recorded even when it's missed.
  expect(median).toBeLessThanOrEqual(1000);
});

test("selecting text: the word layer lines up with the words and copies them, on plain and rotated pages", async ({ page, request }) => {
  const problems = problemsOf(page);
  await signIn(page);
  for (const name of ["one_column_sv", "rotate90", "rotate180_crop", "rotate270_crop"]) {
    const { source } = await fixture(request, name);
    await page.goto(docsPath(`/${source.id}`));
    await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
    await page.getByRole("button", { name: "Select text" }).click();
    const cell = pageCell(page, 0);
    const words = cell.locator("[data-word]");
    await expect(words.first()).toBeAttached();
    const count = await words.count();
    const table = await get<{ word_table: { words: { text: string; page: number; rect: { x0: number; y0: number; x1: number; y1: number } }[] } }>(request, api(`/renditions/${(await renditionOf(request, source.id)).id}`));
    const onPage = table.word_table.words.filter((w) => w.page === 0);
    expect(count).toBe(onPage.length);
    // A span's box as the browser lays it out, turned and stretched, against the word table's rect.
    const cellBox = await cell.boundingBox();
    const boxes = await words.evaluateAll((all) => all.map((el) => {
      const b = el.getBoundingClientRect();
      return { left: b.left, top: b.top, right: b.right, bottom: b.bottom };
    }));
    let worst = 0;
    for (const [i, word] of onPage.entries()) {
      const b = boxes[i];
      if (b === undefined || cellBox === null) continue;
      worst = Math.max(worst, Math.abs(b.left - cellBox.x - word.rect.x0 / 100), Math.abs(b.top - cellBox.y - word.rect.y0 / 100), Math.abs(b.right - cellBox.x - word.rect.x1 / 100), Math.abs(b.bottom - cellBox.y - word.rect.y1 / 100));
    }
    const selected = await cell.locator("[data-word-layer]").evaluate((layer) => {
      const range = document.createRange();
      range.selectNodeContents(layer);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      return selection?.toString() ?? "";
    });
    expect(normal(selected).split(" ").slice(0, 5)).toEqual(onPage.slice(0, 5).flatMap((w) => normal(w.text).split(" ")).slice(0, 5));
    record(`word layer on ${name}`, { words: count, worstSidePx: Number(worst.toFixed(2)), selectedChars: selected.length });
    expect(worst).toBeLessThanOrEqual(2);
  }
  expect(cspProblems(problems)).toEqual([]);
});

test("the viewer's controls and the URL: pages, zoom, thumbnails, filters and reloading", async ({ page, request }) => {
  const problems = problemsOf(page);
  const { source } = await fixture(request, "two_column_en");
  await fixture(request, "three_column_de");
  await signIn(page);
  await page.goto(docsPath("?q=two_column"));
  await expect(page.getByRole("link", { name: "two_column_en.pdf" })).toBeVisible();
  await expect(page.getByRole("link", { name: "three_column_de.pdf" })).toHaveCount(0);
  await page.getByLabel("Kind").selectOption("audio");
  await expect(page).toHaveURL(/kind=audio/);
  await expect(page.getByText("No documents match these filters.")).toBeVisible();
  await page.getByLabel("Kind").selectOption("pdf");
  await page.reload();
  await expect(page.getByLabel("Kind")).toHaveValue("pdf");
  await expect(page.getByLabel("Filter by name")).toHaveValue("two_column");
  await page.getByRole("link", { name: "two_column_en.pdf" }).click();
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  await page.getByRole("button", { name: "Zoom in" }).click();
  await expect(page).toHaveURL(/zoom=125/);
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  const wide = await pageCell(page, 0).boundingBox();
  const natural = (await get<{ pages: { width: number }[] }>(request, api(`/sources/${source.id}/pages`))).pages[0]?.width ?? 0;
  expect(wide?.width).toBeCloseTo((natural / 100) * 1.25, 0);
  await page.getByRole("button", { name: "Next page" }).click();
  await expect(page).toHaveURL(/page=2/);
  await page.reload();
  await expect(page.getByLabel("Zoom", { exact: true })).toHaveValue("125");
  await expect(pageCell(page, 1)).toHaveAttribute("data-page-state", "loaded");
  await page.getByRole("button", { name: "Go to page 1" }).click();
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  await page.getByLabel("Page number").fill("2");
  await page.getByLabel("Page number").press("Enter");
  await expect(page).toHaveURL(/page=2/);
  for (const tab of ["Segments", "Claims", "Renditions", "Versions"]) {
    await page.getByRole("navigation", { name: "This document" }).getByRole("link", { name: tab }).click();
    await expect(page).toHaveURL(new RegExp(`tab=${tab.toLowerCase()}`));
    await expect(page.getByRole("navigation", { name: "This document" }).getByRole("link", { name: tab })).toHaveAttribute("aria-current", "page");
  }
  await expect(page.getByRole("list", { name: "Versions" }).getByText("Version 1")).toBeVisible();
  await page.goto(docsPath("/-/nothing"));
  await expect(page.getByText("There's nothing at this address.")).toBeVisible();
  const { violations } = await new AxeBuilder({ page }).analyze();
  expect(violations.map((v) => `${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
  expect(cspProblems(problems)).toEqual([]);
});

test("axe finds no violations on the list, the viewer with a highlight, its panels and lineage", async ({ page, request }) => {
  const { source, segments } = await fixture(request, "cropbox_offset");
  const segment = segments[0];
  const quote = normal(segment?.text ?? "").split(" ").slice(0, 4).join(" ");
  const { claim, evidence } = await addClaim(request, `axe-${run}`, segment?.id ?? "", quote);
  await signIn(page);
  const check = async (when: string) => {
    const { violations } = await new AxeBuilder({ page }).analyze();
    expect(violations.map((v) => `${when}: ${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
  };
  await page.goto(docsPath(""));
  await expect(page.getByRole("table", { name: "Documents" })).toBeVisible();
  await check("list");
  await page.goto(docsPath(`/${source.id}?claim=${claim}&evidence=${evidence}`));
  await expect(page.locator("[data-highlight]").first()).toBeAttached();
  await expect(page.locator("[data-thumbnail]").first()).toBeAttached();
  await page.getByRole("button", { name: "Select text" }).click();
  await expect(page.locator("[data-word]").first()).toBeAttached();
  await check("viewer");
  await page.getByRole("button", { name: "Change trust tier" }).click();
  await check("trust panel");
  await page.goto(docsPath(`/-/claims/${claim}?evidence=${evidence}`));
  await expect(page.locator("[data-quote]")).toBeVisible();
  await check("lineage");
});

test("trust tier: a person, a proposer, an agent and a reader each see what the server lets them do", async ({ page, request, browser }) => {
  const problems = problemsOf(page);
  await signIn(page);
  const newSource = async (name: string, tier: string) => {
    const source = await upload(request, `${name}-${run}.txt`, `Text of ${name} ${run}.`, `&trust_tier=${tier}`);
    await renditionOf(request, source.id);
    return source;
  };
  const change = async (p: Page, tier: string) => {
    await p.getByRole("button", { name: "Change trust tier" }).click();
    const panel = p.getByRole("region", { name: "Change trust tier" });
    await expect(panel).toContainText("Only sources a person marked trusted or verified can auto-commit");
    await panel.getByRole("radio", { name: new RegExp(`^${tier}$`, "i") }).check();
    return panel;
  };

  // A person with admin scope raises an unverified source. It applies at once.
  const plain = await newSource("plain", "unverified");
  await page.goto(docsPath(`/${plain.id}`));
  const panel = await change(page, "trusted");
  await expect(panel.locator("[data-trust-effect]")).toHaveAttribute("data-trust-effect", "person_commits_agent_waits");
  await panel.getByRole("button", { name: "Change to trusted" }).click();
  await expect(panel).toHaveCount(0);
  await expect(page.getByRole("main").getByText("Trusted", { exact: true }).first()).toBeVisible();
  expect((await get<{ trust_tier: string }>(request, api(`/sources/${plain.id}`))).trust_tier).toBe("trusted");
  record("a person raises unverified to trusted", "committed at once");

  // A person raising an untrusted source waits for review, however much scope they have.
  const risky = await newSource("risky", "untrusted");
  await page.goto(docsPath(`/${risky.id}`));
  const held = await change(page, "trusted");
  await expect(held.locator("[data-trust-effect]")).toHaveAttribute("data-trust-effect", "review_always");
  await held.getByRole("button", { name: "Change to trusted" }).click();
  await expect(held).toContainText("waiting for review");
  await expect(held.getByRole("link", { name: "Open it in Review" })).toBeVisible();
  expect((await get<{ trust_tier: string }>(request, api(`/sources/${risky.id}`))).trust_tier).toBe("untrusted");
  record("a person raises untrusted to trusted", "held for review, and the tier is unchanged");

  // A person with propose scope, as a screen sees it.
  const propose = await signedInContext(browser, proposeToken);
  const proposePage = await propose.newPage();
  const second = await newSource("second", "unverified");
  await proposePage.goto(docsPath(`/${second.id}`));
  const proposePanel = await change(proposePage, "trusted");
  await proposePanel.getByRole("button", { name: "Change to trusted" }).click();
  await expect(proposePanel).toHaveCount(0);
  record("a proposer, a person, raises unverified to trusted", (await get<{ trust_tier: string }>(request, api(`/sources/${second.id}`))).trust_tier);
  await propose.close();

  // An agent asking for the same waits for a person.
  const agent = await signedInContext(browser, agentToken);
  const agentPage = await agent.newPage();
  const third = await newSource("third", "unverified");
  await agentPage.goto(docsPath(`/${third.id}`));
  const agentPanel = await change(agentPage, "trusted");
  await agentPanel.getByRole("button", { name: "Change to trusted" }).click();
  await expect(agentPanel).toContainText("waiting for review");
  expect((await get<{ trust_tier: string }>(request, api(`/sources/${third.id}`))).trust_tier).toBe("unverified");
  record("an agent raises unverified to trusted", "held for review, and the tier is unchanged");
  await agent.close();

  // A reader sees why, has no way to send it, and the server refuses if it's sent anyway.
  const reader = await signedInContext(browser, readToken);
  const readerPage = await reader.newPage();
  await readerPage.goto(docsPath(`/${third.id}`));
  await expect(readerPage.getByText("Your token has read scope, so it can read this source and not change it.")).toBeVisible();
  await expect(readerPage.getByRole("button", { name: "Reprocess" })).toHaveCount(0);
  await readerPage.getByRole("button", { name: "Change trust tier" }).click();
  const readerPanel = readerPage.getByRole("region", { name: "Change trust tier" });
  await expect(readerPanel).toContainText("You can't change this source's trust tier.");
  await expect(readerPanel).toContainText("It needs propose scope, and your token has read.");
  await expect(readerPanel.getByRole("radio")).toHaveCount(0);
  await expect(readerPanel.getByRole("button", { name: /Change to/ })).toHaveCount(0);
  const refused = await request.post(api("/proposals"), {
    headers: { authorization: `Bearer ${readToken}` },
    data: { idempotency_key: `reader-${run}`, rationale: "no", ops: [{ op: "set_source_trust", id: third.id, base_version: third.version, trust_tier: "trusted" }] },
  });
  const reprocessRefused = await request.post(api(`/sources/${third.id}/reprocess`), { headers: { authorization: `Bearer ${readToken}` } });
  record("a read token sent anyway", { proposal: refused.status(), reprocess: reprocessRefused.status() });
  expect(refused.status()).toBe(403);
  expect(reprocessRefused.status()).toBe(403);
  await reader.close();

  // Someone changed it first: the screen says so, and nothing is done.
  const raced = await newSource("raced", "unverified");
  await page.goto(docsPath(`/${raced.id}`));
  const racedPanel = await change(page, "untrusted");
  await post(request, api("/proposals"), { idempotency_key: `race-${run}`, rationale: "first", ops: [{ op: "set_source_trust", id: raced.id, base_version: raced.version, trust_tier: "trusted" }] }, 200);
  await racedPanel.getByRole("button", { name: "Change to untrusted" }).click();
  await expect(racedPanel).toContainText("has changed since it was opened");
  expect((await get<{ trust_tier: string }>(request, api(`/sources/${raced.id}`))).trust_tier).toBe("trusted");
  expect(cspProblems(problems)).toEqual([]);
});

test("item 6: a broken anchor, a superseded rendition, a missing blob, an erased source and a type the browser can't play each show a clear state", async ({ page, request }) => {
  test.setTimeout(900_000);
  await signIn(page);

  // A superseded rendition, reached by really reprocessing through the screen.
  const { source: older, segments } = await fixture(request, "clean_pages");
  const segment = segments[0];
  const quote = normal(segment?.text ?? "").split(" ").slice(0, 3).join(" ");
  const kept = await addClaim(request, `kept-${run}`, segment?.id ?? "", quote);
  const before = await renditionOf(request, older.id);
  await page.goto(docsPath(`/${older.id}`));
  await page.getByRole("button", { name: "Reprocess" }).click();
  const panel = page.getByRole("region", { name: "Reprocess" });
  await panel.getByRole("button", { name: "Reprocess" }).click();
  await expect(panel).toContainText("Reprocessing has been queued");
  await expect.poll(async () => (await get<{ items: Rendition[] }>(request, api(`/sources/${older.id}/renditions`))).items.length, { timeout: 300_000 }).toBeGreaterThan(1);
  await page.goto(docsPath(`/${older.id}?tab=renditions`));
  await expect(page.locator(`[data-rendition="${before.id}"]`).getByText("Superseded")).toBeVisible();
  await page.locator(`[data-rendition="${before.id}"]`).getByRole("link", { name: "Segments" }).click();
  await expect(page.locator("[data-superseded-rendition]")).toContainText("superseded rendition");
  const evidenceNow = await get<{ evidence: { superseded: boolean; broken: boolean; rendition_id: string }[] }>(request, api(`/assertions/${kept.claim}/lineage`));
  record("a claim after its rendition is replaced", evidenceNow.evidence[0]);
  // Re-anchoring moved it to the new rendition, as it should when the quote is still there. A claim left on the old one is what `superseded` marks, so the row is put back there, in this database only.
  if (evidenceNow.evidence[0]?.superseded !== true && PG_CONTAINER !== undefined) {
    const schema = vaultSchema();
    const oldSegment = (await segmentsOf(request, before.id))[0];
    sql(`update "${schema}".evidence set segment_id = '${oldSegment?.id ?? ""}', row = jsonb_set(row, '{evidence,segment_id}', '"${oldSegment?.id ?? ""}"') where id = '${kept.evidence}'`);
  }
  await page.goto(docsPath(`/${older.id}?claim=${kept.claim}&evidence=${kept.evidence}`));
  await expect(page.locator("[data-evidence-state='superseded']")).toContainText("superseded rendition");
  await expect(page.locator("[data-highlight]").first()).toBeAttached();
  await page.goto(docsPath(`/-/claims/${kept.claim}?evidence=${kept.evidence}`));
  await expect(page.locator(`[data-evidence="${kept.evidence}"]`)).toHaveAttribute("data-evidence-state", "superseded");

  // A broken anchor. Only re-anchoring or an audit sets this, so it's set on the row in this database, and the server's answer and the screen are real.
  test.skip(PG_CONTAINER === undefined, "set MEMEX_E2E_PG_CONTAINER, which these states need");
  const { source: brokenSource, segments: brokenSegments } = await fixture(request, "cropbox_offset");
  const brokenSegment = brokenSegments[0];
  const brokenQuote = normal(brokenSegment?.text ?? "").split(" ").slice(0, 4).join(" ");
  const broken = await addClaim(request, `broken-${run}`, brokenSegment?.id ?? "", brokenQuote);
  const schema = vaultSchema();
  sql(`update "${schema}".evidence set anchor_broken = true, row = jsonb_set(row, '{evidence,anchor_broken}', 'true') where id = '${broken.evidence}'`);
  const reported = await get<{ evidence: { broken: boolean; rects: unknown[] }[] }>(request, api(`/assertions/${broken.claim}/lineage`));
  record("the server on a broken anchor", { broken: reported.evidence[0]?.broken, rects: reported.evidence[0]?.rects.length });
  expect(reported.evidence[0]?.broken).toBe(true);
  await page.goto(docsPath(`/${brokenSource.id}?claim=${broken.claim}&evidence=${broken.evidence}`));
  await expect(page.locator("[data-evidence-state='broken']")).toContainText("This anchor is broken");
  await expect(page.locator("[data-evidence-state='broken']")).toContainText(brokenQuote);
  await expect(pageCell(page, 0)).toHaveAttribute("data-page-state", "loaded");
  await expect(page.locator("[data-highlight]")).toHaveCount(0);
  await page.goto(docsPath(`/-/claims/${broken.claim}?evidence=${broken.evidence}`));
  await expect(page.locator(`[data-evidence="${broken.evidence}"]`)).toHaveAttribute("data-evidence-state", "broken");
  await expect(page.locator(`[data-evidence="${broken.evidence}"]`).getByText("Show it in the source")).toHaveCount(0);

  // A missing blob: the original's file really deleted from this run's data folder, on a source whose pages were never rendered, so no cache can answer.
  const lost = await upload(request, `lost-${run}.pdf`, makePdf(2, () => `This original goes missing ${run}`));
  await renditionOf(request, lost.id);
  const sha = (await get<{ sha256: string }>(request, api(`/sources/${lost.id}`))).sha256;
  const blobs = join(DATA_DIR, "vaults", vault, "blobs");
  const folder = readdirSync(blobs).find((d) => sha.startsWith(d));
  expect(folder, "the blob's folder").toBeDefined();
  const inFolder = join(blobs, folder ?? "");
  const file = readdirSync(inFolder, { recursive: true }).map(String).find((name) => name.includes(sha.slice(2)) || name.includes(sha));
  expect(file, "the blob").toBeDefined();
  rmSync(join(inFolder, file ?? ""));
  const asked = await request.get(api(`/sources/${lost.id}/pages/0/image`), { headers: bearer });
  record("the server on a missing blob", { status: asked.status(), body: (await asked.text()).slice(0, 200) });
  await page.goto(docsPath(`/${lost.id}`));
  const failure = page.locator(`[data-page-failure="1"]`);
  await expect(failure).toContainText("Couldn't show page 1.");
  await expect(failure).not.toContainText("Asking the server why.");
  record("the screen on a missing blob", await failure.textContent());

  // An erased source. Erasure is spec 030, which isn't built, so the row is removed from this database, and what the server then says is real. The screen says what a token can know.
  const gone = await upload(request, `gone-${run}.txt`, `Soon to be gone ${run}.`);
  const goneRendition = await renditionOf(request, gone.id);
  const goneSegment = (await segmentsOf(request, goneRendition.id))[0];
  const cited = await addClaim(request, `gone-${run}`, goneSegment?.id ?? "", "Soon to be gone");
  sql(`delete from "${schema}".sources where id = '${gone.id}'`);
  const noSource = await request.get(api(`/sources/${gone.id}`), { headers: bearer });
  record("the server on an erased source", { source: noSource.status(), lineage: (await get<{ evidence: unknown[] }>(request, api(`/assertions/${cited.claim}/lineage`))).evidence.length });
  await page.goto(docsPath(`/${gone.id}`));
  await expect(page.locator("[data-source-gone]")).toContainText("It may have been erased");
  await page.goto(docsPath(`/-/claims/${cited.claim}?evidence=${cited.evidence}`));
  await expect(page.locator("[data-evidence-state='missing']")).toContainText("the source it cited may have been erased");
  await page.goto(docsPath(`/${gone.id}?claim=${cited.claim}&evidence=${cited.evidence}`));
  await expect(page.locator("[data-source-gone]")).toBeVisible();

  // A type this browser can't play. Playwright's Chromium plays AAC and even PCM in Matroska, and refuses ALAC, so half a second of generated tone in ALAC goes through the real stream and the browser itself refuses it.
  const m4a = await upload(request, `tone-${run}.m4a`, readFileSync(join(here, "fixtures/tone-alac.m4a")));
  await page.goto(docsPath(`/${m4a.id}`));
  const problem = page.locator("[data-media-problem]");
  await expect(problem).toContainText("This recording can't be played here.");
  await expect(problem.getByRole("link", { name: "download the original" })).toHaveAttribute("href", new RegExp(`/sources/${m4a.id}/file$`));
  record("a type the browser can't play", await problem.textContent());
  // A browser that says so before trying, which is a different path. Its answer is scripted, and the stream is then never asked for.
  const streams: string[] = [];
  page.on("request", (r) => {
    if (r.url().endsWith("/stream")) streams.push(r.url());
  });
  await page.addInitScript(() => {
    HTMLMediaElement.prototype.canPlayType = () => "";
  });
  await page.goto(docsPath(`/${m4a.id}`));
  await expect(problem).toContainText("This browser has no player for audio/mp4.");
  expect(streams).toEqual([]);
  const { violations } = await new AxeBuilder({ page }).analyze();
  expect(violations.map((v) => `${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
});

test("a source not read yet says so, and a plain text source shows its text", async ({ page, request }) => {
  await signIn(page);
  const text = await upload(request, `plain-${run}.txt`, `Plain words ${run}.`);
  await renditionOf(request, text.id);
  await page.goto(docsPath(`/${text.id}`));
  await expect(page.getByText(`Plain words ${run}.`)).toBeVisible();
  // A PDF whose rendition isn't there yet is answered at the network, as it would otherwise race the real job.
  const pending = await upload(request, `pending-${run}.pdf`, makePdf(1, () => `Pending ${run}`));
  await page.route(`**/sources/${pending.id}/pages`, (route) => route.fulfill({ status: 404, json: { error: { code: "not_found", message: "not found", request_id: "req-pending" } } }));
  await page.goto(docsPath(`/${pending.id}`));
  await expect(page.getByText("This PDF hasn't been read yet.")).toBeVisible();
});
