import { appendFileSync, readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";
import { makeTextPdf } from "../e2e-search/golden-pdf";
import { Canary, StandInChat } from "./stand-in-chat";

// Spec 050, on a real server with the real CSP, the real ADK runner and the real tools. The model is a stand-in on this machine that follows a
// script, so these say what the console does with what a run sends, and nothing about what a model chooses to do.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const model = new StandInChat();
const canary = new Canary();

const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load average")[1]?.replace(/^s?:\s*/, "") ?? "unknown";

interface Person { readonly token: string; readonly id: string }
interface World {
  readonly vault: string;
  readonly admin: Person;
  readonly proposer: Person;
  readonly reader: Person;
  readonly pdf: string;
  /** The PDF's segments, one a page. */
  readonly pages: readonly { readonly id: string; readonly page: number }[];
}
let world: World;

const send = async <T>(request: APIRequestContext, path: string, data: unknown, status = 200, method: "POST" | "PUT" = "POST"): Promise<T> => {
  const res = method === "POST" ? await request.post(path, { headers: bearer, data }) : await request.put(path, { headers: bearer, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const get = async <T>(request: APIRequestContext, path: string, token = adminToken): Promise<T> => {
  const res = await request.get(path, { headers: { authorization: `Bearer ${token}` } });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};
const api = (path: string) => `/api/v1/vaults/${world.vault}${path}`;

const segmentsOf = async (request: APIRequestContext, vault: string, source: string, at_least: number) => {
  let found: { id: string; text: string; page_from?: number | null }[] = [];
  await expect.poll(async () => {
    const renditions = await get<{ items: { id: string; superseded_by?: string | null }[] }>(request, `/api/v1/vaults/${vault}/sources/${source}/renditions`);
    const rendition = renditions.items.find((r) => !r.superseded_by);
    if (rendition === undefined) return 0;
    found = (await get<{ items: typeof found }>(request, `/api/v1/vaults/${vault}/renditions/${rendition.id}/segments?limit=200`)).items;
    return found.length;
  }, { timeout: 300_000, intervals: [500] }).toBeGreaterThanOrEqual(at_least);
  return found;
};

const SPAN = (n: number) => `Span ${String(n)} of the ${run} viaduct measures ${String(100 + n)} metres and its piers are painted colour number ${String(n)}.`;

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  const llm = await model.start();
  await canary.start();
  // Extraction is on with these too, and the stand-in answers it with no claims.
  Object.assign(process.env, { MEMEX_LLM_PROVIDER: "openai", MEMEX_LLM_BASE_URL: llm, MEMEX_LLM_MODEL: "stand-in-extract", MEMEX_CHAT_MODEL: "stand-in-chat", MEMEX_CHAT_TIMEOUT_S: "120" });
  console.log(`server started in ${String(await startServer())} ms`);
  const vault = await send<{ id: string }>(request, "/api/v1/vaults", { name: `chat-${run}` }, 201);
  const make = async (name: string, scope: string): Promise<Person> => {
    const p = await send<{ id: string }>(request, "/api/v1/principals", { name: `${name}-${run}`, kind: "human" }, 201);
    const t = await send<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: p.id, grants: [{ vault_id: vault.id, scope }] }, 201);
    return { token: t.token, id: p.id };
  };
  const admin = await make("chat-admin", "admin");
  const proposer = await make("chat-proposer", "propose");
  const reader = await make("chat-reader", "read");
  const upload = async (name: string, body: string | Buffer) => {
    const res = await request.post(`/api/v1/vaults/${vault.id}/sources?name=${encodeURIComponent(name)}`, { headers: bearer, data: body });
    expect(res.status(), await res.text()).toBe(201);
    return ((await res.json()) as { source: { id: string } }).source.id;
  };
  const pdf = await upload(`viaduct-${run}.pdf`, makeTextPdf(Array.from({ length: 12 }, (_, n) => SPAN(n + 1))));
  const segments = await segmentsOf(request, vault.id, pdf, 10);
  const pages = segments.filter((s) => typeof s.page_from === "number").map((s) => ({ id: s.id, page: s.page_from ?? 0 }));
  record("setup: the PDF's segments", { segments: segments.length, onPages: pages.length });
  world = { vault: vault.id, admin, proposer, reader, pdf, pages };
});

