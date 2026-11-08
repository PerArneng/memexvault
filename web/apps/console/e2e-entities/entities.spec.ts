import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type APIResponse, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";

// Spec 045. Everything runs on a real server with the real CSP. The review screen is spec 046's, so these look in the queue through the API.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);

/** Spec 018: "indexing lag has p95 ≤1 s". Spec 023's item 1 holds a new type to the same target: "target ≤1 s". */
const LAG_TARGET_MS = 1000;
const LAG_TYPES = 5;
const LAG_ENTITIES_PER_TYPE = 8;

let vault = "";
let readToken = "";
let agentToken = "";
const segments: { id: string; text: string }[] = [];
let sourceId = "";

const baseURL = () => test.info().project.use.baseURL ?? "";
const api = (path: string) => `/api/v1/vaults/${vault}${path}`;
const screen = (rest = "") => `/vaults/${vault}/entities${rest}`;
const record = (name: string, value: unknown) => {
  const line = `${name}: ${JSON.stringify(value)}`;
  console.log(line);
  if (process.env.MEMEX_E2E_EVIDENCE) appendFileSync(process.env.MEMEX_E2E_EVIDENCE, `${line}\n`);
};
const loadAverage = () => execFileSync("uptime").toString().trim().split("load averages:")[1]?.trim() ?? "unknown";
const percentile = (values: readonly number[], p: number) => [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.floor(values.length * p))] ?? 0;

/** The shared Postgres can run out of connections to other people's work, which a 503 reports. That's the machine and not this screen, so it's waited out. */
const patiently = async (call: () => Promise<APIResponse>): Promise<APIResponse> => {
  const deadline = Date.now() + 120_000;
  for (;;) {
    const res = await call();
    if (res.status() !== 503 || Date.now() > deadline) return res;
    await new Promise((done) => setTimeout(done, 1000));
  }
};

const post = async <T>(request: APIRequestContext, path: string, data: unknown, status = 200, token = adminToken): Promise<T> => {
  const res = await patiently(() => request.post(path, { headers: { authorization: `Bearer ${token}` }, data }));
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const get = async <T>(request: APIRequestContext, path: string): Promise<T> => {
  const res = await patiently(() => request.get(path, { headers: bearer }));
  expect(res.status(), `${path}: ${await res.text()}`).toBe(200);
  return (await res.json()) as T;
};

/** A 503 can come after the write landed, so a retried name would conflict with itself. Each try here gets its own name. */
const createNamed = async <T>(request: APIRequestContext, path: string, name: string, extra: Record<string, unknown>): Promise<T> => {
  for (let attempt = 0; ; attempt += 1) {
    const res = await request.post(path, { headers: bearer, data: { name: `${name}-${String(attempt)}`, ...extra } });
    if (res.status() === 201) return (await res.json()) as T;
    expect([409, 503], `${path}: ${await res.text()}`).toContain(res.status());
    expect(attempt, `${path} kept failing`).toBeLessThan(60);
    await new Promise((done) => setTimeout(done, 2000));
  }
};

const newSession = async (browser: Browser, token = adminToken): Promise<{ context: BrowserContext; page: Page; problems: string[] }> => {
  const context = await browser.newContext({ baseURL: baseURL(), colorScheme: "dark", viewport: { width: 1400, height: 1200 } });
  const res = await context.request.post("/api/v1/session", { headers: { "x-memex-csrf": "1" }, data: { token } });
  expect(res.ok()).toBe(true);
  const page = await context.newPage();
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") problems.push(m.text());
  });
  return { context, page, problems };
};
const cspProblems = (problems: readonly string[]) => problems.filter((p) => /content security policy/i.test(p));

const productSchema = (over: Record<string, unknown> = {}) => ({
  type: "object",
  additionalProperties: false,
  required: ["name"],
  properties: {
    name: { type: "string", identity: true, searchable: true, filterable: true },
    owner: { type: "string", filterable: true, searchable: true },
    price: { type: "number", filterable: true },
    released: { type: "string", format: "date", filterable: true },
    active: { type: "boolean", filterable: true },
    notes: { type: "string", searchable: true, description: "<b>Free text</b>" },
    ...over,
  },
});

type Entity = { entity_id: string; changeset?: { id: string; state: string } | null };

const makeEntity = (request: APIRequestContext, type: string, values: Record<string, unknown>, aliases: readonly string[] = [], key = `${type}-${JSON.stringify(values)}-${run}`) =>
  post<Entity>(request, api("/entities"), { type, aliases, fields: Object.entries(values).map(([field, value]) => ({ field, value })), rationale: "fixture", idempotency_key: key });
