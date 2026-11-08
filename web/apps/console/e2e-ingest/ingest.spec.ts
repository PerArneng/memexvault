import { execFileSync } from "node:child_process";
import { appendFileSync, closeSync, ftruncateSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";
import { makePdf } from "../e2e-documents/make-pdf";
import { Proxy } from "./proxy";
import { makeScannedPdf } from "./scanned-pdf";
import { StandInLlm } from "./stand-in-llm";

// Spec 041. A real server with the real CSP, through a proxy the tests can cut. The only stand-in is the LLM's server, on this machine.
const bind = process.env.MEMEX_HTTP_BIND ?? "";
const proxyPort = Number(bind.split(":")[1]) + 100;
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const direct = `http://${bind}`;
const RECORDING = process.env.MEMEX_E2E_RECORDING;
const RECORDING_BYTES = 300_000;
const PG_CONTAINER = process.env.MEMEX_E2E_PG_CONTAINER;
const run = Date.now().toString(36);
const proxy = new Proxy(bind, proxyPort);
const llm = new StandInLlm();

let vault = "";
let operatorToken = "";
let operatorId = "";
let readToken = "";

const headers = { authorization: `Bearer ${adminToken}` };
const url = (path: string) => `${direct}/api/v1/vaults/${vault}${path}`;
const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load averages:")[1]?.trim() ?? "unknown";
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));

const get = async <T>(request: APIRequestContext, path: string): Promise<T> => {
  const res = await request.get(url(path), { headers });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};

interface Doc { readonly source_id: string; readonly stage: string; readonly status: string; readonly job_id?: string | null; readonly pending_review: number }
const allDocs = async (request: APIRequestContext): Promise<readonly Doc[]> => {
  const out: Doc[] = [];
  let cursor = "";
  for (;;) {
    const page = await get<{ items: Doc[]; next_cursor?: string | null }>(request, `/activity/documents?limit=200${cursor === "" ? "" : `&cursor=${cursor}`}`);
    out.push(...page.items);
    if (!page.next_cursor) return out;
    cursor = page.next_cursor;
  }
};
const docOf = async (request: APIRequestContext, source: string): Promise<Doc> => {
  const found = (await allDocs(request)).find((d) => d.source_id === source);
  if (found === undefined) throw new Error(`no document ${source}`);
  return found;
};
const until = async <T>(what: string, minutes: number, look: () => Promise<T | null>): Promise<T> => {
  const deadline = Date.now() + minutes * 60_000;
  for (;;) {
    const value = await look();
    if (value !== null) return value;
    if (Date.now() > deadline) throw new Error(`waited ${String(minutes)} min for ${what}`);
    await pause(250);
  }
};

const uploadDirect = async (request: APIRequestContext, name: string, data: Buffer | string): Promise<string> => {
  const res = await request.post(url(`/sources?name=${encodeURIComponent(name)}`), { headers: { ...headers, "content-type": "application/octet-stream" }, data });
  expect(res.status(), await res.text()).toBeLessThan(300);
  return ((await res.json()) as { source: { id: string } }).source.id;
};

const approvePending = async (request: APIRequestContext) => {
  const { items } = await get<{ items: { id: string; state: string }[] }>(request, "/changesets?state=pending&limit=200");
  for (const c of items) {
    const res = await request.post(url(`/changesets/${c.id}/approve`), { headers, data: { reason: "the ingest checks" } });
    if (!res.ok() && res.status() !== 409) throw new Error(`approve: ${String(res.status())} ${await res.text()}`);
  }
  return items.length;
};

interface Attributed { readonly actor: string; readonly state: string }

/** Only this run's database, named by the same URL the server uses. It reads the log, as no route shows who an event came from. */
const actorsOf = (jobId: string): readonly Attributed[] => {
  const database = new URL(process.env.MEMEX_DATABASE_URL ?? "").pathname.slice(1);
  const psql = (statement: string) => execFileSync("docker", ["exec", PG_CONTAINER ?? "", "psql", "-U", "postgres", "-d", database, "-Atc", statement]).toString().trim();
  const schemas = psql("select table_schema from information_schema.tables where table_name = 'events'").split("\n").filter((s) => s !== "");
  for (const schema of schemas) {
    const rows = psql(`select actor, lower(payload::text) from ${schema}.events where payload::text like '%${jobId}%' order by seq`).split("\n").filter((r) => r !== "");
    if (rows.length === 0) continue;
    return rows.map((row) => {
      const split = row.indexOf("|");
      const text = row.slice(split + 1);
      const state = ["cancelled", "queued", "running", "done", "dead"].find((name) => new RegExp(`"state":\\s*"${name}"`).test(text)) ?? "?";
      return { actor: row.slice(0, split), state };
    });
  }
  return [];
};

const signIn = async (page: Page, token = adminToken) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
};
const board = (page: Page) => page.goto(`/vaults/${vault}/ingest`);
const live = (page: Page) => expect(page.locator('[data-connection="live"]')).toBeVisible();

