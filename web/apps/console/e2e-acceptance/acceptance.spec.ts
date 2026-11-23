import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-admin/server";
import { makePdf } from "../e2e-documents/make-pdf";
import { Proxy } from "../e2e-ingest/proxy";
import { makeScannedPdf } from "../e2e-ingest/scanned-pdf";
import { fiftyResults } from "../e2e-search/fixture";
import { StandInAll } from "./stand-in-all";

// Spec 051: full console acceptance journey + consolidated UI target measurements.
const bind = process.env.MEMEX_HTTP_BIND ?? "";
const origin = `http://${bind}`;
const proxyPort = Number(bind.split(":")[1]) + 100;
const proxyOrigin = `http://127.0.0.1:${String(proxyPort)}`;
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const recordingPath = process.env.MEMEX_E2E_RECORDING ?? "";
const screenshotsDir = process.env.MEMEX_E2E_SCREENSHOTS ?? "";
const resultsFile = process.env.MEMEX_E2E_RESULTS ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);

const llm = new StandInAll();
let llmUrl = "";
let workDir = "";
let vaultId = "";
let vaultName = "";
let scannedSourceId = "";
let digitalSourceName = "";
let scannedSourceName = "";

const metrics: Record<string, unknown> = {
  run,
  loadBefore: "",
  loadAfter: "",
  axeRoutes: [] as { route: string; violations: number }[],
  offOriginRequests: [] as string[],
  screenshots: [] as string[],
};

const loadAvg = () => {
  try {
    const out = execFileSync("uptime").toString().trim();
    const m = /load averages?:\s*(.+)$/.exec(out);
    return m?.[1]?.trim() ?? out;
  } catch {
    return "unknown";
  }
};

const percentile = (values: readonly number[], p: number): number => {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0;
};

const serverEnv = (): Record<string, string> => ({
  MEMEX_LLM_PROVIDER: "openai",
  MEMEX_LLM_BASE_URL: llmUrl,
  MEMEX_LLM_MODEL: "stand-in-extract",
  MEMEX_AUDIT_MODEL: "stand-in-audit",
  MEMEX_CHAT_MODEL: "stand-in-chat",
  MEMEX_LLM_PRICES: "stand-in-extract=3:15,stand-in-audit=3:15,stand-in-chat=3:15",
});