const makeType = (request: APIRequestContext, name: string, schema: unknown = productSchema()) => post(request, api("/entity-types"), { name, schema, rationale: "fixture", idempotency_key: `type-${name}-${run}` });
const pending = async (request: APIRequestContext) => (await get<{ items: { id?: string; changeset?: { id: string } }[] }>(request, api("/review/queue?state=pending&limit=200"))).items.map((i) => i.changeset?.id ?? i.id);

const fillField = async (page: Page, prefix: string, field: string, value: string) => page.locator(`#${prefix}-${field}`).fill(value);

/** Fills the first field row, or the row at this place. */
const typeFieldRow = (page: Page, index: number) => page.locator("[data-draft-field]").nth(index);

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  console.log(`server started in ${String(await startServer())} ms`);
  vault = (await createNamed<{ id: string }>(request, "/api/v1/vaults", `entities-${run}`, {})).id;
  const makeToken = async (name: string, kind: string, scope: string) => {
    const principal = await createNamed<{ id: string }>(request, "/api/v1/principals", `${name}-${run}`, { kind });
    return (await post<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: principal.id, grants: [{ vault_id: vault, scope }] }, 201)).token;
  };
  readToken = await makeToken("reader", "human", "read");
  agentToken = await makeToken("agent", "agent", "propose");

  const pdf = readFileSync(join(repo, "crates/memex-pdf/tests/fixtures/clean_pages.pdf"));
  const upload = await request.post(api("/sources?name=entity-evidence.pdf"), { headers: { ...bearer, "content-type": "application/octet-stream" }, data: pdf });
  expect(upload.status(), await upload.text()).toBeLessThan(300);
  sourceId = ((await upload.json()) as { source: { id: string } }).source.id;
  await expect.poll(async () => {
    const { items } = await get<{ items: { id: string }[] }>(request, api(`/sources/${sourceId}/renditions`));
    const rendition = items[0];
    if (rendition === undefined) return 0;
    const found = await get<{ items: { id: string; text: string }[] }>(request, api(`/renditions/${rendition.id}/segments?limit=200`));
    segments.splice(0, segments.length, ...found.items);
    return segments.length;
  }, { timeout: 300_000, intervals: [1000] }).toBeGreaterThanOrEqual(3);
});

test.afterAll(async () => {
  await stopServer();
});

const quoteOf = (segment: { text: string }, skip = 0) => segment.text.split(/\s+/).filter((w) => w !== "").slice(skip, skip + 4).join(" ");
const segmentAt = (index: number) => {
  const segment = segments[index];
  if (segment === undefined) throw new Error(`no segment ${String(index)}`);
  return segment;
};