const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") problems.push(m.text());
  });
  return problems;
};
const cspProblems = (problems: readonly string[]) => problems.filter((p) => /content security policy/i.test(p));

/** Every stage a row showed, as the DOM showed it, kept in the page across navigation inside the app. */
const watchRows = (page: Page) =>
  page.addInitScript(() => {
    const seen: Record<string, { stage: string; status: string }[]> = {};
    (window as unknown as { seenStages: typeof seen }).seenStages = seen;
    const note = (el: Element) => {
      const id = el.getAttribute("data-source-row");
      const stage = el.getAttribute("data-stage");
      const status = el.getAttribute("data-status") ?? "";
      if (id === null || stage === null) return;
      const list = (seen[id] ??= []);
      const last = list.at(-1);
      if (last?.stage !== stage || last.status !== status) list.push({ stage, status });
    };
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "attributes") note(r.target as Element);
        for (const n of r.addedNodes) {
          if (n.nodeType !== 1) continue;
          const el = n as Element;
          if (el.hasAttribute("data-source-row")) note(el);
          el.querySelectorAll("[data-source-row]").forEach(note);
        }
      }
    }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-stage", "data-status"] });
  });
const seenStages = (page: Page) => page.evaluate(() => (window as unknown as { seenStages: Record<string, { stage: string; status: string }[]> }).seenStages);

const ORDER = ["stored", "parsing", "ocr", "transcription", "segmenting", "indexing", "embedding", "extraction", "review", "done"];

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  process.env.MEMEX_LLM_PROVIDER = "openai";
  process.env.MEMEX_LLM_MODEL = "stand-in";
  process.env.MEMEX_LLM_BASE_URL = await llm.start();
  process.env.MEMEX_EXTRACT_TIMEOUT_S = "60";
  console.log(`server started in ${String(await startServer())} ms`);
  await proxy.start();
  const post = async <T>(path: string, data: unknown): Promise<T> => {
    const res = await request.post(`${direct}/api/v1${path}`, { headers, data });
    expect(res.status(), `${path}: ${await res.text()}`).toBe(201);
    return (await res.json()) as T;
  };
  vault = (await post<{ id: string }>("/vaults", { name: `ingest-${run}` })).id;
  const makeToken = async (name: string, scope: string) => {
    const principal = await post<{ id: string }>("/principals", { name: `${name}-${run}`, kind: "human" });
    return { id: principal.id, token: (await post<{ token: string }>("/tokens", { name, principal_id: principal.id, grants: [{ vault_id: vault, scope }] })).token };
  };
  const operator = await makeToken("operator", "propose");
  operatorToken = operator.token;
  operatorId = operator.id;
  readToken = (await makeToken("reader", "read")).token;
});

test.afterAll(async () => {
  await proxy.stop();
  await stopServer();
  await llm.stop();
});

test.beforeEach(() => {
  proxy.setScript(null);
  proxy.refuse(false);
  proxy.slowUploads(null);
  llm.up = true;
  llm.delayMs = 0;
});