test.afterAll(async () => {
  await stopServer();
  await model.stop();
  await canary.stop();
});

const enableChat = (request: APIRequestContext, enabled: boolean) => send(request, api("/chat/settings"), { enabled }, 200, "PUT");

const signIn = async (page: Page, token: string) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/vaults\//, { timeout: 60_000 });
};

const newSession = async (browser: Browser, token: string): Promise<{ context: BrowserContext; page: Page }> => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: `http://${process.env.MEMEX_HTTP_BIND ?? ""}`, viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  await signIn(page, token);
  return { context, page };
};

/** Anything the browser reports as wrong, including a blocked script or style, which is a console error. */
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error" && !/40[0-9]/.test(m.text())) problems.push(m.text());
  });
  page.on("dialog", (d) => {
    problems.push(`dialog: ${d.message()}`);
    void d.dismiss();
  });
  return problems;
};

const panel = (page: Page) => page.getByTestId("chat-panel");
const openChat = async (page: Page) => {
  await page.getByTestId("chat-toggle").click();
  await expect(panel(page)).toBeVisible();
};
const newThread = async (page: Page) => {
  await panel(page).getByRole("button", { name: "New thread" }).click();
};
const ask = async (page: Page, text: string) => {
  const box = panel(page).getByLabel("Message");
  await box.fill(text);
  await box.press("Enter");
};
const idle = (page: Page) => expect(page.getByTestId("chat-transcript")).toHaveAttribute("data-run-status", "idle");

test("item 1 and the banner: chat off, then a streamed reply with tool steps, citations and the model named", async ({ browser, request }) => {
  await enableChat(request, false);
  const { context, page } = await newSession(browser, world.admin.token);
  const problems = problemsOf(page);
  await page.goto(`/vaults/${world.vault}/documents`);
  await openChat(page);
  await expect(panel(page).getByTestId("chat-off")).toHaveAttribute("data-chat-off", "vault");
  await expect(panel(page)).toContainText("Chat is off for this vault.");
  await enableChat(request, true);
  await page.reload();
  await openChat(page);
  await expect(panel(page).getByTestId("chat-model")).toHaveText("stand-in-chat");
  // An admin is told where the model is served, cut to its origin.
  const origin = new URL(process.env.MEMEX_LLM_BASE_URL ?? "").origin;
  await expect(panel(page).getByTestId("chat-endpoint")).toHaveText(origin);
  await expect(panel(page).getByTestId("chat-budget")).toContainText("tokens used");

  model.query = `viaduct ${run} span`;
  await ask(page, "SCENARIO:basic How long are the spans?");
  const step = panel(page).locator('[data-step-name="search"]');
  await expect(step).toHaveAttribute("data-step-state", "done");
  await expect(step).toContainText(`Searched for “viaduct ${run} span”`);
  const reply = panel(page).locator("[data-reply]").last();
  await expect(reply).toHaveAttribute("data-reply-end", "done");
  await idle(page);
  await expect(reply.locator('a[data-link-state="citation"]')).toHaveCount(2);
  await expect(reply.locator("[data-citation-unresolved]")).toHaveCount(1);
  await expect(reply).toContainText("1 citation doesn't resolve");
  const sent = model.requests.at(-1) as { messages: { role: string; content: unknown }[] };
  record("item 1: what the stand-in saw", { requests: model.requests.length, lastMessages: sent.messages.length });

  // The thread is listed, and reads back the same after a reload.
  await page.reload();
  await openChat(page);
  await panel(page).locator("[data-thread]").first().click();
  await expect(panel(page).locator('[data-step-name="search"]')).toHaveAttribute("data-step-state", "done");
  await expect(panel(page).locator("[data-reply]").last().locator('a[data-link-state="citation"]')).toHaveCount(2);

  // A reader sees the model but not the endpoint.
  const reader = await newSession(browser, world.reader.token);
  await reader.page.goto(`/vaults/${world.vault}/documents`);
  await openChat(reader.page);
  await expect(panel(reader.page).getByTestId("chat-model")).toHaveText("stand-in-chat");
  await expect(panel(reader.page).getByTestId("chat-endpoint")).toHaveCount(0);
  const settings = await get<{ endpoint?: string | null; model?: string | null }>(request, api("/chat/settings"), world.reader.token);
  expect(settings.endpoint ?? null).toBeNull();
  await reader.context.close();
  expect(problems).toEqual([]);
  await context.close();
});

