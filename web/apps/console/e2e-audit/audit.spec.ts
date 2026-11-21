import { appendFileSync } from "node:fs";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Browser, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";
import { StandInLlm, WRONG_MARK } from "./stand-in-llm";

// Spec 047. A real server with the real CSP. The only stand-in is the LLM's server, on this machine, so audits run their LLM checks and nothing calls a provider.
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const llm = new StandInLlm();
const EXTRACT_MODEL = `stand-in-extract-${run}`;
const AUDIT_MODEL = `stand-in-audit-${run}`;
const PRICES = `${EXTRACT_MODEL}=3:15`;
const HOSTILE = `<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>`;

const evidence = (line: string) => {
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};

interface World {
  readonly vault: string;
  readonly reader: string;
  readonly agent: string;
  readonly claims: readonly string[];
  readonly planted: number;
}
let world: World;

const api = (path: string) => `/api/v1/vaults/${world.vault}${path}`;
const send = async <T>(request: APIRequestContext, path: string, data: unknown, status = 200, token = adminToken, method: "post" | "put" = "post"): Promise<T> => {
  const res = await request[method](path, { headers: { authorization: `Bearer ${token}` }, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json().catch(() => null)) as T;
};
const get = async <T>(request: APIRequestContext, path: string, token = adminToken): Promise<T> => {
  const res = await request.get(path, { headers: { authorization: `Bearer ${token}` } });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};

const segmentOf = async (request: APIRequestContext, vault: string, source: string): Promise<string> => {
  let found: string | undefined;
  await expect.poll(async () => {
    const renditions = await get<{ items: { id: string }[] }>(request, `/api/v1/vaults/${vault}/sources/${source}/renditions`);
    const rendition = renditions.items[0];
    if (rendition === undefined) return false;
    const segments = await get<{ items: { id: string }[] }>(request, `/api/v1/vaults/${vault}/renditions/${rendition.id}/segments`);
    found = segments.items[0]?.id;
    return found !== undefined;
  }, { timeout: 180_000, intervals: [500] }).toBe(true);
  return found ?? "";
};

const SOURCE_TEXT = (i: number) => `Ship ${String(i)} of the ${run} fleet was launched at Karlskrona in ${String(1700 + i)}. ${i === 0 ? HOSTILE : ""}`;

const serverEnv = (priced: boolean) => {
  process.env.MEMEX_LLM_PROVIDER = "openai";
  process.env.MEMEX_LLM_MODEL = EXTRACT_MODEL;
  process.env.MEMEX_AUDIT_MODEL = AUDIT_MODEL;
  if (priced) process.env.MEMEX_LLM_PRICES = PRICES;
  else delete process.env.MEMEX_LLM_PRICES;
};

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  process.env.MEMEX_LLM_BASE_URL = await llm.start();
  serverEnv(true);
  evidence(`spec 047: server started in ${String(await startServer())} ms`);
  const vault = await send<{ id: string }>(request, "/api/v1/vaults", { name: `audit-${run}` }, 201);
  const res = await request.post(`/api/v1/vaults/${vault.id}/audit/test-vault`, { headers: bearer });
  expect(res.status(), await res.text()).toBe(204);
  const make = async (name: string, kind: string, scope: string) => {
    const p = await send<{ id: string }>(request, "/api/v1/principals", { name: `${name}-${run}`, kind }, 201);
    const t = await send<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: p.id, grants: [{ vault_id: vault.id, scope }] }, 201);
    return t.token;
  };
  const reader = await make("reader", "human", "read");
  const agent = await make("agent", "agent", "propose");
  const segments: { segment: string; i: number }[] = [];
  for (let i = 0; i < 6; i += 1) {
    const up = await request.post(`/api/v1/vaults/${vault.id}/sources?name=ship-${String(i)}-${run}.txt`, { headers: bearer, data: SOURCE_TEXT(i) });
    expect(up.status(), await up.text()).toBe(201);
    const source = ((await up.json()) as { source: { id: string } }).source.id;
    segments.push({ segment: await segmentOf(request, vault.id, source), i });
  }
  world = { vault: vault.id, reader, agent, claims: [], planted: 0 };
  // 24 claims, four on each source, each quoting its source and approved by a person, so they stand as a machine's claims do.
  const claims: string[] = [];
  for (let n = 0; n < 24; n += 1) {
    const { segment, i } = segments[n % 6] ?? { segment: "", i: 0 };
    const id = `claim-${String(n)}-${run}`;
    const subject = n === 5 ? `ship ${String(i)} ${WRONG_MARK}` : `ship ${String(i)} (${String(n)})`;
    const cs = await send<{ changeset: { id: string; state: string } }>(request, api("/proposals"), {
      ops: [{ op: "add_assertion", id, subject, predicate: "launched in", value: String(1700 + i), epistemic: "asserted", machine: true, confidence_permille: 900, valid_from_ms: null, valid_to_ms: null, evidence: [[`${id}-e`, { segment_id: segment, quote: `launched at Karlskrona in ${String(1700 + i)}`, stance: "supports" }]] }],
      rationale: "fixture",
      idempotency_key: `${id}-k`,
    }, 200, agent);
    if (cs.changeset.state === "pending") await send(request, api(`/changesets/${cs.changeset.id}/approve`), { reason: "fixture" });
    claims.push(id);
  }
  // Planting needs the keyword index caught up, and says so until it is.
  let planted = 0;
  await expect.poll(async () => {
    const r = await request.post(api("/audit/planted"), { headers: bearer, data: { per_kind: 1, seed: "7" } });
    if (r.status() !== 200) return r.status();
    planted = ((await r.json()) as { kinds: { planted: number }[] }).kinds.reduce((a, k) => a + k.planted, 0);
    return 200;
  }, { timeout: 120_000, intervals: [1000] }).toBe(200);
  world = { ...world, claims, planted };
  evidence(`spec 047: fixture vault ${vault.id}, 24 claims, ${String(planted)} planted errors`);
});