test("item 1: a new type made in the UI is usable and searchable within the lag target, with no frontend change or reload", async ({ browser, request }) => {
  const { context, page, problems } = await newSession(browser);
  await page.goto(screen());
  await expect(page.getByRole("heading", { name: "Types" })).toBeVisible();
  // Anything that reloaded the page would lose this.
  await page.evaluate(() => {
    (window as unknown as { stillThisPage: boolean }).stillThisPage = true;
  });
  const lags: number[] = [];
  for (let t = 0; t < LAG_TYPES; t += 1) {
    const type = `Lag${run}T${String(t)}`;
    const extra = `extra${String(t)}`;
    await page.getByRole("link", { name: "Entities", exact: true }).click();
    await page.getByRole("link", { name: "New type" }).click();
    await page.getByLabel("Type name").fill(type);
    await page.getByRole("button", { name: "Add a field" }).click();
    const row = typeFieldRow(page, 1);
    await row.getByLabel("Field name").fill(extra);
    await row.getByLabel("Holds").selectOption("number");
    await row.getByLabel("Filterable").check();
    await expect(page.getByRole("button", { name: "Submit" })).toBeDisabled();
    await page.getByRole("button", { name: "Preview the change" }).click();
    await expect(page.locator("[data-preview-change=new]")).toBeVisible();
    await expect(page.getByText("No approval needed.")).toBeVisible();
    await page.getByRole("button", { name: "Submit" }).click();
    await expect(page.getByRole("heading", { name: type })).toBeVisible();
    await expect(page.getByRole("columnheader", { name: extra })).toHaveCount(0); // There's nothing in the table yet: it says so.
    await expect(page.getByText("No entities of this type yet.")).toBeVisible();

    for (let e = 0; e < LAG_ENTITIES_PER_TYPE; e += 1) {
      const word = `marker${run}x${String(t)}y${String(e)}`;
      await page.getByRole("link", { name: "New entity" }).click();
      await fillField(page, "new", "name", word);
      await fillField(page, "new", extra, String(100 + e));
      const posted = page.waitForResponse((r) => r.request().method() === "POST" && r.url().endsWith("/entities"));
      await page.getByRole("button", { name: "Propose this entity" }).click();
      const body = (await (await posted).json()) as Entity;
      const committedAt = Date.now();
      expect(body.changeset?.state).toBe("committed");
      for (;;) {
        const { hits } = await get<{ hits: { id: string }[] }>(request, api(`/search/lexical?q=${word}&kind=entity&limit=5`));
        if (hits.length > 0) break;
        if (Date.now() - committedAt > 30_000) throw new Error(`${word} wasn't found in 30 s`);
      }
      lags.push(Date.now() - committedAt);
      await expect(page.locator("[data-entity]")).toBeVisible();
      await page.locator("[data-entity]").getByRole("link", { name: type }).click();
    }

    // Usable: its generated column, its filter and the screen's own search, all without touching the frontend.
    await expect(page.getByRole("columnheader", { name: extra })).toBeVisible();
    await page.getByRole("button", { name: "Add a filter" }).click();
    await page.locator("#filter-0-field").selectOption(extra);
    await page.locator("#filter-0-op").selectOption("gte");
    await page.locator("#filter-0-value").fill("102");
    await page.getByRole("button", { name: "Apply filters" }).click();
    await expect(page.locator("[data-filter-key][data-settled=true]")).toHaveAttribute("data-entity-count", String(LAG_ENTITIES_PER_TYPE - 2));
    await page.getByLabel("Find by name or keyword").fill(`marker${run}x${String(t)}y0`);
    await page.getByRole("button", { name: "Find", exact: true }).click();
    await expect(page.locator("[data-find-hit]")).toHaveCount(1);
  }
  expect(await page.evaluate(() => (window as unknown as { stillThisPage?: boolean }).stillThisPage)).toBe(true);
  const p95 = percentile(lags, 0.95);
  record("item 1 lag, ms", { n: lags.length, p50: percentile(lags, 0.5), p95, max: Math.max(...lags), target: LAG_TARGET_MS, load: loadAverage() });
  expect(cspProblems(problems)).toEqual([]);
  expect(p95).toBeLessThanOrEqual(LAG_TARGET_MS);
  await context.close();
});

test("the type list comes from the server, and versions and the schema show", async ({ browser, request }) => {
  const name = `Listed${run}`;
  await makeType(request, name);
  const { context, page } = await newSession(browser);
  await page.goto(screen());
  await expect(page.getByRole("link", { name, exact: true })).toBeVisible();
  // Seed types are data like any other.
  await expect(page.getByRole("link", { name: "Person", exact: true })).toBeVisible();
  // The command palette opens a type's table, as the rest of the screen does.
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder(/./).first().fill(name);
  await page.getByRole("option", { name }).first().click();
  await expect(page).toHaveURL(new RegExp(`/entities/-/types/${name}$`));
  await page.goto(screen(`/-/types/${name}/schema`));
  await expect(page.locator("[data-schema-field=price]")).toContainText("number");
  await expect(page.locator("[data-schema-field=name]")).toContainText("identity");
  await expect(page.locator("[data-type-version=\"1\"]")).toContainText("new");
  await context.close();
});

test("the preview says additive and needs no approval, and that's what a submission does", async ({ browser, request }) => {
  const name = `Grow${run}`;
  await makeType(request, name);
  const { context, page } = await newSession(browser);
  await page.goto(screen(`/-/types/${name}/edit`));
  await page.getByRole("button", { name: "Add a field" }).click();
  const added = typeFieldRow(page, 6);
  await added.getByLabel("Field name").fill("color");
  await page.getByRole("button", { name: "Preview the change" }).click();
  await expect(page.locator("[data-preview-change=additive][data-preview-decision=auto_commit]")).toBeVisible();
  await expect(page.getByLabel("Preview")).toContainText("the optional field color is added");
  // Editing after a preview asks for a new one, so a person can't submit something they didn't see described.
  await added.getByLabel("Filterable").check();
  await expect(page.getByRole("button", { name: "Submit" })).toBeDisabled();
  await page.getByRole("button", { name: "Preview the change" }).click();
  await expect(page.getByRole("button", { name: "Submit" })).toBeEnabled();
  await page.getByRole("button", { name: "Submit" }).click();
  await expect(page).toHaveURL(new RegExp(`/-/types/${name}$`));
  const versions = await get<{ items: { version: number }[] }>(request, api(`/entity-types/${name}/versions`));
  expect(versions.items.map((v) => v.version)).toEqual([1, 2]);
  await context.close();
});