test("item 2: ten citations open the region each cites", async ({ browser }) => {
  const chosen = world.pages.slice(0, 10);
  expect(chosen).toHaveLength(10);
  model.citeSegments = chosen.map((s) => s.id);
  const { context, page } = await newSession(browser, world.admin.token);
  const problems = problemsOf(page);
  await page.goto(`/vaults/${world.vault}/search`);
  await openChat(page);
  await newThread(page);
  await ask(page, "SCENARIO:cite10 Where is each span?");
  await idle(page);
  await expect(panel(page).locator('[data-step-name="read"][data-step-state="done"]')).toHaveCount(10);
  const outcomes: Record<string, unknown>[] = [];
  for (const [i, s] of chosen.entries()) {
    const chip = panel(page).locator("[data-reply]").last().locator(`a[data-link-state="citation"]:has([data-citation="${s.id}"])`);
    await expect(chip, `citation ${String(i + 1)}`).toHaveCount(1);
    await chip.click();
    await expect(page).toHaveURL(new RegExp(`/documents/${world.pdf}\\?`));
    const url = new URL(page.url());
    expect(url.searchParams.get("segment"), `citation ${String(i + 1)}: the segment`).toBe(s.id);
    expect(url.searchParams.get("page"), `citation ${String(i + 1)}: the page`).toBe(String(s.page + 1));
    await expect(page.locator('[data-segment-state="located"]')).toBeVisible();
    const cell = page.locator(`[data-page-index="${String(s.page)}"]`);
    await expect(cell.locator("[data-highlight]").first()).toBeAttached();
    await cell.scrollIntoViewIfNeeded();
    await expect(cell.locator("[data-highlight]").first()).toBeInViewport();
    outcomes.push({ citation: i + 1, segment: s.id, page: s.page + 1, ok: true });
  }
  record("item 2: outcomes", outcomes);
  expect(outcomes).toHaveLength(10);
  expect(problems).toEqual([]);
  await context.close();
});