test.afterAll(async () => {
  await stopServer();
  await llm.stop();
});

const signIn = async (browser: Browser, token: string): Promise<Page> => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: test.info().project.use.baseURL ?? "", viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log(`page error: ${e.message}`));
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/vaults\//, { timeout: 60_000 });
  return page;
};

interface RunWire { id: string; state: string; stop: string | null; seed: string; suspect: number; failed: number; tokens: { input_tokens: number; output_tokens: number; thinking_tokens: number }; llm_calls: number }

/** Starts a run through the form, and returns its id from the page it lands on. */
const startRun = async (page: Page, seed: string, sample: string, budget = ""): Promise<string> => {
  await page.goto(`/vaults/${world.vault}/audit`);
  await page.getByLabel("Seed").fill(seed);
  await page.getByLabel("Sample size").fill(sample);
  await page.getByLabel("Token budget").fill(budget);
  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.vault}/audit/[^/?-][^/?]*$`));
  return decodeURIComponent(new URL(page.url()).pathname.split("/").pop() ?? "");
};

/** A finished run with this seed, made through the API when no earlier check left one, so each check stands alone. */
const finishedRun = async (request: APIRequestContext, seed: string): Promise<string> => {
  const runs = await get<{ items: { id: string; seed: string }[] }>(request, api("/audit/runs?limit=200"));
  const id = runs.items.find((r) => r.seed === seed)?.id ?? (await send<{ id: string }>(request, api("/audit/runs"), { seed, sample_size: 50 }, 202)).id;
  await expect.poll(async () => (await get<{ run: RunWire }>(request, api(`/audit/runs/${id}`))).run.state, { timeout: 120_000 }).toBe("finished");
  return id;
};

const sampleText = async (page: Page) => (await page.getByTestId("sample").locator("li").allTextContents()).join(" ");

test("item 1: a run started from the UI appears live, finishes, and its findings link to their review items", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  const loads: string[] = [];
  await page.goto(`/vaults/${world.vault}/audit`);
  await expect(page.getByRole("button", { name: "Start run" })).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  const t0 = Date.now();
  const id = await startRun(page, "42", "50");
  page.on("load", () => loads.push(page.url()));
  const seen = new Set<string>();
  // Reads the badge as it changes, with no reload, until it says finished.
  await expect.poll(async () => {
    const state = (await page.getByTestId("run-state").first().textContent()) ?? "";
    seen.add(state);
    return state;
  }, { timeout: 120_000, intervals: [100] }).toBe("Finished");
  const finishedMs = Date.now() - t0;
  expect(loads, "it finished on screen with no reload").toEqual([]);
  const wire = await get<{ run: RunWire; sample: string[] }>(request, api(`/audit/runs/${id}`));
  expect(wire.run.state).toBe("finished");
  expect(wire.run.llm_calls, "the stand-in answered its LLM checks").toBeGreaterThan(0);
  await expect(page.getByTestId("run-seed")).toHaveText("42");
  await expect(page.getByTestId("run-failed")).toHaveText(String(wire.run.failed));
  await expect(page.getByTestId("run-suspect")).toHaveText(String(wire.run.suspect));
  evidence(`item 1: run ${id} finished on screen ${String(finishedMs)} ms after Start, states seen ${[...seen].join(" > ")}, ${String(wire.run.failed)} failed, ${String(wire.run.suspect)} suspect, ${String(wire.run.llm_calls)} LLM calls, sample ${String(wire.sample.length)}`);

  // Findings only, each with its review item where the server made one.
  await page.getByRole("link", { name: "Show findings only" }).click();
  await expect(page).toHaveURL(/findings=true/);
  const checks = await get<{ items: { claim_id: string; outcome: string; changeset_id: string | null; checks: { check: string; cause: string | null }[] }[] }>(request, api(`/audit/runs/${id}/checks?findings=true&limit=200`));
  expect(checks.items.length).toBe(wire.run.failed + wire.run.suspect);
  await expect(page.getByTestId("checks-table").locator("tbody tr[data-check]")).toHaveCount(checks.items.length);
  const linked = checks.items.filter((c) => c.changeset_id !== null);
  expect(linked.length, "some findings became review items").toBeGreaterThan(0);
  for (const c of linked) {
    const row = page.locator(`tr[data-check="${c.claim_id}"]`);
    await expect(row.getByRole("link", { name: "Review item" })).toHaveAttribute("href", `/vaults/${world.vault}/review/${encodeURIComponent(c.changeset_id ?? "")}`);
  }
  const entailment = checks.items.find((c) => c.checks.some((k) => k.cause === "not_entailed"));
  evidence(`item 1: ${String(checks.items.length)} findings, ${String(linked.length)} with review items, not_entailed from the stand-in ${entailment === undefined ? "not among them (that claim was planted over)" : `on ${entailment.claim_id}`}`);

  // Following one leads to the review item, a retraction a person decides.
  const first = linked[0];
  if (first === undefined) throw new Error("no linked finding");
  const item = await get<{ state: string; ops: { op: string }[] }>(request, api(`/changesets/${first.changeset_id ?? ""}`));
  expect(item.state).toBe("pending");
  expect(item.ops.map((o) => o.op)).toContain("retract_assertion");
  const row = page.locator(`tr[data-check="${first.claim_id}"]`);
  await row.getByRole("button", { name: "Details" }).click();
  await expect(page.getByTestId("check-details").first()).toBeVisible();
  await expect(page.getByTestId("claim-sources").first()).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await row.getByRole("link", { name: "Review item" }).click();
  await expect(page).toHaveURL(new RegExp(`/review/${first.changeset_id ?? ""}`));
  await expect(page.getByText("Retire a claim").first()).toBeVisible();

  // The fixture's hostile source text, which the run read, ran nothing.
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await page.context().close();
});

test("item 2: rerunning with the same seed shows the same sample, and another seed doesn't", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  const earlier = { id: await finishedRun(request, "42") };
  await page.goto(`/vaults/${world.vault}/audit/${earlier.id}`);
  await expect(page.getByTestId("sample")).toBeVisible();
  const before = await sampleText(page);
  const again = await startRun(page, "42", "50");
  await expect(page.getByTestId("run-state")).toHaveText("Finished", { timeout: 120_000 });
  const after = await sampleText(page);
  expect(after).toBe(before);
  const other = await startRun(page, "43", "10");
  await expect(page.getByTestId("run-state")).toHaveText("Finished", { timeout: 120_000 });
  const different = await sampleText(page);
  const a = await get<{ sample: string[] }>(request, api(`/audit/runs/${earlier.id}`));
  const b = await get<{ sample: string[] }>(request, api(`/audit/runs/${again}`));
  const c = await get<{ sample: string[] }>(request, api(`/audit/runs/${other}`));
  expect(b.sample).toEqual(a.sample);
  expect(c.sample.slice(0, 10)).not.toEqual(a.sample.slice(0, 10));
  expect(different).toBe(c.sample.join(" "));
  evidence(`item 2: seed 42 twice gave the same ${String(a.sample.length)} claims in the same order on screen and in the API; seed 43's first 10 differ`);
  await page.context().close();
});