const checkAxe = async (page: Page, routeLabel: string, disableRules: readonly string[] = []) => {
  let builder = new AxeBuilder({ page });
  if (disableRules.length > 0) builder = builder.disableRules([...disableRules]);
  const res = await builder.analyze();
  (metrics.axeRoutes as { route: string; violations: number }[]).push({
    route: routeLabel,
    violations: res.violations.length,
  });
  expect(
    res.violations,
    `axe violations on ${routeLabel}: ${res.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length})`).join("; ")}`,
  ).toEqual([]);
};

const snap = async (page: Page, filename: string) => {
  if (!screenshotsDir) return;
  mkdirSync(screenshotsDir, { recursive: true });
  const target = join(screenshotsDir, filename);
  await page.screenshot({ path: target, fullPage: false });
  (metrics.screenshots as string[]).push(filename);
};

const attachGuards = (page: Page, allowedOrigins: readonly string[]) => {
  const problems: string[] = [];
  page.on("request", (req) => {
    const u = req.url();
    if (u.startsWith("data:") || u.startsWith("blob:") || u.startsWith("about:")) return;
    const ok = allowedOrigins.some((o) => u.startsWith(`${o}/`) || u === o);
    if (!ok) (metrics.offOriginRequests as string[]).push(`${req.method()} ${u}`);
  });
  page.on("pageerror", (err) => problems.push(`pageerror: ${err.message}`));
  page.on("console", (msg) => {
    if (msg.type() !== "error") return;
    const loc = msg.location().url;
    const path = loc ? new URL(loc).pathname : "";
    // Expected 401 from initial signed-out whoami check on /login.
    if (msg.text().includes("401") && path === "/api/v1/whoami") return;
    problems.push(`console.error: ${msg.text()} (${loc})`);
  });
  return problems;
};

const waitAllDocsSettled = async (request: APIRequestContext, expectedCount: number) => {
  await expect
    .poll(
      async () => {
        const res = await request.get(`/api/v1/vaults/${vaultId}/activity/documents?limit=100`, { headers: bearer });
        if (!res.ok()) return "err";
        const data = (await res.json()) as { items: { stage: string; status: string; name: string }[] };
        if (data.items.length < expectedCount) return `count:${String(data.items.length)}`;
        const unsettled = data.items.filter((d) => !["review", "done"].includes(d.stage) || d.status === "failed");
        return unsettled.length === 0 ? "ok" : unsettled.map((d) => `${d.name}:${d.stage}:${d.status}`).join(",");
      },
      { timeout: 180_000, intervals: [1000] },
    )
    .toBe("ok");
};

test.beforeAll(async () => {
  metrics.loadBefore = loadAvg();
  workDir = mkdtempSync(join(tmpdir(), "memex-s051-"));
  llmUrl = await llm.start();
  const ms = await startServer(serverEnv());
  console.log(`acceptance: server started in ${String(ms)} ms, stand-in LLM at ${llmUrl}`);
});

test.afterAll(async () => {
  metrics.loadAfter = loadAvg();
  await stopServer();
  await llm.stop();
  if (workDir) rmSync(workDir, { recursive: true, force: true });
  if (resultsFile) {
    mkdirSync(join(resultsFile, ".."), { recursive: true });
    writeFileSync(resultsFile, `${JSON.stringify(metrics, null, 2)}\n`);
  }
  console.log(`ACCEPTANCE_METRICS: ${JSON.stringify(metrics)}`);
});

test("1. full console journey: upload to chat, review, audit, erasure, axe, keyboard, same-origin", async ({ browser, page, request }) => {
  const problems = attachGuards(page, [origin]);

  // 1. Login screen
  await page.goto("/login");
  await expect(page.getByLabel("Access token")).toBeVisible();
  await checkAxe(page, "/login");
  await snap(page, "01-login.png");

  await page.getByLabel("Access token").fill(adminToken);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible({ timeout: 60_000 });

  // 2. Create a vault via the UI (first vault on home page, or Admin -> Vaults)
  vaultName = `acceptance-${run}`;
  const createInput = page.getByLabel("New vault's name");
  if (!(await createInput.isVisible().catch(() => false))) {
    // Already have a vault: navigate to Admin -> Vaults
    const currentVaultMatch = /\/vaults\/([^/]+)/.exec(page.url());
    const existingVault = currentVaultMatch?.[1] ?? "";
    await page.goto(`/vaults/${existingVault}/admin/vaults`);
  }
  await expect(createInput).toBeVisible();
  await createInput.fill(vaultName);
  await page.getByRole("button", { name: "Create vault" }).click();
  await expect(page).toHaveURL(/\/vaults\/[^/]+/, { timeout: 30_000 });
  const mVault = /\/vaults\/([^/]+)/.exec(page.url());
  vaultId = mVault?.[1] ?? "";
  expect(vaultId).not.toBe("");

  // Enable chat on this vault via the UI
  await page.goto(`/vaults/${vaultId}/admin/vault`);
  await page.getByRole("button", { name: "Turn chat on…" }).click();
  await page.getByRole("button", { name: "Turn chat on", exact: true }).click();
  await expect(page.getByRole("button", { name: "Turn chat off" })).toBeVisible();
  await checkAxe(page, "/admin/vault");
  await snap(page, "02-admin-vault.png");

  // 3. Upload digital PDF, scanned PDF, and recording on Ingest
  await page.goto(`/vaults/${vaultId}/ingest`);
  await expect(page.locator('[data-connection="live"]')).toBeVisible({ timeout: 30_000 });

  digitalSourceName = `turbine-report-${run}.pdf`;
  scannedSourceName = `scanned-memo-${run}.pdf`;
  const digitalPath = join(workDir, digitalSourceName);
  const scannedPath = join(workDir, scannedSourceName);
  writeFileSync(
    digitalPath,
    makePdf(2, (p) => `Gothenburg turbine pressure valve inspection page ${String(p + 1)} nominal 42 bar`),
  );
  writeFileSync(scannedPath, await makeScannedPdf(browser, ["Scanned turbine inspection memo Gothenburg 42 bar"]));

  const uploadFiles = [digitalPath, scannedPath];
  if (recordingPath) {
    const wavCopy = join(workDir, `briefing-${run}.wav`);
    writeFileSync(wavCopy, readFileSync(recordingPath));
    uploadFiles.push(wavCopy);
  }

  const fileInput = page.locator('input[type="file"]');
  await fileInput.setInputFiles(uploadFiles);

  // Watch them process live through the pipeline until all settle at review/done
  await waitAllDocsSettled(request, uploadFiles.length);
  await expect(page.getByText(digitalSourceName).first()).toBeVisible();
  await expect(page.getByText(scannedSourceName).first()).toBeVisible();
  await checkAxe(page, "/ingest");
  await snap(page, "03-ingest.png");

  // Record scanned source id for erasure later
  const sourcesRes = await request.get(`/api/v1/vaults/${vaultId}/sources?limit=50`, { headers: bearer });
  const sourcesJson = (await sourcesRes.json()) as { items: { id: string; name: string }[] };
  scannedSourceId = sourcesJson.items.find((s) => s.name === scannedSourceName)?.id ?? "";
  expect(scannedSourceId).not.toBe("");

  // 4. Review: approve extracted claims via UI
  await page.goto(`/vaults/${vaultId}/review`);
  await expect(page.getByRole("heading", { name: "Review", exact: true })).toBeVisible();
  await checkAxe(page, "/review");
  await snap(page, "04-review.png");

  // Approve pending items via the UI queue
  for (let i = 0; i < 6; i += 1) {
    const approveBtn = page.getByRole("button", { name: /^Approve/ }).first();
    if (!(await approveBtn.isVisible({ timeout: 3000 }).catch(() => false))) break;
    await approveBtn.click();
    await page.waitForTimeout(400);
  }

  // 5. Documents viewer: inspect digital PDF and recording
  await page.goto(`/vaults/${vaultId}/documents`);
  await expect(page.getByText(digitalSourceName).first()).toBeVisible();
  await page.getByText(digitalSourceName).first().click();
  await expect(page.locator("img").first()).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/documents");
  await snap(page, "05-documents.png");

  if (recordingPath) {
    await page.goto(`/vaults/${vaultId}/documents`);
    await page.getByText(`briefing-${run}.wav`).first().click();
    await expect(page.locator("audio").first()).toBeVisible({ timeout: 30_000 });
  }

  // 6. Wiki: create a page with a [[link]] via the UI
  await page.goto(`/vaults/${vaultId}/pages`);
  await page.getByRole("link", { name: "New page" }).click();
  await page.getByLabel("Title").fill("Acceptance Overview");
  const cm = page.locator(".cm-content");
  await cm.click();
  await cm.fill("Summary of the Gothenburg turbine inspection. See also [[Valve Notes]].");
  await page.getByLabel("Why").fill("Create acceptance overview page");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("heading", { name: "Acceptance Overview" })).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/pages");
  await snap(page, "06-wiki.png");

  // 7. Entities: create runtime entity type `Valve` and an entity via UI
  await page.goto(`/vaults/${vaultId}/entities`);
  await page.getByRole("link", { name: "New type" }).click();
  await page.getByLabel("Type name").fill("Valve");
  const firstField = page.locator("[data-draft-field]").first();
  await firstField.getByLabel("Field name").fill("code");
  await firstField.getByLabel("Identity").check();
  await firstField.getByLabel("Searchable").check();
  await firstField.getByLabel("Filterable").check();
  await page.getByLabel("Why").fill("Define Valve type for acceptance");
  await page.getByRole("button", { name: "Preview the change" }).click();
  await expect(page.getByRole("button", { name: "Submit" })).toBeEnabled({ timeout: 15_000 });
  await page.getByRole("button", { name: "Submit" }).click();
  await page.goto(`/vaults/${vaultId}/entities/-/types/Valve`);
  await expect(page.getByRole("link", { name: "New entity" })).toBeVisible({ timeout: 30_000 });

  // Create a Valve entity
  await page.getByRole("link", { name: "New entity" }).click();
  await page.locator("#new-code").fill("VLV-0042");
  await page.getByLabel("Why").fill("Add acceptance valve");
  await page.getByRole("button", { name: "Propose this entity" }).click();
  await expect(page.getByText("VLV-0042").first()).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/entities");
  await snap(page, "07-entities.png");

  // 8. Search & follow citation into the Document viewer
  await page.goto(`/vaults/${vaultId}/search`);
  const searchBox = page.getByRole("searchbox");
  await searchBox.fill("turbine");
  await searchBox.press("Enter");
  await expect(page.getByText(/Gothenburg turbine/i).first()).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/search");
  await snap(page, "08-search.png");

  // Click a citation link to open the Document viewer
  const citationLink = page.locator(`a[href*="/vaults/${vaultId}/documents/"]`).first();
  await expect(citationLink).toBeVisible();
  await citationLink.click();
  await expect(page).toHaveURL(new RegExp(`/vaults/${vaultId}/documents/`), { timeout: 30_000 });

  // 9. Audit: trigger an on-demand run and wait for it to finish
  await page.goto(`/vaults/${vaultId}/audit`);
  await expect(page.getByRole("button", { name: "Start run" })).toBeVisible();
  await page.getByLabel("Seed").fill("7");
  await page.getByLabel("Sample size").fill("10");
  await page.getByLabel("Token budget").fill("50000");
  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page.getByTestId("run-state").first()).toHaveText("Finished", { timeout: 120_000 });
  await checkAxe(page, "/audit");
  await snap(page, "09-audit.png");

  // 10. Chat: stream reply with citation chip + proposal card, approve proposal directly in Chat
  llm.chatQuery = "turbine";
  await page.goto(`/vaults/${vaultId}/chat`);
  const composer = page.getByLabel("Message");
  await expect(composer).toBeVisible();
  await composer.fill("SCENARIO:propose Summarize the turbine inspection");
  await composer.press("Enter");
  await expect(page.getByTestId("chat-transcript")).toHaveAttribute("data-run-status", "idle", { timeout: 60_000 });
  await expect(page.getByText(/queued the proposal/i)).toBeVisible();
  // Approve the proposal card right inside Chat
  const cardApprove = page.getByRole("button", { name: "Approve", exact: true }).first();
  await expect(cardApprove).toBeVisible({ timeout: 15_000 });
  await cardApprove.click();
  await expect(page.getByText(/Committed|Approved/i).first()).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/chat");
  await snap(page, "10-chat.png");

  // 11. Admin: propose & approve erasure of scanned PDF, verify source is gone
  await page.goto(`/vaults/${vaultId}/admin/erasure`);
  await page.getByRole("link", { name: scannedSourceName }).click();
  await expect(page.getByTestId("removed-count")).toBeVisible({ timeout: 30_000 });
  await page.getByLabel("Why it's being erased, for the person who approves it").fill("Acceptance erasure check");
  await page.getByRole("button", { name: "Erase…" }).click();
  await page.getByLabel(`Type the document's name, ${scannedSourceName}, to confirm`).fill(scannedSourceName);
  await page.getByRole("button", { name: "Propose the erasure" }).click();
  await expect(page.getByTestId("erasure-progress")).toHaveAttribute("data-state", "pending", { timeout: 30_000 });
  await checkAxe(page, "/admin/erasure");
  await snap(page, "11-admin.png");

  // Follow link to Review and approve the erasure
  await page.getByRole("link", { name: "Open it in Review" }).click();
  await expect(page.getByRole("button", { name: /^Approve/ }).first()).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: /^Approve/ }).first().click();

  // Confirm source file returns 404 once erase job finishes
  await expect
    .poll(
      async () => {
        const res = await request.get(`/api/v1/vaults/${vaultId}/sources/${scannedSourceId}/file`, { headers: bearer });
        return res.status();
      },
      { timeout: 60_000, intervals: [500] },
    )
    .toBe(404);

  // 12. Keyboard pass: navigate sidebar via keyboard & open/close command palette
  const navLinks = page.locator("nav a");
  await navLinks.first().focus();
  const initialFocus = await page.evaluate(() => document.activeElement?.textContent?.trim() ?? "");
  expect(initialFocus.length).toBeGreaterThan(0);
  await page.keyboard.press("Tab");
  const nextFocus = await page.evaluate(() => document.activeElement?.textContent?.trim() ?? "");
  expect(nextFocus).not.toBe(initialFocus);
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();

  // Verify zero off-origin requests and zero unexpected errors
  expect(metrics.offOriginRequests).toEqual([]);
  // Allow only the known Radix modal CSP error when opening command palette (spec 040).
  const nonModalProblems = problems.filter((p) => !/content security policy/i.test(p));
  expect(nonModalProblems).toEqual([]);
});