test("item 1: a digital PDF, a scanned PDF and a recording show every stage in order", async ({ page, browser, request }) => {
  test.skip(RECORDING === undefined, "set MEMEX_E2E_RECORDING to a short recording, and MEMEX_MODELS_DIR with whisper");
  const problems = problemsOf(page);
  await watchRows(page);
  await signIn(page);
  await board(page);
  await live(page);
  await expect(page.getByText("There's nothing in this vault yet.")).toBeVisible();

  const scanned = await makeScannedPdf(browser, ["The harbour master recorded a storm in the north channel during the autumn of that year.", "Three ships returned safely to the old port before the tide turned against them."]);
  const digital = makePdf(2, (n) => `Item one ${run} digital page ${String(n + 1)} about the harbour`);
  // Each call takes a moment, as a model's would, so extraction can be seen.
  llm.delayMs = 4000;
  const started = Date.now();
  await page.getByLabel("Files to upload").setInputFiles([
    { name: `digital-${run}.pdf`, mimeType: "application/pdf", buffer: digital },
    { name: `scanned-${run}.pdf`, mimeType: "application/pdf", buffer: scanned },
    // MP3 frames stand alone, so the start of a long recording is a short one. Rendition jobs over 60 s lose their lease until spec 82 lands.
    { name: `recording-${run}.mp3`, mimeType: "audio/mpeg", buffer: readFileSync(RECORDING ?? "").subarray(0, RECORDING_BYTES) },
  ]);

  const items = page.getByRole("list", { name: "Upload queue" });
  for (const name of [`digital-${run}.pdf`, `scanned-${run}.pdf`, `recording-${run}.mp3`]) await expect(items.locator(`[data-upload-name="${name}"]`)).toHaveAttribute("data-upload-state", "stored");

  // Review needs a person, so each document is approved only once the board has shown it there.
  const ids = (await allDocs(request)).map((d) => d.source_id);
  expect(ids.length).toBe(3);
  const row = (id: string) => page.locator(`[data-source-row="${id}"]`);
  await until("all three documents to finish", 10, async () => {
    const shown = await Promise.all(ids.map((id) => row(id).getAttribute("data-stage")));
    if (shown.includes("review")) await approvePending(request);
    const docs = await allDocs(request);
    return docs.every((d) => d.stage === "done") ? true : null;
  });
  const took = Date.now() - started;
  for (const id of ids) await expect(row(id)).toHaveAttribute("data-stage", "done");

  const seen = await seenStages(page);
  const names = new Map(await Promise.all(ids.map(async (id) => [id, (await get<{ name: string }>(request, `/sources/${id}`)).name] as const)));
  const wanted = (name: string) => (name.startsWith("recording") ? ["stored", "transcription", "indexing", "embedding", "extraction", "review", "done"] : name.startsWith("scanned") ? ["stored", "parsing", "ocr", "segmenting", "indexing", "embedding", "extraction", "review", "done"] : ["stored", "parsing", "segmenting", "indexing", "embedding", "extraction", "review", "done"]);
  const timelineOf = () => page.locator("[data-timeline-stage]").evaluateAll((els) => els.map((el) => ({ stage: el.getAttribute("data-timeline-stage") ?? "", state: el.getAttribute("data-step-state") ?? "" })));
  const evidence: Record<string, unknown> = {};
  for (const id of ids) {
    const name = names.get(id) ?? "";
    const stages = (seen[id] ?? []).map((s) => s.stage);
    // Live, on the board: never backwards, and it reaches every stage a person has to act on or wait for.
    const indexes = stages.map((s) => ORDER.indexOf(s));
    expect(indexes, `${name} moved backwards: ${stages.join(" > ")}`).toEqual([...indexes].sort((a, b) => a - b));
    for (const stage of ["embedding", "extraction", "review", "done"]) expect(stages, `${name} never showed ${stage}: ${stages.join(" > ")}`).toContain(stage);

    // Opened from the board in the same session, the timeline names every stage in order, and which were seen happen.
    await page.getByRole("link", { name }).click();
    await expect(page.locator("[data-timeline-stage]").last()).toHaveAttribute("data-step-state", "current");
    // A digital page gets OCR too when it's sampled for hidden text, so it may be seen here, though nothing durable keeps that.
    const without = (list: readonly string[]) => (name.startsWith("digital") ? list.filter((s) => s !== "ocr") : list).join();
    await expect.poll(async () => without((await timelineOf()).map((s) => s.stage)), { message: name }).toBe(without(wanted(name)));
    const inSession = await timelineOf();
    expect(inSession.every((s) => s.stage === "done" ? s.state === "current" : s.state === "seen" || s.state === "passed"), JSON.stringify(inSession)).toBe(true);
    await page.getByRole("link", { name: "Back to the board" }).click();

    evidence[name] = { showedOnBoard: stages, timeline: inSession.map((s) => `${s.stage}:${s.state}`) };
  }
  // A reload knows the same way through, from what's durable, though it saw none of it.
  for (const id of ids) {
    await page.goto(`/vaults/${vault}/ingest/${id}`);
    await expect.poll(async () => (await timelineOf()).map((s) => s.stage).join()).toBe(wanted(names.get(id) ?? "").join());
  }
  record("item 1", { evidence, tookMs: took, load: loadAverage(), llmCalls: llm.calls });
  expect(cspProblems(problems)).toEqual([]);
});

test("the live view says in plain words when it waits for the LLM, and carries on by itself", async ({ page, request }) => {
  await signIn(page);
  await board(page);
  await live(page);
  llm.up = false;
  const source = await uploadDirect(request, `waits-${run}.txt`, `This text waits for the language model ${run}. `.repeat(20));
  await page.goto(`/vaults/${vault}/ingest/${source}`);
  const view = page.locator(`[data-document-activity="${source}"]`);
  await expect(view).toHaveAttribute("data-status", "waiting", { timeout: 180_000 });
  await expect(view.locator('[data-explanation="waiting"]')).toContainText("Waiting for the LLM");
  await expect(view.locator('[data-explanation="waiting"]')).toContainText("Nothing has failed");
  await expect(view).toHaveAttribute("data-stage", "extraction");
  llm.up = true;
  await expect(view).not.toHaveAttribute("data-status", "waiting", { timeout: 300_000 });
  await until("the waiting document to reach review", 5, async () => ((await docOf(request, source)).stage === "review" ? true : null));
  await approvePending(request);
  await expect(view).toHaveAttribute("data-stage", "done");
});