test("item 5: a budget that runs out shows as stopped by budget, not as failed", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  const id = await startRun(page, "44", "20", "1");
  await expect(page.getByTestId("run-state")).toHaveText("Stopped by budget", { timeout: 120_000 });
  await expect(page.getByTestId("run-explain")).toContainText("isn't a failure");
  const wire = await get<{ run: RunWire }>(request, api(`/audit/runs/${id}`));
  expect(wire.run.state).toBe("finished");
  expect(wire.run.stop).toBe("run_budget");
  expect(wire.run.llm_calls).toBe(0);
  await page.goto(`/vaults/${world.vault}/audit`);
  await expect(page.locator(`tr[data-run="${id}"]`).getByTestId("run-state")).toHaveText("Stopped by budget");
  await expect(page.getByText("Failed", { exact: true })).toHaveCount(0);
  evidence(`item 5: budget 1 stopped run ${id} with stop=${wire.run.stop ?? ""}, state ${wire.run.state}, ${String(wire.run.llm_calls)} LLM calls; the screen says Stopped by budget`);
  await page.context().close();
});

test("item 4: a recall figure from too few planted errors shows the warning", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  const report = await get<{ overall_planted: number; too_small: boolean; overall_recall: number | null; llm_evidence: string }>(request, api("/audit/recall"));
  expect(report.too_small).toBe(true);
  await page.goto(`/vaults/${world.vault}/audit/-/recall`);
  await expect(page.getByTestId("recall-warning").first()).toContainText("Too few planted errors to trust this figure.");
  await expect(page.getByTestId("sample-size")).toContainText(`${String(report.overall_planted)} planted errors`);
  evidence(`item 4: ${String(report.overall_planted)} planted, recall ${String(report.overall_recall)}, llm_evidence ${report.llm_evidence}; the warning shows`);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.context().close();
});