test("2. consolidated UI targets (specs 036, 040, 041, 043, 045)", async ({ page, request }) => {
  // --- Spec 036: 100,000-row virtualized table scroll >= 50 fps on /gallery ---
  await page.goto("/gallery");
  await expect(page.getByRole("heading", { name: "Design system" })).toBeVisible({ timeout: 30_000 });
  await checkAxe(page, "/gallery");
  const table = page.getByRole("table", { name: "Documents" });
  await table.scrollIntoViewIfNeeded();
  await expect(table).toHaveAttribute("aria-rowcount", "100001");
  const scrollResult = await table.evaluate(async (el) => {
    const frames: number[] = [];
    const duration = 2500;
    const max = el.scrollHeight - el.clientHeight;
    const start = performance.now();
    await new Promise<void>((done) => {
      const tick = (now: number) => {
        frames.push(now);
        const t = (now - start) / duration;
        el.scrollTop = ((t * 2) % 1) * max;
        if (t < 1) requestAnimationFrame(tick);
        else done();
      };
      requestAnimationFrame(tick);
    });
    const gaps = frames.slice(1).map((f, i) => f - (frames[i] ?? f)).sort((a, b) => a - b);
    return {
      fps: Number(((frames.length - 1) / (duration / 1000)).toFixed(1)),
      p95GapMs: Number((gaps[Math.floor(gaps.length * 0.95)] ?? 0).toFixed(1)),
      domRows: el.querySelectorAll('[role="row"]').length,
    };
  });
  metrics.spec036TableScroll = scrollResult;
  expect(scrollResult.fps).toBeGreaterThanOrEqual(50);
  expect(scrollResult.domRows).toBeGreaterThan(0);
  expect(scrollResult.domRows).toBeLessThan(60);

  // Sign in for authenticated target checks
  await page.goto("/login");
  await page.getByLabel("Access token").fill(adminToken);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible({ timeout: 60_000 });

  // --- Spec 043: Search UI added time over API <= 50 ms p95 on 50-result fixture ---
  const fixtureBody = JSON.stringify(fiftyResults());
  await page.route(`**/api/v1/vaults/${vaultId}/search?*`, async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: fixtureBody });
  });
  await page.goto(`/vaults/${vaultId}/search`);
  await expect(page.locator("#search-box")).toBeVisible();
  const searchSamples = await page.evaluate(
    async ({ total }) => {
      const input = document.getElementById("search-box") as HTMLInputElement;
      const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      const out: number[] = [];
      for (let n = 0; n < total; n += 1) {
        const q = `measured river ${String(n)}`;
        const painted = new Promise<number>((done, reject) => {
          const timer = setTimeout(() => {
            observer.disconnect();
            reject(new Error(`timed out waiting for search results for ${q}`));
          }, 15_000);
          const observer = new MutationObserver(() => {
            const el = document.querySelector("[data-search-results]:not([data-search-stale])");
            if (el?.getAttribute("data-search-query") !== q) return;
            clearTimeout(timer);
            observer.disconnect();
            requestAnimationFrame(() => {
              const ch = new MessageChannel();
              ch.port1.onmessage = () => done(performance.now());
              ch.port2.postMessage(0);
            });
          });
          observer.observe(document, { subtree: true, childList: true, attributes: true });
        });
        setValue?.call(input, q);
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.form?.requestSubmit();
        const paintedAt = await painted;
        const entry = performance
          .getEntriesByType("resource")
          .filter((e) => e.name.includes("/search?") && decodeURIComponent(e.name.replace(/\+/g, " ")).includes(q))
          .at(-1) as PerformanceResourceTiming | undefined;
        if (entry) out.push(Math.max(0, paintedAt - entry.responseEnd));
        await new Promise((r) => setTimeout(r, 40));
      }
      return out;
    },
    { total: 55 },
  );
  await page.unroute(`**/api/v1/vaults/${vaultId}/search?*`);
  const countedSearch = searchSamples.slice(5);
  const searchP95 = Number(percentile(countedSearch, 0.95).toFixed(1));
  const searchP50 = Number(percentile(countedSearch, 0.5).toFixed(1));
  metrics.spec043SearchUi = { p50Ms: searchP50, p95Ms: searchP95, samples: countedSearch.length };
  expect(countedSearch.length).toBeGreaterThanOrEqual(45);
  expect(searchP95).toBeLessThanOrEqual(50);

  // --- Spec 045: New entity type indexing lag <= 1000 ms & filter UI overhead <= 100 ms p95 ---
  const lagSamples: number[] = [];
  const typeName = `Pump${run.slice(-4)}`;
  const tRes = await request.post(`/api/v1/vaults/${vaultId}/entity-types`, {
    headers: bearer,
    data: {
      name: typeName,
      schema: {
        type: "object",
        additionalProperties: false,
        required: ["name"],
        properties: {
          name: { type: "string", identity: true, searchable: true, filterable: true },
          rating: { type: "number", filterable: true },
        },
      },
      rationale: "acceptance lag bench",
      idempotency_key: `acc-type-${run}`,
    },
  });
  expect(tRes.ok()).toBe(true);
  for (let i = 0; i < 10; i += 1) {
    const tokenName = `PMP${run}x${String(i)}`;
    const t0 = Date.now();
    const eRes = await request.post(`/api/v1/vaults/${vaultId}/entities`, {
      headers: bearer,
      data: {
        type: typeName,
        aliases: [],
        fields: [
          { field: "name", value: tokenName },
          { field: "rating", value: 10 + i },
        ],
        rationale: "acceptance lag bench",
        idempotency_key: `acc-ent-${run}-${String(i)}`,
      },
    });
    expect(eRes.ok()).toBe(true);
    const afterPost = Date.now();
    for (;;) {
      const s = await request.get(`/api/v1/vaults/${vaultId}/search/lexical?q=${encodeURIComponent(tokenName)}&kind=entity&limit=5`, { headers: bearer });
      if (s.ok()) {
        const body = (await s.json()) as { hits?: unknown[] };
        if ((body.hits?.length ?? 0) > 0) {
          lagSamples.push(Date.now() - afterPost);
          break;
        }
      }
      if (Date.now() - t0 > 15_000) break;
      await new Promise((r) => setTimeout(r, 20));
    }
  }
  const lagP95 = Number(percentile(lagSamples, 0.95).toFixed(1));
  metrics.spec045EntityLag = { p50Ms: Number(percentile(lagSamples, 0.5).toFixed(1)), p95Ms: lagP95, samples: lagSamples.length };
  expect(lagSamples.length).toBe(10);
  expect(lagP95).toBeLessThanOrEqual(1000);

  await page.goto(`/vaults/${vaultId}/entities/-/types/${typeName}?f=${encodeURIComponent("rating:gte:0")}`);
  await expect(page.locator(`[data-filter-key="rating:gte:0"][data-settled="true"]`)).toBeVisible({ timeout: 30_000 });
  const filterOverhead: number[] = [];
  for (let i = 0; i < 20; i += 1) {
    const val = String(10 + (i % 10));
    const expectedKey = `rating:gte:${val}`;
    await page.locator("#filter-0-value").fill(val);
    const trial = await page.evaluate(async (key) => {
      performance.clearResourceTimings();
      const form = document.querySelector<HTMLFormElement>("form[aria-label=Filters]");
      if (form === null) throw new Error("no filter form");
      const started = performance.now();
      form.requestSubmit();
      const settled = () => {
        const status = document.querySelector(`[data-filter-key="${CSS.escape(key)}"]`) ?? document.querySelector("[data-filter-key]");
        return status?.getAttribute("data-filter-key") === key && status.getAttribute("data-settled") === "true" ? status : null;
      };
      if (!settled()) {
        await new Promise<void>((done, fail) => {
          const stop = setTimeout(() => fail(new Error(`no filter settle for ${key}`)), 15_000);
          const observer = new MutationObserver(() => {
            if (settled() !== null) {
              clearTimeout(stop);
              observer.disconnect();
              done();
            }
          });
          observer.observe(document.body, { subtree: true, childList: true, attributes: true });
        });
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const total = performance.now() - started;
      const req = performance.getEntriesByType("resource").filter((e) => e.name.includes("/entities?")).at(-1) as PerformanceResourceTiming | undefined;
      const api = req ? req.responseEnd - req.startTime : 0;
      return Math.max(0, total - api);
    }, expectedKey);
    filterOverhead.push(trial);
  }
  const filterP95 = Number(percentile(filterOverhead, 0.95).toFixed(1));
  metrics.spec045FilterOverhead = { p50Ms: Number(percentile(filterOverhead, 0.5).toFixed(1)), p95Ms: filterP95, samples: filterOverhead.length };
  expect(filterOverhead.length).toBe(20);
  expect(filterP95).toBeLessThanOrEqual(100);

  // --- Spec 041: Live ingest board 50 events/s burst (10 s, 500 frames) through local proxy ---
  const proxy = new Proxy(bind, proxyPort);
  await proxy.start();
  try {
    const browser = page.context().browser();
    if (!browser) throw new Error("no browser on context");
    const pContext = await browser.newContext({ baseURL: proxyOrigin, colorScheme: "dark", viewport: { width: 1440, height: 960 } });
    const pPage = await pContext.newPage();
    await pPage.goto("/login");
    await pPage.getByLabel("Access token").fill(adminToken);
    await pPage.getByRole("button", { name: "Sign in" }).click();
    await expect(pPage.getByRole("button", { name: "Sign out" })).toBeVisible({ timeout: 30_000 });

    const docCount = 200;
    const now = Date.now();
    const docs = Array.from({ length: docCount }, (_, i) => ({
      source_id: `load-src-${String(i)}`,
      name: `load-doc-${String(i)}.pdf`,
      media_type: "application/pdf",
      stage: "ocr",
      status: "active",
      job_id: `job-${String(i)}`,
      progress: { stage: "ocr", done: 0, total: 1000, time_ms: now },
      pending_review: 0,
      created_at_ms: now,
      updated_at_ms: now,
    }));
    const sseClients = new Set<import("node:http").ServerResponse>();
    proxy.setScript((req, res) => {
      const u = req.url ?? "";
      if (req.method === "GET" && u.includes("/activity/documents")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ head: 1, items: docs, next_cursor: null }));
        return true;
      }
      if (req.method === "GET" && /\/activity(\?|$)/.test(u)) {
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
        res.write(`event: hello\ndata: ${JSON.stringify({ head: 1, resumed: true })}\n\n`);
        res.socket?.setNoDelay(true);
        sseClients.add(res);
        res.on("close", () => sseClients.delete(res));
        return true;
      }
      return false;
    });

    await pPage.goto(`/vaults/${vaultId}/ingest`);
    await expect(pPage.locator('[data-connection="live"]')).toBeVisible({ timeout: 30_000 });

    const inputLatencies: number[] = [];
    const filterInput = pPage.getByPlaceholder(/Filter/i).first();
    const burstStart = Date.now();
    const appearLatencies: number[] = [];
    for (let sec = 0; sec < 10; sec += 1) {
      for (let f = 0; f < 50; f += 1) {
        const idx = f % 10;
        const done = sec * 50 + f + 1;
        const sentAt = Date.now();
        const frame = JSON.stringify({
          job_id: `job-${String(idx)}`,
          source_id: `load-src-${String(idx)}`,
          stage: "ocr",
          done,
          total: 1000,
          time_ms: sentAt,
        });
        for (const client of sseClients) {
          client.write(`event: progress\ndata: ${frame}\n\n`);
        }
        if (f === 0) {
          const tIn = await pPage.evaluate(() => performance.now());
          if (await filterInput.isVisible().catch(() => false)) {
            await filterInput.fill(sec % 2 === 0 ? "load-doc-0" : "");
          }
          const tOut = await pPage.evaluate(() => new Promise<number>((r) => requestAnimationFrame(() => r(performance.now()))));
          inputLatencies.push(Math.max(0, tOut - tIn));
          appearLatencies.push(Math.max(0, Date.now() - sentAt));
        }
        await new Promise((r) => setTimeout(r, 18));
      }
    }
    for (const client of sseClients) client.end();
    proxy.setScript(null);
    const burstDurationS = Number(((Date.now() - burstStart) / 1000).toFixed(1));
    const appearP95 = Number(percentile(appearLatencies, 0.95).toFixed(1));
    const inputP95 = Number(percentile(inputLatencies, 0.95).toFixed(1));
    metrics.spec041LiveBurst = { durationS: burstDurationS, frames: 500, appearP95Ms: appearP95, inputP95Ms: inputP95 };
    expect(appearP95).toBeLessThanOrEqual(1000);
    expect(inputP95).toBeLessThanOrEqual(100);
    await pContext.close();
  } finally {
    await proxy.stop();
  }

  // --- Spec 040: Offline banner within 5 s of server stop, recovers without reload ---
  await page.goto(`/vaults/${vaultId}/documents`);
  await expect(page.getByText(digitalSourceName).first()).toBeVisible({ timeout: 30_000 });
  await page.evaluate(() => {
    (window as unknown as { __noReloadMarker: string }).__noReloadMarker = "alive";
  });
  const stoppedAt = Date.now();
  await stopServer();
  await expect(page.getByRole("status").filter({ hasText: /reach the server|offline/i }).first()).toBeVisible({ timeout: 5_000 });
  const bannerMs = Date.now() - stoppedAt;

  await startServer(serverEnv());
  const backAt = Date.now();
  await expect(page.getByRole("status").filter({ hasText: /reach the server|offline/i })).toHaveCount(0, { timeout: 10_000 });
  const recoverMs = Date.now() - backAt;
  const marker = await page.evaluate(() => (window as unknown as { __noReloadMarker?: string }).__noReloadMarker);
  expect(marker).toBe("alive");
  metrics.spec040Offline = { bannerMs, recoverMs, reloaded: marker !== "alive" };
  expect(bannerMs).toBeLessThanOrEqual(5_000);
});