test("item 3: killing the connection mid-run recovers with no reload, and counts match the API", async ({ page, request }) => {
  const problems = problemsOf(page);
  let navigations = 0;
  await signIn(page);
  await board(page);
  await live(page);
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) navigations += 1;
  });
  await page.evaluate(() => {
    (window as unknown as { sentinel: string }).sentinel = "same document";
  });

  const countsOnScreen = () => page.locator("[data-stage-count]").evaluateAll((els) => Object.fromEntries(els.map((el) => [el.getAttribute("data-stage-count") ?? "", Number(el.getAttribute("data-count"))]).filter(([, n]) => n !== 0)) as Record<string, number>);
  const countsFromApi = async () => {
    const out: Record<string, number> = {};
    for (const d of await allDocs(request)) out[d.stage] = (out[d.stage] ?? 0) + 1;
    return out;
  };
  const sorted = (o: Record<string, number>) => JSON.stringify(Object.entries(o).sort());

  const before = (await allDocs(request)).length;
  const mine: string[] = [];
  const results: unknown[] = [];
  for (let round = 0; round < 3; round += 1) {
    for (let i = 0; i < 5; i += 1) mine.push(await uploadDirect(request, `kill-${run}-${String(round)}-${String(i)}.txt`, `Round ${String(round)} document ${String(i)} of ${run} about the harbour. `.repeat(10)));
    // Mid-run: the work has started and isn't over.
    await until("frames to reach the board", 2, async () => ((await page.locator("[data-document-count]").getAttribute("data-document-count")) === String(before + mine.length) ? true : null));
    const positionBefore = Number(await page.locator("[data-applied-seq]").getAttribute("data-applied-seq"));
    const cut = Date.now();
    proxy.refuse(true);
    const killed = proxy.kill();
    await expect(page.locator('[data-connection="reconnecting"]')).toBeVisible();
    await expect(page.locator("[data-connection-detail]")).toContainText("reconnecting by itself");
    const shownAfter = Date.now() - cut;
    // Work goes on, and it's the stream's job to catch up. These go straight to the server, past the cut.
    mine.push(await uploadDirect(request, `kill-${run}-${String(round)}-during.txt`, `Uploaded during cut ${String(round)} of ${run}. `.repeat(10)));
    await pause(4000);
    proxy.refuse(false);
    await live(page);
    const recoveredAfter = Date.now() - cut;
    const resumedFrom = proxy.resumes.at(-1) ?? "";
    expect(Number(resumedFrom)).toBeGreaterThanOrEqual(positionBefore - 1);
    await expect(page.locator("[data-connection-detail]")).toHaveCount(0);
    results.push({ round, killed, indicatorMs: shownAfter, liveAgainMs: recoveredAfter, positionBefore, resumedFrom });
  }
  // Everything finishes, and the board has to get to the server's answer by itself.
  await until("the run to finish", 10, async () => {
    await approvePending(request);
    const docs = await allDocs(request);
    return docs.every((d) => d.stage === "done" || d.stage === "review") && docs.length === before + mine.length ? true : null;
  });
  await approvePending(request);
  await until("every document to be done", 5, async () => ((await allDocs(request)).every((d) => d.stage === "done") ? true : null));
  await expect.poll(async () => sorted(await countsOnScreen()), { timeout: 60_000 }).toBe(sorted(await countsFromApi()));
  const finalCounts = await countsFromApi();
  expect(await page.evaluate(() => (window as unknown as { sentinel?: string }).sentinel)).toBe("same document");
  expect(navigations).toBe(0);
  record("item 3", { rounds: results, documents: before + mine.length, screen: await countsOnScreen(), api: finalCounts, navigations, load: loadAverage() });
  expect(cspProblems(problems)).toEqual([]);
});

