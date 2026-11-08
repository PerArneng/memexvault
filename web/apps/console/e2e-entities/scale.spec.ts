import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";

/**
 * Spec 045's item 6: a table of 200,000 entities filters and scrolls without freezing, with filter response p95 ≤100 ms over the API's time.
 *
 * The pass rule, set before anything was measured:
 * - Filter: for each trial, from the form's submit to the new rows painted, in the browser, less that request's own time from the Resource Timing API. Its p95
 *   across all trials ≤100 ms, over at least 200 trials across five kinds of filter, each trial with a new value so no cache answers it.
 * - Scroll: scrolling from the first row to the last, loading every page. The longest main-thread task ≤250 ms, the p99 gap between frames ≤100 ms,
 *   every row reached, and at most 200 row elements in the DOM at any time.
 * - The sort over 200,000 loaded rows and the filter with all of them loaded are recorded, and aren't part of the rule, which is about the table as first opened.
 * A debug server is fine, as both rules are about the browser's side: the API's time is taken out or doesn't block frames.
 *
 * The rows go straight into the entity tables, as spec 023's `entity_filter_bench` does through the store. Loading them through proposals would take hours and
 * say nothing about the table. They aren't in the event log or the search index, so only the routes this table uses can see them.
 */
const COUNT = Number(process.env.MEMEX_E2E_ENTITY_COUNT ?? 200_000);
const TRIALS_PER_LANE = 40;
const OVERHEAD_P95_MS = 100;
const LONG_TASK_MS = 250;
const FRAME_GAP_P99_MS = 100;
const MAX_ROW_ELEMENTS = 200;

const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const CONTAINER = process.env.MEMEX_E2E_PG_CONTAINER;
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const TYPE = "Gadget";
let vault = "";

const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load averages:")[1]?.trim() ?? "unknown";
const percentile = (values: readonly number[], p: number) => [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.floor(values.length * p))] ?? 0;
const summary = (values: readonly number[]) => ({ n: values.length, p50: Math.round(percentile(values, 0.5) * 10) / 10, p95: Math.round(percentile(values, 0.95) * 10) / 10, p99: Math.round(percentile(values, 0.99) * 10) / 10, max: Math.round(Math.max(...values) * 10) / 10 });