interface UsageWire { rows: { agent: string; model: string; vault_id: string; day: string; derivations: number; input_tokens: number; output_tokens: number; thinking_tokens: number; cost_usd: number | null }[] }

const month = () => {
  const now = new Date();
  const y = now.getUTCFullYear();
  const m = now.getUTCMonth() + 1;
  const last = new Date(Date.UTC(y, m, 0)).getUTCDate();
  const mm = String(m).padStart(2, "0");
  return { from: `${String(y)}-${mm}-01`, to: `${String(y)}-${mm}-${String(last).padStart(2, "0")}` };
};

const fmt = (n: number) => n.toLocaleString("en-US");

const compareUsage = async (page: Page, request: APIRequestContext, priced: boolean) => {
  await finishedRun(request, "42");
  const { from, to } = month();
  const report = await get<UsageWire>(request, `/api/v1/system/usage?from=${from}&to=${to}`);
  const rows = report.rows.filter((r) => r.vault_id === world.vault);
  expect(rows.some((r) => r.agent === "audit"), "audit runs count as the audit agent").toBe(true);
  expect(rows.some((r) => r.agent === "extraction")).toBe(true);
  const sum = (f: (r: UsageWire["rows"][number]) => number) => rows.reduce((n, r) => n + f(r), 0);
  await page.goto(`/vaults/${world.vault}/audit/-/agents`);
  await expect(page.getByTestId("total-count")).toHaveText(fmt(sum((r) => r.derivations)));
  await expect(page.getByTestId("total-input")).toHaveText(fmt(sum((r) => r.input_tokens)));
  await expect(page.getByTestId("total-output")).toHaveText(fmt(sum((r) => r.output_tokens)));
  await expect(page.getByTestId("total-thinking")).toHaveText(fmt(sum((r) => r.thinking_tokens)));
  await expect(page.getByTestId("total-tokens")).toHaveText(fmt(sum((r) => r.input_tokens + r.output_tokens + r.thinking_tokens)));
  const auditRow = page.locator(`tr[data-group="audit|${AUDIT_MODEL}"]`);
  await expect(auditRow.getByTestId("cost")).toHaveText("No price set");
  const cost = sum((r) => r.cost_usd ?? 0);
  if (priced) {
    expect(rows.find((r) => r.agent === "extraction")?.cost_usd).not.toBeNull();
    await expect(page.locator(`tr[data-group="extraction|${EXTRACT_MODEL}"]`).getByTestId("cost")).toHaveText(cost < 0.01 ? `$${cost.toFixed(4)}` : `$${cost.toFixed(2)}`);
    await expect(page.getByTestId("usage-total-row").getByTestId("cost")).toContainText(cost < 0.01 ? `$${cost.toFixed(4)}` : `$${cost.toFixed(2)}`);
  } else {
    expect(rows.every((r) => r.cost_usd === null)).toBe(true);
    await expect(page.getByTestId("usage-totals")).not.toContainText("$");
    await expect(page.getByTestId("usage-total-row").getByTestId("cost")).toHaveText("No price set");
  }
  const tokens = sum((r) => r.input_tokens + r.output_tokens + r.thinking_tokens);
  evidence(`item 3 (${priced ? "with a price for extraction" : "with no price"}): ${from} to ${to}, ${String(rows.length)} rows (${[...new Set(rows.map((r) => `${r.agent}/${r.model}`))].join(", ")}), ${String(sum((r) => r.derivations))} calls or runs, ${String(tokens)} tokens, cost $${cost.toFixed(6)}; the screen's totals match`);
};