test("item 4: retry and cancel from the UI, attributed to the signed-in principal", async ({ page, browser, request }) => {
  test.skip(PG_CONTAINER === undefined, "set MEMEX_E2E_PG_CONTAINER, as only the log can show who acted");
  const problems = problemsOf(page);
  await signIn(page, operatorToken);
  const who = `operator-${run}`;

  // A running job: pages of pixels, so OCR takes a while, and cancel comes during it.
  const pages = Array.from({ length: 6 }, (_, i) => `Page ${String(i + 1)} of a scan about the old lighthouse and its keepers, written out in full for the OCR to read.`);
  const scan = await uploadDirect(request, `cancel-${run}.pdf`, await makeScannedPdf(browser, pages));
  await page.goto(`/vaults/${vault}/ingest/${scan}`);
  const view = page.locator(`[data-document-activity="${scan}"]`);
  await expect(page.locator(`[data-progress-text^="OCR page"]`).first()).toBeVisible({ timeout: 180_000 });
  const progressShown = await page.locator("[data-progress-text]").first().getAttribute("data-progress-text");
  const card = page.locator('[data-job-state="running"]');
  await expect(card).toBeVisible();
  const jobId = (await card.getAttribute("data-job")) ?? "";
  await card.getByRole("button", { name: "Cancel job" }).click();
  await card.getByRole("button", { name: "Yes, cancel the job" }).click();
  await expect(page.locator(`[data-job="${jobId}"]`)).toHaveAttribute("data-job-state", "cancelled");
  await expect(page.locator(`[data-job="${jobId}"]`)).toContainText(`Cancelled by ${who}.`);
  await expect(view).toHaveAttribute("data-status", "failed");
  const cancelledAt = Date.now();

  // It stops: what it was doing never lands, however long the worker takes to notice.
  await pause(20_000);
  const after = await get<{ state: string }>(request, `/jobs/${jobId}`);
  expect(after.state).toBe("cancelled");
  const { items: renditions } = await get<{ items: unknown[] }>(request, `/sources/${scan}/renditions`);
  expect(renditions).toEqual([]);

  // A cancelled job can be retried, and then it finishes.
  await page.locator(`[data-job="${jobId}"]`).getByRole("button", { name: "Retry job" }).click();
  await page.locator(`[data-job="${jobId}"]`).getByRole("button", { name: "Yes, retry" }).click();
  await expect(page.locator(`[data-job="${jobId}"]`)).toContainText(`Retried by ${who}.`);
  await until("the retried job to finish", 5, async () => ((await get<{ state: string }>(request, `/jobs/${jobId}`)).state === "done" ? true : null));

  // A dead job: bytes that say they're a PDF and aren't, and it fails for good.
  const broken = await uploadDirect(request, `broken-${run}.pdf`, `%PDF-1.4\nthis is not really a PDF ${run}\n`);
  const brokenDoc = await until("the broken document to fail", 5, async () => {
    const d = await docOf(request, broken);
    return d.status === "failed" && d.job_id ? d : null;
  });
  const deadJob = brokenDoc.job_id ?? "";
  await page.goto(`/vaults/${vault}/ingest/${broken}`);
  const dead = page.locator(`[data-job="${deadJob}"]`);
  await expect(dead).toHaveAttribute("data-job-state", "dead");
  await expect(dead.locator("[data-attempt] pre").first()).toBeVisible();
  await expect(page.locator('[data-explanation="failed"]')).toContainText("you can retry it");
  const attemptsBefore = await dead.locator("[data-attempt]").count();
  await dead.getByRole("button", { name: "Retry job" }).click();
  await dead.getByRole("button", { name: "Yes, retry" }).click();
  await expect(dead).toContainText(`Retried by ${who}.`);
  await until("the second try to fail", 5, async () => ((await get<{ state: string; attempts: unknown[] }>(request, `/jobs/${deadJob}`)).attempts.length > attemptsBefore ? true : null));
  await until("it to be dead again", 5, async () => ((await get<{ state: string }>(request, `/jobs/${deadJob}`)).state === "dead" ? true : null));

  // The dead-letter list and its bulk retry.
  await page.goto(`/vaults/${vault}/ingest/-/failed`);
  await expect(page.locator(`[data-failed-job="${deadJob}"]`)).toBeVisible();
  await page.getByLabel(`Select job ${deadJob}`).check();
  await page.getByRole("button", { name: /Retry selected \(1\)/ }).click();
  await page.getByRole("button", { name: "Yes, retry 1" }).click();
  await expect(page.getByText(`Retried 1 by ${who}.`)).toBeVisible();

  const scanEvents = actorsOf(jobId);
  const deadEvents = actorsOf(deadJob);
  const byOperator = (events: readonly Attributed[], state: string) => events.filter((e) => e.state === state && e.actor === operatorId).length;
  expect(byOperator(scanEvents, "cancelled"), JSON.stringify(scanEvents)).toBe(1);
  expect(scanEvents.filter((e) => e.actor === operatorId).length, JSON.stringify(scanEvents)).toBe(2);
  expect(deadEvents.filter((e) => e.actor === operatorId).length, JSON.stringify(deadEvents)).toBe(2);
  record("item 4", { progressShown, cancelledAfterMs: Date.now() - cancelledAt, operatorId, cancelEvents: scanEvents.filter((e) => e.actor === operatorId), retryEvents: deadEvents.filter((e) => e.actor === operatorId), load: loadAverage() });
  expect(cspProblems(problems)).toEqual([]);
});