test("item 3: hostile content in a document and in a reply runs nothing and loads nothing from outside", async ({ browser, request }) => {
  const outside = `http://127.0.0.1:${String(canary.port)}`;
  const hostile = [
    `hostile-${run}`,
    `<script>window.__pwned = "doc-script"</script>`,
    `<img src="${outside}/doc-img.png" onerror="window.__pwned='doc-img'">`,
    `[a link](javascript:window.__pwned='doc-link')`,
    `![remote](${outside}/doc-md.png)`,
    `<iframe src="${outside}/doc-frame"></iframe>`,
    `<style>@import url(${outside}/doc.css);</style>`,
    `<svg onload="window.__pwned='doc-svg'"></svg>`,
    `<a href="javascript:window.__pwned='doc-a'">tag link</a>`,
  ].join(" ");
  const res = await request.post(`/api/v1/vaults/${world.vault}/sources?name=${encodeURIComponent(`hostile-${run}.txt`)}`, { headers: bearer, data: `The ${run} hostile notes. ${hostile}\n` });
  expect(res.status(), await res.text()).toBe(201);
  const source = ((await res.json()) as { source: { id: string } }).source.id;
  await segmentsOf(request, world.vault, source, 1);
  model.query = `hostile-${run}`;
  model.hostileReply = [
    `Here it is <script>window.__pwned = "reply-script"</script> and <img src="${outside}/reply-img.png" onerror="window.__pwned='reply-img'">.`,
    `[click me](javascript:window.__pwned='reply-link') and [also](JaVaScRiPt:window.__pwned='reply-link2')`,
    `![tracker](${outside}/reply-md.png?leak=vault) <iframe src="${outside}/reply-frame"></iframe> <object data="${outside}/reply-object"></object>`,
    `<link rel="stylesheet" href="${outside}/reply.css"> <meta http-equiv="refresh" content="0;url=${outside}/reply-refresh"> <video src="${outside}/reply.mp4" autoplay></video>`,
    `<a href="${outside}/reply-a">a tag</a> [cite:javascript:alert(1)]`,
  ].join("\n\n");

  const { context, page } = await newSession(browser, world.admin.token);
  const problems = problemsOf(page);
  const outsideRequests: string[] = [];
  const origin = `http://${process.env.MEMEX_HTTP_BIND ?? ""}`;
  page.on("request", (r) => {
    if (!r.url().startsWith(`${origin}/`)) outsideRequests.push(r.url());
  });
  await page.goto(`/vaults/${world.vault}/documents`);
  await openChat(page);
  await newThread(page);
  await ask(page, "SCENARIO:hostile What do the notes say?");
  await idle(page);
  // The tool's answer, shown as what it found.
  const step = panel(page).locator('[data-step-name="search"]').last();
  await step.locator("summary").click();
  await expect(step).toContainText(`hostile-${run}`);
  const reply = panel(page).locator("[data-reply]").last();
  await expect(reply).toContainText("Here it is");
  await expect(reply).toContainText("<script>");
  // Give anything that would load time to try.
  await page.waitForTimeout(3000);
  const inPanel = await panel(page).evaluate((root) => ({
    scripts: root.querySelectorAll("script").length,
    images: root.querySelectorAll("img, image").length,
    frames: root.querySelectorAll("iframe, object, embed, video, audio, source").length,
    styles: root.querySelectorAll("style, link, meta").length,
    svgs: root.querySelectorAll("svg[onload], [onerror], [onload], [onclick]").length,
    scriptLinks: [...root.querySelectorAll("a")].filter((a) => /^\s*javascript:/i.test(a.getAttribute("href") ?? "")).length,
    blockedLinks: root.querySelectorAll("[data-blocked-link]").length,
  }));
  const pwned = await page.evaluate(() => (window as unknown as { __pwned?: unknown }).__pwned ?? null);
  const evidence = { inPanel, pwned, canaryHits: canary.hits, outsideRequests, problems: problems.filter((p) => !/^Failed to load resource/.test(p)) };
  record("item 3: hostile fixtures", evidence);
  expect(pwned).toBeNull();
  expect(canary.hits).toEqual([]);
  expect(outsideRequests).toEqual([]);
  expect(inPanel).toMatchObject({ scripts: 0, images: 0, frames: 0, styles: 0, svgs: 0, scriptLinks: 0 });
  expect(inPanel.blockedLinks).toBeGreaterThanOrEqual(2);
  // The unresolvable citation is marked, not followed.
  await expect(reply.locator("[data-citation-unresolved]")).toHaveCount(1);
  expect(problems.filter((p) => p.startsWith("dialog") || p.startsWith("pageerror"))).toEqual([]);
  await context.close();
});

const changeset = (request: APIRequestContext, id: string) => get<{ state: string; decided_by: string | null; decision_reason: string | null; proposer: string; requested_by?: string | null; ops: unknown[] }>(request, api(`/changesets/${id}`));