test("item 3: totals on screen match the API for the month, with and without a price", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  await compareUsage(page, request, true);
  await expect(page.getByRole("figure")).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.context().close();
  // The same server, started again with no prices at all.
  await stopServer();
  serverEnv(false);
  evidence(`item 3: restarted with no MEMEX_LLM_PRICES in ${String(await startServer())} ms`);
  const again = await signIn(browser, adminToken);
  await compareUsage(again, request, false);
  await again.context().close();
});

test("item 6: an invalid schedule is refused with a clear message", async ({ browser, request }) => {
  const page = await signIn(browser, adminToken);
  const before = await get<unknown>(request, api("/audit/schedule"));
  await page.goto(`/vaults/${world.vault}/audit/-/schedule`);
  await page.getByRole("button", { name: "Edit schedule" }).click();
  await page.getByLabel("Interval, in minutes").fill("0");
  await page.getByLabel("Sample size, 1 to 1000").fill("5000");
  await page.getByRole("button", { name: "Save schedule" }).click();
  await expect(page.getByTestId("schedule-problems")).toContainText("The interval must be a whole number of minutes, at least 1.");
  await expect(page.getByTestId("schedule-problems")).toContainText("The sample size must be a whole number from 1 to 1000.");
  expect(await get<unknown>(request, api("/audit/schedule"))).toEqual(before);
  // The server holds the same rules for anyone who skips the form.
  const refused = await request.put(api("/audit/schedule"), { headers: bearer, data: { enabled: true, interval_ms: 1000, sample_size: 5, run_token_budget: 0, day_token_budget: 0 } });
  expect(refused.status()).toBe(400);
  const message = ((await refused.json()) as { error: { message: string } }).error.message;
  expect(message).toBe("the interval must be at least a minute");
  // A good one saves.
  await page.getByLabel("Run on this schedule").check();
  await page.getByLabel("Interval, in minutes").fill("90");
  await page.getByLabel("Sample size, 1 to 1000").fill("25");
  await page.getByRole("button", { name: "Save schedule" }).click();
  await expect(page.getByTestId("schedule-summary")).toContainText("every 90 minutes");
  const saved = await get<{ enabled: boolean; interval_ms: number; sample_size: number }>(request, api("/audit/schedule"));
  expect([saved.interval_ms, saved.sample_size]).toEqual([90 * 60_000, 25]);
  // Turned off again, so no scheduled run lands in the other checks.
  await send(request, api("/audit/schedule"), { ...saved, enabled: false }, 200, adminToken, "put");
  evidence(`item 6: interval 0 and sample 5000 refused in the form with both reasons and nothing saved; the API refuses interval_ms 1000 with 400 "${message}"; 90 minutes and 25 saved`);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.context().close();
});