test("item 6: an oversized file, a disallowed type, a lost connection during upload, a duplicate", async ({ page }) => {
  const problems = problemsOf(page);
  await signIn(page, operatorToken);
  await board(page);
  await live(page);
  const files = page.getByLabel("Files to upload");
  const item = (name: string) => page.locator(`[data-upload-name="${name}"]`);

  // Sparse, so it takes no disk. It's refused from its size alone, and nothing reads it.
  const folder = mkdtempSync(join(tmpdir(), "memex-ingest-"));
  try {
    const big = join(folder, `big-${run}.pdf`);
    const handle = openSync(big, "w");
    ftruncateSync(handle, 3 * 1024 ** 3);
    closeSync(handle);
    let sent = 0;
    page.on("request", (r) => {
      if (r.method() === "POST" && r.url().includes("/sources?")) sent += 1;
    });
    await files.setInputFiles(big);
    await expect(item(`big-${run}.pdf`)).toHaveAttribute("data-upload-state", "refused");
    await expect(item(`big-${run}.pdf`)).toContainText("This is 3.0 GB, and the most this server takes is 2.0 GB.");
    expect(sent).toBe(0);

    // What the browser can tell, it says at once. What it can't goes to the server, whose answer shows.
    await files.setInputFiles({ name: `archive-${run}.zip`, mimeType: "application/zip", buffer: Buffer.from("PK\x03\x04 not allowed") });
    await expect(item(`archive-${run}.zip`)).toHaveAttribute("data-upload-state", "refused");
    await expect(item(`archive-${run}.zip`)).toContainText("doesn't take application/zip");
    expect(sent).toBe(0);
    await files.setInputFiles({ name: `blob-${run}.bin`, mimeType: "application/octet-stream", buffer: Buffer.from([0, 1, 2, 3, 0, 255, 0, 254]) });
    await expect(item(`blob-${run}.bin`)).toHaveAttribute("data-upload-state", "failed");
    await expect(item(`blob-${run}.bin`)).toContainText("doesn't take its type");
    expect(sent).toBe(1);
  } finally {
    rmSync(folder, { recursive: true });
  }

  // Slowed down, so the cut falls in the middle of the bytes, and there's a bar to see.
  const body = Buffer.from(`A long text that is cut off on its way ${run}. `.repeat(200_000));
  const name = `lost-${run}.txt`;
  proxy.slowUploads(1024 * 1024);
  await files.setInputFiles({ name, mimeType: "text/plain", buffer: body });
  const bar = item(name).getByRole("progressbar");
  await expect(bar).toBeVisible();
  await expect.poll(async () => Number(await bar.getAttribute("aria-valuenow"))).toBeGreaterThan(0);
  const reached = Number(await bar.getAttribute("aria-valuenow"));
  expect(reached).toBeLessThan(body.length);
  // Refused as well as cut, as a browser sends a request again by itself when a kept-alive connection drops and a new one works.
  proxy.refuse(true);
  proxy.kill();
  await expect(item(name)).toHaveAttribute("data-upload-state", "failed");
  await expect(item(name)).toContainText("The connection was lost during the upload");
  proxy.refuse(false);
  const stored = async () => (await page.request.get(`/api/v1/vaults/${vault}/sources?limit=200`).then((r) => r.json()) as { items: { name: string }[] }).items.filter((s) => s.name === name).length;
  await live(page);
  expect(await stored()).toBe(0);
  // Nothing's lost but the bytes on the way: the same file goes again.
  proxy.slowUploads(null);
  await page.getByRole("button", { name: `Retry uploading ${name}` }).click();
  await expect(item(name)).toHaveAttribute("data-upload-state", "stored");
  expect(await stored()).toBe(1);
  record("item 6 lost upload", { size: body.length, reachedBeforeCut: reached });

  // The same bytes again.
  await files.setInputFiles({ name: `again-${run}.txt`, mimeType: "text/plain", buffer: body });
  await expect(item(`again-${run}.txt`)).toHaveAttribute("data-upload-state", "duplicate");
  await expect(item(`again-${run}.txt`)).toContainText(`already in this vault as ${name}`);

  // Trust tier goes with the file.
  await page.getByRole("radio", { name: "Trusted" }).check();
  await files.setInputFiles({ name: `tier-${run}.txt`, mimeType: "text/plain", buffer: Buffer.from(`Trusted text ${run}. `.repeat(10)) });
  await expect(item(`tier-${run}.txt`)).toHaveAttribute("data-upload-state", "stored");
  const listed = (await page.request.get(`/api/v1/vaults/${vault}/sources?limit=200`).then((r) => r.json()) as { items: { name: string; trust_tier: string }[] }).items.find((s) => s.name === `tier-${run}.txt`);
  expect(listed?.trust_tier).toBe("trusted");
  expect(cspProblems(problems)).toEqual([]);
});