test("item 2: each field value on a fixture entity links to a highlighted source region", async ({ browser, request }) => {
  const type = `Traced${run}`;
  await makeType(request, type);
  const { context, page, problems } = await newSession(browser);
  await page.goto(screen(`/-/types/${type}/new`));
  const values = [
    { field: "name", value: `Traced ${run}`, segment: segmentAt(0), quote: quoteOf(segmentAt(0)) },
    { field: "owner", value: "An owner", segment: segmentAt(1), quote: quoteOf(segmentAt(1)) },
    { field: "price", value: "42", segment: segmentAt(2), quote: quoteOf(segmentAt(2)) },
  ];
  for (const v of values) {
    await fillField(page, "new", v.field, v.value);
    const box = page.locator(`[data-field-input=${v.field}]`);
    await box.locator("summary").click();
    await page.locator(`#new-${v.field}-segment`).fill(v.segment.id);
    await page.locator(`#new-${v.field}-quote`).fill(v.quote);
  }
  await page.getByRole("button", { name: "Propose this entity" }).click();
  await expect(page.locator("[data-entity]")).toBeVisible();
  const detailUrl = page.url();
  let highlighted = 0;
  for (const v of values) {
    await page.goto(detailUrl);
    const card = page.locator(`[data-field=${v.field}]`);
    await expect(card.locator("[data-claim-value]").first()).toHaveText(v.value);
    await expect(card.locator("[data-evidence-row] q")).toHaveText(v.quote);
    await card.getByRole("link", { name: "Show it in the source" }).click();
    await expect(page).toHaveURL(new RegExp(`/documents/${sourceId}\\?.*claim=.*evidence=`));
    await expect(page.locator("[data-highlight]").first()).toBeAttached();
    const boxes = await page.locator("[data-highlight]").evaluateAll((all) => all.map((el) => el.getBoundingClientRect()).filter((r) => r.width > 0 && r.height > 0).length);
    expect(boxes).toBeGreaterThan(0);
    highlighted += 1;
  }
  record("item 2", { fields: values.length, highlighted, load: loadAverage() });
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("item 3: conflicting values show together, with the current choice and why", async ({ browser, request }) => {
  const type = `Conflicted${run}`;
  await makeType(request, type);
  const made = await post<Entity>(request, api("/entities"), {
    type,
    aliases: [],
    fields: [
      { field: "name", value: "Pump" },
      { field: "owner", value: "First owner", evidence: [{ segment_id: segmentAt(0).id, quote: quoteOf(segmentAt(0)) }] },
    ],
    rationale: "fixture",
    idempotency_key: `conflict-${run}`,
  });
  const { context, page, problems } = await newSession(browser);
  // The second value goes in through the screen, beside the first, as the field's second source.
  await page.goto(screen(`/${made.entity_id}/edit`));
  await fillField(page, "edit", "owner", "Second owner");
  await page.locator("[data-field-input=owner] summary").click();
  await page.locator("#edit-owner-segment").fill(segmentAt(1).id);
  await page.locator("#edit-owner-quote").fill(quoteOf(segmentAt(1)));
  await page.getByRole("button", { name: "Propose this edit" }).click();
  await expect(page.getByText("This edit is saved.")).toBeVisible();
  await page.goto(screen(`/${made.entity_id}`));
  const card = page.locator("[data-field=owner]");
  const current = card.locator("[data-claim-role=current]");
  const conflict = card.locator("[data-claim-role=conflict]");
  await expect(current).toBeVisible();
  await expect(conflict).toHaveCount(1);
  await expect(current).toContainText("Current choice");
  await expect(conflict).toContainText("Not chosen");
  const values = [await current.locator("[data-claim-value]").textContent(), await conflict.locator("[data-claim-value]").textContent()];
  expect(values.sort()).toEqual(["First owner", "Second owner"]);
  // Why, in the server's own words, which is what makes it the choice and not this screen's guess.
  const detail = await get<{ fields: { field: string; why: string }[] }>(request, api(`/entities/${made.entity_id}`));
  const why = detail.fields.find((f) => f.field === "owner")?.why ?? "";
  expect(why).not.toBe("");
  await expect(card.locator("[data-field-why]")).toHaveText(`Why this one: ${why}`);
  // Both trace to their own place.
  await expect(current.locator("[data-evidence-row]")).toHaveCount(1);
  await expect(conflict.locator("[data-evidence-row]")).toHaveCount(1);
  await expect(card.getByText("1 conflict")).toBeVisible();
  // The table marks it too.
  await page.goto(screen(`/-/types/${type}`));
  await expect(page.getByRole("row").filter({ hasText: "Pump" }).getByText("1 conflict")).toBeVisible();
  record("item 3", { why });
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("item 4: a breaking type change and a merge end up in the review queue, not applied", async ({ browser, request }) => {
  const type = `Governed${run}`;
  await makeType(request, type);
  const keep = await makeEntity(request, type, { name: "Duplicate name", owner: "A", price: 1 }, ["same alias"], `keep-${run}`);
  const drop = await makeEntity(request, type, { name: "Duplicate name", owner: "B" }, ["same alias", "only here"], `drop-${run}`);
  const { context, page, problems } = await newSession(browser);

  // A breaking type change.
  await page.goto(screen(`/-/types/${type}/edit`));
  await page.getByRole("button", { name: "Remove notes" }).click();
  await page.getByRole("button", { name: "Preview the change" }).click();
  await expect(page.locator("[data-preview-change=breaking][data-preview-decision=review]")).toBeVisible();
  await expect(page.getByLabel("Preview")).toContainText("the field notes is removed");
  await expect(page.getByText("A person has to approve this.")).toBeVisible();
  await page.getByRole("button", { name: "Submit" }).click();
  const heldType = page.locator("[data-proposal=pending]");
  await expect(heldType).toBeVisible();
  const typeChangeset = (await heldType.getAttribute("data-changeset")) ?? "";
  expect(typeChangeset).not.toBe("");
  await expect(heldType.getByRole("link", { name: "Open it in Review" })).toHaveAttribute("href", `/vaults/${vault}/review/${typeChangeset}`);
  expect(await pending(request)).toContain(typeChangeset);
  const versions = await get<{ items: { version: number; schema: { properties: Record<string, unknown> } }[] }>(request, api(`/entity-types/${type}/versions`));
  expect(versions.items).toHaveLength(1);
  expect(Object.keys(versions.items[0]?.schema.properties ?? {})).toContain("notes");

  // A merge, from the same-as suggestions, with its preview.
  await page.goto(screen(`/-/types/${type}/same-as`));
  await expect(page.locator("[data-same-as]").first()).toBeVisible();
  const [low, high] = [keep.entity_id, drop.entity_id].sort() as [string, string];
  await page.locator("[data-same-as='duplicate name']").getByRole("link", { name: /Preview merging/ }).click();
  await expect(page).toHaveURL(new RegExp(`/${high}/merge/${low}$`));
  await expect(page.locator("[data-merge-preview]")).toBeVisible();
  await expect(page.locator("[data-merge-row=name]")).toHaveAttribute("data-merge-outcome", "same");
  await expect(page.locator("[data-merge-row=owner]")).toHaveAttribute("data-merge-outcome", "conflict");
  await page.getByRole("button", { name: "Send this merge to review" }).click();
  const heldMerge = page.locator("[data-proposal=pending]");
  await expect(heldMerge).toBeVisible();
  const mergeChangeset = (await heldMerge.getAttribute("data-changeset")) ?? "";
  expect(await pending(request)).toContain(mergeChangeset);
  const after = await get<{ id: string; redirected_from?: string | null }>(request, api(`/entities/${high}`));
  expect(after.id).toBe(high);
  expect(after.redirected_from ?? null).toBeNull();

  // A split waits as well, and shows what would move.
  await page.goto(screen(`/${low}/split`));
  // The name stays, as this entity would be invalid without it, which the server refuses: moving something it needs is a refusal to show, not a queue entry.
  await page.locator("[data-split-claim]").filter({ hasText: "owner" }).locator("input").check();
  await expect(page.locator("[data-split-moved='1']")).toBeVisible();
  await fillField(page, "split", "name", "A new name");
  await page.getByRole("button", { name: "Send this split to review" }).click();
  const heldSplit = page.locator("[data-proposal=pending]");
  await expect(heldSplit).toBeVisible();
  expect(await pending(request)).toContain((await heldSplit.getAttribute("data-changeset")) ?? "");

  // An agent's new entity waits too, with the same notice.
  const agent = await newSession(browser, agentToken);
  await agent.page.goto(screen(`/-/types/${type}/new`));
  await fillField(agent.page, "new", "name", "From an agent");
  await agent.page.getByRole("button", { name: "Propose this entity" }).click();
  // A machine's value needs evidence, and the server's reason is what the page shows.
  await expect(agent.page.getByText("This entity was refused.")).toBeVisible();
  await expect(agent.page.getByRole("alert").filter({ hasText: "has no evidence" })).toBeVisible();
  await agent.page.locator("[data-field-input=name] summary").click();
  await agent.page.locator("#new-name-segment").fill(segmentAt(0).id);
  await agent.page.locator("#new-name-quote").fill(quoteOf(segmentAt(0)));
  await agent.page.getByRole("button", { name: "Propose this entity" }).click();
  await expect(agent.page.locator("[data-proposal=pending]")).toBeVisible();
  await agent.context.close();

  // Approving the merge is spec 046's screen. Through the API, the old address then leads to the entity that stayed, and says so.
  await post(request, api(`/changesets/${mergeChangeset}/approve`), { reason: "the same pump" });
  await page.goto(screen(`/${high}`));
  await expect(page.locator(`[data-redirected-from="${high}"]`)).toBeVisible();
  await expect(page.locator("[data-entity]")).toHaveAttribute("data-entity", low);
  await expect(page.locator("[data-aliases]")).toContainText("only here");
  expect(cspProblems(problems)).toEqual([]);
  record("item 4", { typeChangeset, mergeChangeset, load: loadAverage() });
  await context.close();
});

const HOSTILE = '<img src=x onerror="window.pwned=1"><script>window.pwned=2</script><b>bold</b>';

test("item 7: field values containing HTML render as plain text", async ({ browser, request }) => {
  const type = `Hostile${run}`;
  await makeType(request, type);
  const made = await post<Entity>(request, api("/entities"), {
    type,
    aliases: [`${HOSTILE} alias`],
    fields: [
      { field: "name", value: `${HOSTILE} name` },
      { field: "owner", value: `${HOSTILE} owner one` },
      { field: "notes", value: `${HOSTILE} notes` },
    ],
    rationale: "fixture",
    idempotency_key: `hostile-${run}`,
  });
  const other = await makeEntity(request, type, { name: `${HOSTILE} other` }, [], `hostile-other-${run}`);
  await post(request, api(`/entities/${made.entity_id}/edit`), { base_version: 1, fields: [{ field: "owner", value: `${HOSTILE} owner two` }], rationale: "fixture", idempotency_key: `hostile-edit-${run}` });
  const { context, page, problems } = await newSession(browser);
  const noMarkup = async () => {
    // Nothing the values held became an element, and nothing they held ran.
    expect(await page.locator("main img, main script, main b").count()).toBe(0);
    expect(await page.evaluate(() => (window as unknown as { pwned?: number }).pwned)).toBeUndefined();
  };
  const rawText = page.getByText(HOSTILE, { exact: false });

  await page.goto(screen(`/-/types/${type}`));
  await expect(rawText.first()).toBeVisible();
  await noMarkup();

  await page.goto(screen(`/${made.entity_id}`));
  await expect(page.locator("[data-field=notes] [data-claim-value]").first()).toHaveText(`${HOSTILE} notes`);
  await expect(page.locator("[data-field=owner] [data-claim-role=conflict] [data-claim-value]")).toHaveCount(1);
  await expect(page.locator("[data-aliases]")).toHaveText(`${HOSTILE} alias`);
  await noMarkup();

  await page.goto(screen(`/${made.entity_id}/merge/${other.entity_id}`));
  await expect(page.locator("[data-merge-row=name]")).toContainText(`${HOSTILE} name`);
  await noMarkup();

  await page.goto(screen(`/${made.entity_id}/split`));
  await expect(page.locator("[data-split-claim]").first()).toContainText(HOSTILE);
  await noMarkup();

  await page.goto(screen(`/-/types/${type}?find=${encodeURIComponent("other")}`));
  await expect(page.locator("[data-find-hit]").first()).toContainText(HOSTILE);
  await noMarkup();

  // A schema's description is vault content too.
  await page.goto(screen(`/-/types/${type}/schema`));
  await expect(page.locator("[data-schema-field=notes]")).toContainText("<b>Free text</b>");
  await noMarkup();
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("filters, columns and sort live in the URL, and the sort says it only covers loaded rows", async ({ browser, request }) => {
  const type = `Tabled${run}`;
  await makeType(request, type);
  for (let i = 0; i < 5; i += 1) await makeEntity(request, type, { name: `Row ${String(i)}`, owner: i % 2 === 0 ? "Even Owner" : "Odd", price: 100 - i, released: `2026-01-0${String(i + 1)}`, active: i % 2 === 0 }, [], `table-${String(i)}-${run}`);
  const { context, page, problems } = await newSession(browser);
  await page.goto(screen(`/-/types/${type}`));
  const status = page.locator("[data-filter-key][data-settled=true]");
  await expect(status).toHaveAttribute("data-entity-count", "5");
  await expect(status).toContainText("which is all of them");

  // Typed filters: each kind's input and operators come from the schema. Text folds on the server.
  const apply = async (field: string, op: string, value: string, expected: string) => {
    await page.goto(screen(`/-/types/${type}`));
    await page.getByRole("button", { name: "Add a filter" }).click();
    await page.locator("#filter-0-field").selectOption(field);
    await page.locator("#filter-0-op").selectOption(op);
    const input = page.locator("#filter-0-value");
    if ((await input.evaluate((el) => el.tagName)) === "SELECT") await input.selectOption(value);
    else await input.fill(value);
    await page.getByRole("button", { name: "Apply filters" }).click();
    await expect(page.locator(`[data-filter-key="${field}:${op}:${value}"][data-settled=true]`)).toHaveAttribute("data-entity-count", expected);
  };
  await apply("price", "lt", "98", "2");
  await apply("owner", "eq", "even owner", "3");
  await apply("released", "gte", "2026-01-04", "2");
  await apply("active", "eq", "false", "2");
  await expect(page.locator("#filter-0-op option")).toHaveCount(1);

  // A value the server would refuse is stopped in the page, and a URL with a stale field gets the server's own message.
  await page.goto(screen(`/-/types/${type}`));
  await page.getByRole("button", { name: "Add a filter" }).click();
  await page.locator("#filter-0-field").selectOption("owner");
  await page.getByRole("button", { name: "Apply filters" }).click();
  await expect(page.getByText("Give a value for owner.")).toBeVisible();

  // A search that looks like a number stays what was asked, whatever the router does to the URL.
  await page.goto(screen(`/-/types/${type}`));
  await page.getByLabel("Find by name or keyword").fill("123");
  await page.getByRole("button", { name: "Find", exact: true }).click();
  await expect(page.locator("[data-find='123']")).toBeVisible();
  await page.reload();
  await expect(page.locator("[data-find='123']")).toBeVisible();

  // Reload keeps it all.
  const url = screen(`/-/types/${type}?f=${encodeURIComponent("price:gte:97\nprice:lt:100")}&cols=name,price&sort=price:desc`);
  await page.goto(url);
  await expect(page.locator("[data-filter-key][data-settled=true]")).toHaveAttribute("data-entity-count", "3");
  await expect(page.getByRole("columnheader")).toHaveText(["name", "price ↓", "Conflicts"]);
  const note = page.locator("[data-sort-note]");
  await expect(note).toContainText("within the 3 rows loaded so far, not across every row");
  await expect(page.getByRole("columnheader", { name: "price" })).toHaveAttribute("aria-sort", "descending");
  await expect(page.getByRole("row").nth(1)).toContainText("99");
  await page.reload();
  await expect(page.locator("#filter-1-value")).toHaveValue("100");

  // Column choice.
  await page.locator("[data-column-chooser] summary").click();
  await page.getByRole("checkbox", { name: "owner" }).check();
  await expect(page).toHaveURL(/cols=/);
  await expect(page.getByRole("columnheader", { name: "owner" })).toBeVisible();
  await page.getByRole("button", { name: "Clear the sort" }).click();
  await expect(note).toHaveCount(0);
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("a read token sees everything and is offered no writes, and a typed write URL says why not", async ({ browser, request }) => {
  const type = `Reader${run}`;
  await makeType(request, type);
  const made = await makeEntity(request, type, { name: "Read me" }, [], `reader-${run}`);
  const { context, page } = await newSession(browser, readToken);
  await page.goto(screen());
  await expect(page.getByRole("link", { name: "New type" })).toHaveCount(0);
  await page.goto(screen(`/-/types/${type}`));
  await expect(page.getByRole("link", { name: "Read me" })).toBeVisible();
  await expect(page.getByRole("link", { name: "New entity" })).toHaveCount(0);
  await page.goto(screen(`/${made.entity_id}`));
  await expect(page.locator("[data-entity]")).toBeVisible();
  await expect(page.getByRole("link", { name: "Propose a merge" })).toHaveCount(0);
  for (const path of [`/-/new-type`, `/-/types/${type}/edit`, `/-/types/${type}/new`, `/${made.entity_id}/edit`, `/${made.entity_id}/merge`, `/${made.entity_id}/split`]) {
    await page.goto(screen(path));
    await expect(page.getByText("You don't have permission to propose changes in this vault.")).toBeVisible();
  }
  await context.close();
});

test("the graph panel shows nearby objects through the neighborhood API", async ({ browser, request }) => {
  const type = `Linked${run}`;
  await makeType(request, type);
  const made = await makeEntity(request, type, { name: "Linked entity" }, [], `linked-${run}`);
  const detail = await get<{ fields: { field: string; current: { assertion_id: string } }[] }>(request, api(`/entities/${made.entity_id}`));
  const claim = detail.fields[0]?.current.assertion_id ?? "";
  const { context, page, problems } = await newSession(browser);
  await page.goto(screen(`/${made.entity_id}`));
  await expect(page.locator("[data-graph-empty]")).toBeVisible();

  const title = `Citing page ${run}`;
  const pageMade = await post<{ page_id: string }>(request, api("/pages"), { title, body: `This cites {{claim:${claim}}}.\n`, rationale: "fixture", idempotency_key: `citing-${run}` });
  await page.reload();
  const node = page.locator(`[data-graph-node="${pageMade.page_id}"]`);
  await expect(node).toContainText(title);
  await expect(node).toContainText("claim_ref via name");
  await expect(page.locator("[data-graph] svg")).toHaveAttribute("aria-label", /1 objects near Linked entity/);
  await node.getByRole("link", { name: title }).click();
  await expect(page).toHaveURL(new RegExp(`/pages/${pageMade.page_id}`));
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("loading, empty, error and malformed states all read properly", async ({ browser, request }) => {
  const type = `States${run}`;
  await makeType(request, type);
  const { context, page, problems } = await newSession(browser);

  await page.goto(screen(`/-/types/NoSuchType${run}`));
  await expect(page.getByText("Couldn't find this type.")).toBeVisible();
  await page.goto(screen("/no-such-entity"));
  await expect(page.getByText("Couldn't find this entity.")).toBeVisible();
  await page.goto(screen("/-/nonsense/address"));
  await expect(page.getByText("There's nothing at this address.")).toBeVisible();

  // A server error on the rows, with its request id, and a retry that works.
  let failing = true;
  await page.route("**/entities?*", async (route) => {
    if (failing) await route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: { code: "internal", message: "It broke", request_id: "req-e2e" } }) });
    else await route.continue();
  });
  await page.goto(screen(`/-/types/${type}`));
  const table = page.getByRole("region", { name: `${type} entities` });
  await expect(table.getByText("It broke")).toBeVisible();
  await expect(table.getByText("request id: req-e2e")).toBeVisible();
  failing = false;
  await table.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("No entities of this type yet.")).toBeVisible();

  // A schema this console can't make sense of comes from the server's own response, so the screen has to survive it.
  await page.route(`**/entity-types/${type}/versions`, (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [{ name: type, version: 1, schema: { properties: { good: { type: "string" }, bad: "not an object" } }, change: "new", reasons: [], created_at_ms: 1 }] }) }));
  await page.goto(screen(`/-/types/${type}`));
  await expect(page.getByText("Part of this type's schema can't be shown.")).toBeVisible();
  await expect(page.getByText("The field bad isn't described by an object")).toBeVisible();
  await page.unroute(`**/entity-types/${type}/versions`);
  await page.route(`**/entity-types/${type}/versions`, (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [{ name: type, version: 1, schema: "nonsense", change: "new", reasons: [], created_at_ms: 1 }] }) }));
  await page.goto(screen(`/-/types/${type}/schema`));
  await expect(page.getByText("This type's schema isn't an object")).toBeVisible();

  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});