test("item 6: a read-scope user can view runs but not start them", async ({ browser, request }) => {
  // A run to look at, even when this is the only check run.
  if ((await get<{ items: unknown[] }>(request, api("/audit/runs"))).items.length === 0) await send(request, api("/audit/runs"), { seed: "1", sample_size: 5 }, 202);
  const page = await signIn(browser, world.reader);
  await page.goto(`/vaults/${world.vault}/audit`);
  await expect(page.getByTestId("runs-table")).toBeVisible();
  await expect(page.getByRole("button", { name: "Start run" })).toHaveCount(0);
  await expect(page.getByTestId("read-only")).toContainText("need admin scope");
  const runs = await get<{ items: { id: string }[] }>(request, api("/audit/runs"), world.reader);
  const one = runs.items[0];
  if (one === undefined) throw new Error("no runs");
  await page.goto(`/vaults/${world.vault}/audit/${one.id}`);
  await expect(page.getByTestId("run-state")).toBeVisible();
  await expect(page.getByTestId("checks-table")).toBeVisible();
  await page.goto(`/vaults/${world.vault}/audit/-/schedule`);
  await expect(page.getByTestId("schedule-summary")).toBeVisible();
  await expect(page.getByRole("button", { name: "Edit schedule" })).toHaveCount(0);
  await page.goto(`/vaults/${world.vault}/audit/-/agents`);
  await expect(page.getByText("Agent usage needs a server admin.")).toBeVisible();
  const refused = await request.post(api("/audit/runs"), { headers: { authorization: `Bearer ${world.reader}` }, data: {} });
  expect(refused.status()).toBe(403);
  const refusedSchedule = await request.put(api("/audit/schedule"), { headers: { authorization: `Bearer ${world.reader}` }, data: { enabled: true, interval_ms: 3_600_000, sample_size: 5, run_token_budget: 0, day_token_budget: 0 } });
  expect(refusedSchedule.status()).toBe(403);
  evidence(`item 6: a read token sees runs, a run's findings, the schedule and recall, gets no Start or Edit, and the API answers 403 to starting a run and to writing the schedule`);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.context().close();
});