test("item 6: a read-scope user sees the view with no controls", async ({ page, request }) => {
  const problems = problemsOf(page);
  const source = await uploadDirect(request, `read-${run}.txt`, `For the reader ${run}. `.repeat(10));
  await signIn(page, readToken);
  await board(page);
  await expect(page.getByRole("heading", { name: "Ingest" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Ingest" })).toBeVisible();
  await live(page);
  await expect(page.locator("[data-no-upload]")).toContainText("propose");
  await expect(page.locator(`[data-source-row="${source}"]`)).toBeVisible();
  await expect(page.getByLabel("Files to upload")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Choose files" })).toHaveCount(0);

  await page.goto(`/vaults/${vault}/ingest/${source}`);
  await expect(page.locator(`[data-document-activity="${source}"]`)).toBeVisible();
  await expect(page.locator("[data-job]").first()).toBeVisible();
  await expect(page.getByRole("button", { name: /Cancel job|Retry job/ })).toHaveCount(0);

  await page.goto(`/vaults/${vault}/ingest/-/failed`);
  await expect(page.getByRole("heading", { name: "Failed jobs" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Retry selected/ })).toHaveCount(0);
  await expect(page.getByRole("checkbox")).toHaveCount(0);

  // The server says the same, which is what actually counts.
  const refused = await page.request.post(`/api/v1/vaults/${vault}/sources?name=x.txt`, { headers: { "x-memex-csrf": "1", "content-type": "application/octet-stream" }, data: "no" });
  expect(refused.status()).toBe(403);
  expect(cspProblems(problems)).toEqual([]);
});

const percentile = (values: readonly number[], p: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] ?? Number.NaN;
};

test("item 2: 50 events a second for 5 minutes", async ({ page }) => {
  test.skip(process.env.MEMEX_E2E_LOAD === undefined, "set MEMEX_E2E_LOAD=1: it takes over 5 minutes");
  const SECONDS = Number(process.env.MEMEX_E2E_LOAD_SECONDS ?? 300);
  const DOCUMENTS = 2000;
  const ON_SCREEN = 12;
  test.setTimeout((SECONDS + 240) * 1000);
  const id = (i: number) => `doc-${String(i).padStart(5, "0")}`;
  const now0 = Date.now();
  // The newest come first on the board, so these are on screen.
  const docs = Array.from({ length: DOCUMENTS }, (_, i) => ({
    source_id: id(i), logical_id: id(i), version_no: 1, recorded_at_ms: now0 - i * 1000, stage: "ocr", status: "active", partial: false, attempts: 1, pending_review: 0,
    job_id: `job-${String(i).padStart(5, "0")}`, progress: { stage: "ocr", done: 0, total: 1_000_000, time_ms: now0 } as { stage: string; done: number; total: number; time_ms: number },
  }));
  let seq = 100;
  const sentProgress = new Map<string, number>();
  const sentDurable = new Map<number, number>();
  const counters = { written: 0, snapshots: 0, streams: 0 };
  const timers: NodeJS.Timeout[] = [];
  proxy.setScript((req, res) => {
    const path = req.url ?? "";
    if (path.includes("/activity/documents")) {
      counters.snapshots += 1;
      const query = new URL(path, "http://x").searchParams;
      const start = Number(query.get("cursor") ?? 0);
      const limit = Number(query.get("limit") ?? 50);
      const next = start + limit < docs.length ? String(start + limit) : null;
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ head: seq, items: docs.slice(start, start + limit), next_cursor: next }));
      return true;
    }
    if (!/\/activity(\?|$)/.test(path)) return false;
    counters.streams += 1;
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
    res.write(`event: hello\ndata: ${JSON.stringify({ head: seq, resumed: true })}\n\n`);
    res.socket?.setNoDelay(true);
    const started = Date.now();
    let emitted = 0;
    const emit = () => {
      const n = emitted;
      emitted += 1;
      const slot = n % 50;
      const at = Date.now();
      if (slot < 45) {
        // Thirty a second on screen and fifteen off it.
        const doc = docs[slot < 30 ? slot % ON_SCREEN : ON_SCREEN + 50 + (n % (DOCUMENTS - ON_SCREEN - 50))];
        if (doc === undefined) return;
        const done = doc.progress.done + 1;
        doc.progress = { stage: "ocr", done, total: 1_000_000, time_ms: at };
        if (slot < 30) sentProgress.set(`${doc.source_id}:${String(done)}`, at);
        res.write(`event: progress\ndata: ${JSON.stringify({ job_id: doc.job_id, source_id: doc.source_id, stage: "ocr", done, total: 1_000_000, time_ms: at })}\n\n`);
      } else {
        seq += 1;
        const doc = docs[ON_SCREEN + 50 + (n % (DOCUMENTS - ON_SCREEN - 50))];
        if (doc === undefined) return;
        const job = { id: doc.job_id, kind: "rendition", state: "running", stage: "parsing", status: "active", attempts: 1, max_attempts: 3, next_run_at_ms: 0, source_id: doc.source_id };
        const frame = { seq, time_ms: at, kind: "state.recorded", jobs: [job], counts: slot === 45 ? { job: 1, changeset: 1 } : { job: 1 } };
        sentDurable.set(seq, at);
        res.write(`id: ${String(seq)}\nevent: durable\ndata: ${JSON.stringify(frame)}\n\n`);
      }
      counters.written += 1;
    };
    const timer = setInterval(() => {
      const owed = Math.floor(((Date.now() - started) / 1000) * 50);
      while (emitted < owed) emit();
    }, 20);
    timers.push(timer);
    res.on("close", () => clearInterval(timer));
    return true;
  });

  await page.addInitScript(() => {
    const seen: [string, number][] = [];
    const inputs: number[] = [];
    const w = window as unknown as { drain: () => unknown };
    w.drain = () => ({ seen: seen.splice(0), inputs: inputs.splice(0) });
    // After the frame that holds the change.
    const afterPaint = (run: () => void) => requestAnimationFrame(() => {
      const channel = new MessageChannel();
      channel.port1.onmessage = run;
      channel.port2.postMessage(0);
    });
    new MutationObserver((records) => {
      const keys: string[] = [];
      for (const r of records) {
        const el = r.target as Element;
        const row = el.getAttribute("data-source-row");
        if (row !== null) keys.push(`${row}:${el.getAttribute("data-done") ?? ""}`);
        const applied = el.getAttribute("data-applied-seq");
        if (applied !== null) keys.push(`seq:${applied}`);
      }
      if (keys.length > 0) afterPaint(() => {
        const at = Date.now();
        for (const key of keys) seen.push([key, at]);
      });
    }).observe(document, { subtree: true, attributes: true, attributeFilter: ["data-done", "data-applied-seq"] });
    document.addEventListener("input", (e) => {
      const from = e.timeStamp;
      afterPaint(() => inputs.push(performance.now() - from));
    }, true);
  });

  await signIn(page);
  const problems = problemsOf(page);
  await board(page);
  await live(page);
  await expect(page.locator("[data-document-count]")).toHaveAttribute("data-document-count", String(DOCUMENTS));
  const onScreen = await page.locator("[data-source-row]").evaluateAll((els) => els.map((el) => el.getAttribute("data-source-row")));
  for (let i = 0; i < ON_SCREEN; i += 1) expect(onScreen).toContain(id(i));

  const cdp = await page.context().newCDPSession(page);
  const heap = async () => {
    await cdp.send("HeapProfiler.collectGarbage");
    return (await cdp.send("Runtime.getHeapUsage")).usedSize;
  };
  const filter = page.getByLabel("Filter by name or id");
  await filter.focus();

  const shown = new Map<string, number>();
  const firstShown: { doc: string; done: number; at: number }[] = [];
  const seqShown: { seq: number; at: number }[] = [];
  const inputs: number[] = [];
  const drain = async () => {
    const batch = (await page.evaluate(() => (window as unknown as { drain: () => unknown }).drain())) as { seen: [string, number][]; inputs: number[] };
    inputs.push(...batch.inputs);
    for (const [key, at] of batch.seen) {
      if (shown.has(key)) continue;
      shown.set(key, at);
      if (key.startsWith("seq:")) seqShown.push({ seq: Number(key.slice(4)), at });
      else {
        const split = key.lastIndexOf(":");
        firstShown.push({ doc: key.slice(0, split), done: Number(key.slice(split + 1)), at });
      }
    }
  };

  const heaps: { minute: number; bytes: number; load: string }[] = [];
  const started = Date.now();
  let typed = false;
  let lastDrain = started;
  let nextMinute = 1;
  while (Date.now() - started < SECONDS * 1000) {
    await page.keyboard.press(typed ? "Backspace" : "d");
    typed = !typed;
    await pause(500);
    if (Date.now() - lastDrain > 5000) {
      await drain();
      lastDrain = Date.now();
    }
    if ((Date.now() - started) / 60_000 >= nextMinute) {
      heaps.push({ minute: nextMinute, bytes: await heap(), load: loadAverage() });
      nextMinute += 1;
    }
  }
  await pause(3000);
  await drain();
  const wrote = counters.written;
  for (const t of timers) clearInterval(t);
  const elapsedS = (Date.now() - started) / 1000;

  // A later value on a row or a later position covers everything before it.
  const latency: number[] = [];
  let missing = 0;
  const byDoc = new Map<string, { done: number; at: number }[]>();
  for (const s of firstShown) (byDoc.get(s.doc) ?? byDoc.set(s.doc, []).get(s.doc))?.push(s);
  for (const list of byDoc.values()) list.sort((a, b) => a.done - b.done);
  for (const [key, sentAt] of sentProgress) {
    const split = key.lastIndexOf(":");
    const done = Number(key.slice(split + 1));
    const hit = byDoc.get(key.slice(0, split))?.find((s) => s.done >= done);
    if (hit === undefined) missing += 1;
    else latency.push(hit.at - sentAt);
  }
  seqShown.sort((a, b) => a.seq - b.seq);
  for (const [position, sentAt] of sentDurable) {
    const hit = seqShown.find((s) => s.seq >= position);
    if (hit === undefined) missing += 1;
    else latency.push(hit.at - sentAt);
  }
  const report = {
    seconds: Math.round(elapsedS),
    framesWritten: wrote,
    framesPerSecond: Math.round((wrote / elapsedS) * 10) / 10,
    documents: DOCUMENTS,
    streamsOpened: counters.streams,
    snapshotPagesServed: counters.snapshots,
    appearance: { samples: latency.length, missing, p50: percentile(latency, 50), p95: percentile(latency, 95), p99: percentile(latency, 99), max: Math.max(...latency) },
    input: { samples: inputs.length, p50: Math.round(percentile(inputs, 50)), p95: Math.round(percentile(inputs, 95)), p99: Math.round(percentile(inputs, 99)), max: Math.round(Math.max(...inputs)) },
    heaps,
    heapGrowth: (heaps.at(-1)?.bytes ?? 0) - (heaps[0]?.bytes ?? 0),
    load: loadAverage(),
    csp: cspProblems(problems).length,
  };
  record("item 2", report);
  proxy.setScript(null);
  const MIB = 1024 * 1024;
  expect.soft(report.streamsOpened, "the stream stayed open").toBe(1);
  expect.soft(missing, "every frame appeared").toBe(0);
  expect.soft(report.appearance.p95, "event appearance p95").toBeLessThanOrEqual(1000);
  expect.soft(report.input.p95, "input response p95").toBeLessThanOrEqual(100);
  expect.soft(Math.max(...heaps.map((h) => h.bytes)), "heap at every sample").toBeLessThanOrEqual(64 * MIB);
  expect.soft(report.heapGrowth, "heap growth from minute 1").toBeLessThanOrEqual(5 * MIB);
});