const post = async <T>(request: APIRequestContext, path: string, data: unknown, status = 200): Promise<T> => {
  const res = await request.post(path, { headers: bearer, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};

/** Only this run's database, named by the same URL the server uses. */
const sql = (statement: string): string => {
  const database = new URL(process.env.MEMEX_DATABASE_URL ?? "").pathname.slice(1);
  return execFileSync("docker", ["exec", CONTAINER ?? "", "psql", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", database, "-Atc", statement], { maxBuffer: 1 << 24 }).toString().trim();
};

/** The row format is the store's. One real entity gives every shape it uses, so this can't drift from it silently: a wrong row fails the first read below. */
const load = (schema: string, template: string) => {
  const t = JSON.parse(template) as { header: { status: unknown } };
  const status = JSON.stringify(t.header.status);
  const started = Date.now();
  const field = (name: string, value: string, lookup: string) => `jsonb_build_object('field', '${name}', 'value', ${value}, 'assertion_id', '${name}' || lpad(to_hex(i), 28, '0'), 'why', 'it''s the only claim on this field', 'others', '[]'::jsonb, 'lookups', jsonb_build_array(${lookup}))`;
  const lookup = (text: string, number: string, time: string) => `jsonb_build_object('text', ${text}, 'number', ${number}, 'time_ms', ${time})`;
  sql(`
    INSERT INTO ${schema}.entities (id, row, type_name, current)
    SELECT lpad(to_hex(i), 32, '0'),
      jsonb_build_object(
        'header', jsonb_build_object('id', lpad(to_hex(i), 32, '0'), 'creator', 'bench', 'status', '${status}'::jsonb, 'version', 1, 'acl_id', null),
        'type_name', '${TYPE}', 'type_version', 1,
        'aliases', jsonb_build_array('Alias ' || i), 'alias_keys', jsonb_build_array('alias ' || i),
        'fields', jsonb_build_array(
          ${field("name", "to_jsonb('Gadget ' || i)", lookup("'gadget ' || i", "null", "null"))},
          ${field("category", "to_jsonb('Category ' || (i % 1000))", lookup("'category ' || (i % 1000)", "null", "null"))},
          ${field("price", "to_jsonb(i * 7919 % 100000)", lookup("null", "(i * 7919 % 100000)", "null"))},
          ${field("released", "to_jsonb('2026-01-01'::text)", lookup("null", "null", "1767225600000"))}
        ),
        'merged_from', '[]'::jsonb, 'redirect_to', null, 'changeset_id', 'bench', 'updated_at_ms', 0
      ),
      '${TYPE}', true
    FROM generate_series(0, ${String(COUNT - 1)}) i`);
  sql(`
    INSERT INTO ${schema}.entity_fields (entity_id, type_name, field, text_value, number_value, time_value)
    SELECT lpad(to_hex(i), 32, '0'), '${TYPE}', f.field, f.t, f.n, f.at
    FROM generate_series(0, ${String(COUNT - 1)}) i,
    LATERAL (VALUES
      ('name', 'gadget ' || i, null::double precision, null::bigint),
      ('category', 'category ' || (i % 1000), null, null),
      ('price', null, (i * 7919 % 100000)::double precision, null),
      ('released', null, null, 1767225600000::bigint),
      ('$alias', 'alias ' || i, null, null)
    ) AS f(field, t, n, at)`);
  sql(`ANALYZE ${schema}.entities; ANALYZE ${schema}.entity_fields;`);
  return Date.now() - started;
};

test.beforeAll(async ({ request }) => {
  test.setTimeout(1_800_000);
  test.skip(CONTAINER === undefined, "needs MEMEX_E2E_PG_CONTAINER, as the rows go straight into the database");
  console.log(`server started in ${String(await startServer())} ms`);
  vault = (await post<{ id: string }>(request, "/api/v1/vaults", { name: `entities-scale-${run}` }, 201)).id;
  await post(request, `/api/v1/vaults/${vault}/entity-types`, {
    name: TYPE,
    schema: { type: "object", additionalProperties: false, properties: {
      name: { type: "string", identity: true, searchable: true, filterable: true },
      category: { type: "string", filterable: true },
      price: { type: "number", filterable: true },
      released: { type: "string", format: "date", filterable: true },
      notes: { type: "string", searchable: true },
    } },
    rationale: "scale",
    idempotency_key: `scale-type-${run}`,
  });
  await post(request, `/api/v1/vaults/${vault}/entities`, { type: TYPE, aliases: ["template"], fields: [{ field: "name", value: "zz template" }], rationale: "scale", idempotency_key: `scale-template-${run}` });
  const schema = sql(`select nspname from pg_namespace where nspname like 'vault\\_${vault}%'`);
  const template = sql(`select row from ${schema}.entities limit 1`);
  const before = loadAverage();
  const ms = load(schema, template);
  const size = sql(`select pg_size_pretty(pg_total_relation_size('${schema}.entities')) || ' + ' || pg_size_pretty(pg_total_relation_size('${schema}.entity_fields'))`);
  const rows = sql(`select (select count(*) from ${schema}.entities) || ' entities, ' || (select count(*) from ${schema}.entity_fields) || ' field values'`);
  record("item 6 load", { rows, seconds: Math.round(ms / 1000), size, loadBefore: before, loadAfter: loadAverage() });
});

test.afterAll(async () => {
  await stopServer();
});

test.use({ viewport: { width: 1400, height: 1200 } });

const signIn = async (page: Page) => {
  const res = await page.context().request.post("/api/v1/session", { headers: { "x-memex-csrf": "1" }, data: { token: adminToken } });
  expect(res.ok()).toBe(true);
};

interface Trial { readonly total: number; readonly api: number; readonly rows: number }

/** Runs in the page, so the clock starts at the submit and stops after the frame with the rows in it. */
const timedSubmit = (page: Page, expectedKey: string): Promise<Trial> =>
  page.evaluate(async (key) => {
    performance.clearResourceTimings();
    const form = document.querySelector<HTMLFormElement>("form[aria-label=Filters]");
    if (form === null) throw new Error("no filter form");
    const started = performance.now();
    form.requestSubmit();
    const settled = () => {
      const status = document.querySelector(`[data-filter-key="${CSS.escape(key)}"]`) ?? document.querySelector("[data-filter-key]");
      return status?.getAttribute("data-filter-key") === key && status.getAttribute("data-settled") === "true" ? status : null;
    };
    const status = settled() ?? await new Promise<Element>((done, fail) => {
      const stop = setTimeout(() => fail(new Error(`no rows for ${key} in 60 s`)), 60_000);
      const observer = new MutationObserver(() => {
        const found = settled();
        if (found !== null) {
          clearTimeout(stop);
          observer.disconnect();
          done(found);
        }
      });
      observer.observe(document.body, { subtree: true, childList: true, attributes: true });
    });
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const total = performance.now() - started;
    const request = performance.getEntriesByType("resource").filter((e) => e.name.includes("/entities?")).at(-1) as PerformanceResourceTiming | undefined;
    if (request === undefined) throw new Error("no request was seen, so a cache answered");
    return { total, api: request.responseEnd - request.startTime, rows: Number(status.getAttribute("data-entity-count")) };
  }, expectedKey);

interface Lane {
  readonly name: string;
  readonly filters: (i: number) => readonly (readonly [string, string, string])[];
}

const LANES: readonly Lane[] = [
  { name: "exact, one entity", filters: (i) => [["name", "eq", `gadget ${String((i * 7907) % COUNT)}`]] },
  { name: "exact, ~0.1%", filters: (i) => [["category", "eq", `category ${String(i)}`]] },
  { name: "range, ~0.1%", filters: (i) => [["price", "lt", String(90 + i)]] },
  { name: "range, ~50%", filters: (i) => [["price", "lt", String(50_000 + i)]] },
  { name: "exact and range", filters: (i) => [["category", "eq", `category ${String(i + 500)}`], ["price", "lt", String(50_000 + i)]] },
];

const keyOf = (filters: readonly (readonly [string, string, string])[]) => filters.map(([f, o, v]) => `${f}:${o}:${v}`).join("&");
const tableUrl = (filters: readonly (readonly [string, string, string])[]) => `/vaults/${vault}/entities/-/types/${TYPE}?${filters.map(([f, o, v]) => `f=${encodeURIComponent(`${f}:${o}:${v}`)}`).join("&")}`;

const measureLanes = async (page: Page, trialsPerLane: number, navigate: boolean) => {
  const out: Record<string, ReturnType<typeof summary>> = {};
  const overheads: number[] = [];
  const apis: number[] = [];
  for (const lane of LANES) {
    const laneOverheads: number[] = [];
    if (navigate) await page.goto(tableUrl(lane.filters(1000)));
    else {
      // Keep what's loaded: the rows are read from the URL with no reload.
      await page.evaluate((to) => {
        history.replaceState(null, "", to);
        dispatchEvent(new PopStateEvent("popstate"));
      }, tableUrl(lane.filters(1000)));
    }
    await expect(page.locator(`[data-filter-key="${keyOf(lane.filters(1000))}"][data-settled=true]`)).toBeVisible();
    for (let i = 0; i < trialsPerLane; i += 1) {
      const filters = lane.filters(i);
      for (const [index, [, , value]] of filters.entries()) await page.locator(`#filter-${String(index)}-value`).fill(value);
      const trial = await timedSubmit(page, keyOf(filters));
      laneOverheads.push(trial.total - trial.api);
      apis.push(trial.api);
    }
    overheads.push(...laneOverheads);
    out[lane.name] = summary(laneOverheads);
  }
  return { byLane: out, overhead: summary(overheads), api: summary(apis), raw: overheads };
};

test("item 6: filtering 200,000 entities, over the API's time", async ({ page }) => {
  test.setTimeout(1_200_000);
  await signIn(page);
  const loadAtStart = loadAverage();
  await page.goto(`/vaults/${vault}/entities/-/types/${TYPE}`);
  await expect(page.locator("[data-filter-key][data-settled=true]")).toHaveAttribute("data-entity-count", "200");
  const result = await measureLanes(page, TRIALS_PER_LANE, true);
  record("item 6 filter overhead, ms", { byLane: result.byLane, overall: result.overhead, api: result.api, rule: OVERHEAD_P95_MS, loadAtStart, loadAtEnd: loadAverage() });
  expect(result.raw.length).toBeGreaterThanOrEqual(200);
  expect(result.overhead.p95).toBeLessThanOrEqual(OVERHEAD_P95_MS);
});

test("item 6: scrolling all 200,000 entities", async ({ page }) => {
  test.setTimeout(1_800_000);
  await signIn(page);
  await page.goto(`/vaults/${vault}/entities/-/types/${TYPE}`);
  const status = page.locator("[data-filter-key][data-settled=true]");
  await expect(status).toHaveAttribute("data-entity-count", "200");
  const loadAtStart = loadAverage();
  const started = Date.now();
  const measured = await page.evaluate(async (expected) => {
    const scroller = document.querySelector<HTMLElement>("[role=table]");
    if (scroller === null) throw new Error("no table");
    const gaps: number[] = [];
    const tasks: number[] = [];
    const observer = new PerformanceObserver((list) => {
      for (const e of list.getEntries()) tasks.push(e.duration);
    });
    observer.observe({ type: "longtask", buffered: false });
    let maxRowElements = 0;
    let last = performance.now();
    let rows = 0;
    const count = () => Number(document.querySelector("[data-filter-key]")?.getAttribute("data-entity-count"));
    const deadline = performance.now() + 1_500_000;
    while (performance.now() < deadline) {
      await new Promise((done) => requestAnimationFrame(done));
      const now = performance.now();
      gaps.push(now - last);
      last = now;
      scroller.scrollTop = scroller.scrollHeight;
      maxRowElements = Math.max(maxRowElements, scroller.querySelectorAll("[role=row]").length);
      rows = count();
      if (rows >= expected && document.querySelector("[data-filter-key]")?.textContent?.includes("all of them") === true) break;
    }
    observer.disconnect();
    const memory = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
    return { gaps, tasks, maxRowElements, rows, heapMb: memory === undefined ? null : Math.round(memory.usedJSHeapSize / (1 << 20)), scrollHeight: scroller.scrollHeight };
  }, COUNT);
  const seconds = Math.round((Date.now() - started) / 1000);
  const longest = measured.tasks.length === 0 ? 0 : Math.max(...measured.tasks);
  const gapSummary = summary(measured.gaps);
  record("item 6 scroll", { rows: measured.rows, seconds, gaps: gapSummary, longTasks: measured.tasks.length, longestTask: Math.round(longest), maxRowElements: measured.maxRowElements, heapMb: measured.heapMb, scrollHeight: measured.scrollHeight, rule: { longTask: LONG_TASK_MS, gapP99: FRAME_GAP_P99_MS, rowElements: MAX_ROW_ELEMENTS }, loadAtStart, loadAtEnd: loadAverage() });

  // Recorded and outside the rule: the last row is really there, a sort of everything loaded, and filtering with everything loaded.
  await expect(page.getByRole("row").last()).toContainText("Gadget");
  const sortMs = await page.evaluate(async () => {
    const started = performance.now();
    document.querySelector<HTMLButtonElement>("[role=columnheader] button")?.click();
    await new Promise<void>((done) => {
      const check = () => (document.querySelector("[data-sort-note]") === null ? requestAnimationFrame(check) : done());
      check();
    });
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    return performance.now() - started;
  });
  await expect(page.locator("[data-sort-note]")).toContainText(`${COUNT.toLocaleString("en")} rows loaded`);
  record("item 6 outside the rule, sort of all loaded rows, ms", { ms: Math.round(sortMs), load: loadAverage() });

  expect(measured.rows).toBeGreaterThanOrEqual(COUNT);
  expect(longest).toBeLessThanOrEqual(LONG_TASK_MS);
  expect(gapSummary.p99).toBeLessThanOrEqual(FRAME_GAP_P99_MS);
  expect(measured.maxRowElements).toBeLessThanOrEqual(MAX_ROW_ELEMENTS);
});