test("axe finds no violations on the type list, the editor, the table and the detail", async ({ browser, request }) => {
  const type = `Axe${run}`;
  await makeType(request, type);
  const a = await post<Entity>(request, api("/entities"), { type, aliases: ["An alias"], fields: [{ field: "name", value: "Axe one" }, { field: "owner", value: "First", evidence: [{ segment_id: segmentAt(0).id, quote: quoteOf(segmentAt(0)) }] }], rationale: "fixture", idempotency_key: `axe-${run}` });
  await post(request, api(`/entities/${a.entity_id}/edit`), { base_version: 1, fields: [{ field: "owner", value: "Second" }], rationale: "fixture", idempotency_key: `axe-edit-${run}` });
  const b = await makeEntity(request, type, { name: "Axe two" }, [], `axe-two-${run}`);
  const { context, page, problems } = await newSession(browser);
  const check = async (what: string, path: string, ready: string) => {
    await page.goto(screen(path));
    await expect(page.locator(ready).first()).toBeVisible();
    const { violations } = await new AxeBuilder({ page }).analyze();
    expect(violations.map((v) => `${what}: ${v.id}: ${v.nodes.map((n) => n.html).join(" ")}`)).toEqual([]);
  };
  await check("types", "", "[role=table]");
  await check("new type", "/-/new-type", "[data-draft-field]");
  await check("table", `/-/types/${type}?sort=name:asc`, "[data-sort-note]");
  await check("schema", `/-/types/${type}/schema`, "[data-schema-field]");
  await check("new entity", `/-/types/${type}/new`, "[data-field-input]");
  await check("detail", `/${a.entity_id}`, "[data-field]");
  await check("merge", `/${a.entity_id}/merge/${b.entity_id}`, "[data-merge-preview]");
  await check("split", `/${a.entity_id}/split`, "[data-split-preview]");
  await page.keyboard.press("Tab");
  expect(cspProblems(problems)).toEqual([]);
  await context.close();
});