test("item 4: approving a card is approving in review, and those who can't decide see no buttons", async ({ browser, request }) => {
  model.query = `viaduct ${run} span`;
  const { context, page } = await newSession(browser, world.admin.token);
  const problems = problemsOf(page);
  await page.goto(`/vaults/${world.vault}/documents`);
  await openChat(page);
  const proposeIn = async (p: Page) => {
    await newThread(p);
    await ask(p, "SCENARIO:propose Please record what span 1 measures.");
    await idle(p);
    const card = panel(p).locator("[data-proposal]").last();
    await expect(card).toBeVisible();
    return { card, id: (await card.getAttribute("data-proposal")) ?? "" };
  };
  // One approved from its card.
  const first = await proposeIn(page);
  await expect(first.card.getByTestId("proposal-op")).toHaveCount(1);
  await expect(first.card).toContainText("Only a person can approve this");
  await first.card.getByRole("button", { name: "Approve" }).click();
  await expect.poll(async () => (await changeset(request, first.id)).state).toBe("committed");
  await expect(first.card.getByTestId("proposal-decided-by")).toBeVisible();
  // One approved in the review screen, the way a reviewer does.
  const second = await proposeIn(page);
  await page.goto(`/vaults/${world.vault}/review/${second.id}`);
  await expect(page.getByTestId("queue-list")).toHaveAttribute("aria-activedescendant", `queue-${second.id}`);
  await page.keyboard.press("a");
  await expect.poll(async () => (await changeset(request, second.id)).state).toBe("committed");
  const [a, b] = [await changeset(request, first.id), await changeset(request, second.id)];
  const same = (c: typeof a) => ({ state: c.state, decided_by: c.decided_by, decision_reason: c.decision_reason, proposer: c.proposer, requested_by: c.requested_by ?? null });
  record("item 4: card and review screen", { card: same(a), review: same(b) });
  expect(same(a)).toEqual(same(b));
  expect(a.decided_by).toBe(world.admin.id);
  expect(a.proposer).toBe("chat-agent");

  // Someone with propose scope gets a card in their own thread, with no buttons.
  const proposer = await newSession(browser, world.proposer.token);
  await proposer.page.goto(`/vaults/${world.vault}/documents`);
  await openChat(proposer.page);
  const theirs = await proposeIn(proposer.page);
  await expect(theirs.card.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(theirs.card.getByRole("button", { name: "Reject" })).toHaveCount(0);
  await expect(theirs.card.getByTestId("proposal-no-decide")).toContainText("Deciding needs admin scope");
  expect((await changeset(request, theirs.id)).state).toBe("pending");
  await proposer.context.close();

  // Someone with read scope can't propose at all: the step fails, and there's no card and no buttons.
  const reader = await newSession(browser, world.reader.token);
  await reader.page.goto(`/vaults/${world.vault}/documents`);
  await openChat(reader.page);
  await newThread(reader.page);
  await ask(reader.page, "SCENARIO:propose Please record what span 1 measures.");
  await idle(reader.page);
  await expect(panel(reader.page).locator("[data-proposal]")).toHaveCount(0);
  await expect(panel(reader.page).getByRole("button", { name: "Approve" })).toHaveCount(0);
  const steps = await panel(reader.page).locator("[data-step]").evaluateAll((all) => all.map((el) => [el.getAttribute("data-step-name"), el.getAttribute("data-step-state")]));
  record("item 4: a reader's steps", steps);
  await reader.context.close();
  expect(problems).toEqual([]);
  await context.close();
});

test("item 5: Stop ends the stream within a second and the partial reply stays marked", async ({ browser }) => {
  const { context, page } = await newSession(browser, world.admin.token);
  const problems = problemsOf(page);
  await page.goto(`/vaults/${world.vault}/documents`);
  await openChat(page);
  const timings: { shownMs: number; upstreamClosedMs: number | null; words: number }[] = [];
  for (let i = 0; i < 5; i += 1) {
    await newThread(page);
    const closesBefore = model.closedAt.length;
    await ask(page, `SCENARIO:slow Tell me everything, round ${String(i)}.`);
    const reply = panel(page).locator("[data-reply]").last();
    await expect(reply).toContainText("word5 ");
    const stop = panel(page).getByRole("button", { name: "Stop" });
    // The time between the click and the screen saying stopped is measured in the page, so Playwright's polling isn't in it.
    await page.evaluate(() => {
      const w = window as unknown as { __stopWatch?: Promise<number> };
      w.__stopWatch = new Promise<number>((done) => {
        let started: number | null = null;
        document.addEventListener("click", () => (started ??= performance.now()), { capture: true, once: true });
        const watch = () => {
          const transcript = document.querySelector('[data-testid="chat-transcript"]');
          const last = [...document.querySelectorAll("[data-reply]")].at(-1);
          if (started !== null && transcript?.getAttribute("data-run-status") === "idle" && last?.getAttribute("data-reply-end") === "stopped") done(performance.now() - started);
          else requestAnimationFrame(watch);
        };
        requestAnimationFrame(watch);
      });
    });
    const clickedAt = Date.now();
    await stop.click();
    const shownMs = await page.evaluate(() => (window as unknown as { __stopWatch: Promise<number> }).__stopWatch);
    await expect.poll(() => model.closedAt.length, { timeout: 10_000 }).toBeGreaterThan(closesBefore);
    const closed = model.closedAt[closesBefore] ?? null;
    const text = (await reply.textContent()) ?? "";
    timings.push({ shownMs: Math.round(shownMs), upstreamClosedMs: closed === null ? null : closed - clickedAt, words: (text.match(/word\d+/g) ?? []).length });
    await expect(reply).toHaveAttribute("data-reply-end", "stopped");
    await expect(reply).toContainText("Stopped: this reply is partial");
    // No more words arrive after the stop.
    const before = (await reply.textContent()) ?? "";
    await page.waitForTimeout(500);
    expect((await reply.textContent()) ?? "").toBe(before);
  }
  record("item 5: stop timings", { timings, load: loadAverage() });
  for (const t of timings) {
    expect(t.shownMs).toBeLessThan(1000);
    expect(t.upstreamClosedMs ?? Infinity).toBeLessThan(1000);
  }
  // The server kept the partial reply as cancelled, and it reads back marked.
  await page.reload();
  await openChat(page);
  await panel(page).locator("[data-thread]").first().click();
  await expect(panel(page).locator("[data-reply]").last()).toHaveAttribute("data-reply-end", "stopped");
  expect(problems).toEqual([]);
  await context.close();
});

test("item 7: the AG-UI client is only in edge-agui, and the chat loads only when opened", async ({ browser }) => {
  // The dependency side: only edge-agui lists them, and dependency-cruiser's rule holds the imports to it in `pnpm lint`.
  const listing = (path: string) => JSON.parse(readFileSync(join(repo, "web", path, "package.json"), "utf8")) as { dependencies?: Record<string, string>; devDependencies?: Record<string, string> };
  const packages = ["packages/types", "packages/core", "packages/api", "packages/edge-http", "packages/edge-sse", "packages/edge-browser", "packages/edge-agui", "packages/ui", "apps/console"];
  const naming = packages.filter((p) => {
    const l = listing(p);
    return Object.keys({ ...l.dependencies, ...l.devDependencies }).some((d) => d === "rxjs" || d.startsWith("@ag-ui/"));
  });
  expect(naming).toEqual(["packages/edge-agui"]);

  const { context, page } = await newSession(browser, world.admin.token);
  const scripts: string[] = [];
  page.on("request", (r) => {
    if (r.resourceType() === "script") scripts.push(new URL(r.url()).pathname);
  });
  await page.goto(`/vaults/${world.vault}/documents`);
  await expect(page.getByTestId("chat-toggle")).toBeVisible();
  const hasAgUi = async (names: readonly string[]) => {
    for (const name of names) {
      const body = await (await page.request.get(name)).text();
      if (body.includes("abortRun")) return name;
    }
    return null;
  };
  const beforeOpen = [...scripts];
  expect(beforeOpen.some((s) => s.includes("chat-panel"))).toBe(false);
  expect(await hasAgUi(beforeOpen)).toBeNull();
  await openChat(page);
  await expect(panel(page).getByTestId("chat-model")).toBeVisible();
  const afterOpen = [...scripts];
  expect(afterOpen.some((s) => s.includes("chat-panel"))).toBe(true);
  expect(await hasAgUi(afterOpen)).toBeNull();
  await newThread(page);
  await ask(page, "Anything?");
  await idle(page);
  const agUi = await hasAgUi(scripts.filter((s) => !afterOpen.includes(s)));
  record("item 7: chunks", { beforeOpen: beforeOpen.length, afterOpen: afterOpen.length - beforeOpen.length, onFirstSend: scripts.length - afterOpen.length, agUiChunk: agUi });
  expect(agUi).not.toBeNull();
  await context.close();
});
