import AxeBuilder from "@axe-core/playwright";
import { appendFileSync } from "node:fs";
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { PgProxy } from "./pg-proxy";
import { serverLog, startServer, stopServer } from "./server";

// Spec 048. A real server with the real CSP, its Postgres behind a proxy the tests can cut. No LLM or provider is called: the LLM and the
// transcription API are configured, so their keys show on the settings page, but point at a closed port on this machine.

test.describe.configure({ mode: "serial" });

const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bind = process.env.MEMEX_HTTP_BIND ?? "";
const run = Date.now().toString(36);
const database = new URL(process.env.MEMEX_DATABASE_URL ?? "");
/** The database password, which these tests also use as the LLM's and the transcriber's keys: it must appear in no answer, page or log. */
const MARKER = decodeURIComponent(database.password);
const proxy = new PgProxy({ host: database.hostname, port: Number(database.port || "5432") }, Number(bind.split(":")[1]) + 200);
const evidence = (line: string) => {
  const file = process.env.MEMEX_E2E_EVIDENCE;
  console.log(line);
  if (file !== undefined && file !== "") appendFileSync(file, `${line}\n`);
};

interface World {
  vault: string;
  vaultName: string;
}
const world: World = { vault: "", vaultName: "" };

const bearer = (token: string) => ({ authorization: `Bearer ${token}` });
const send = async <T>(request: APIRequestContext, method: "POST" | "GET", path: string, data: unknown, status: number, token = adminToken): Promise<T> => {
  const res = method === "POST" ? await request.post(path, { headers: bearer(token), data }) : await request.get(path, { headers: bearer(token) });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const status = async (request: APIRequestContext, path: string, token: string) => (await request.get(path, { headers: bearer(token) })).status();

/** A principal and a token for it, made through the API as the test's own setup. */
const principalWith = async (request: APIRequestContext, name: string, grants: readonly { vault_id: string; scope: string }[], kind = "human") => {
  const p = await send<{ id: string }>(request, "POST", "/api/v1/principals", { name: `${name}-${run}`, kind }, 201);
  const t = await send<{ token: string; details: { id: string } }>(request, "POST", "/api/v1/tokens", { name, principal_id: p.id, grants }, 201);
  return { id: p.id, token: t.token, tokenId: t.details.id, name: `${name}-${run}` };
};

const signIn = async (page: Page, token: string) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).not.toHaveURL(/\/login/, { timeout: 60_000 });
};

const newSession = async (browser: Browser, token: string): Promise<{ context: BrowserContext; page: Page }> => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: `http://${bind}`, viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  await signIn(page, token);
  return { context, page };
};

/** Every request that could change something, so a test can show one click sent none. Sign-in and sign-out are left out. */
const writesOf = (page: Page) => {
  const writes: string[] = [];
  page.on("request", (r) => {
    const url = new URL(r.url());
    if (r.method() !== "GET" && url.pathname.startsWith("/api/v1/") && url.pathname !== "/api/v1/session") writes.push(`${r.method()} ${url.pathname}`);
  });
  return writes;
};

/** Anything the browser reports as wrong, such as a blocked script or style. Expected refusals are left out. */
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error" && !/\b(40[0-9]|50[0-9])\b/.test(m.text())) problems.push(m.text());
  });
  return problems;
};

const adminUrl = (tab: string, vault = world.vault) => `/vaults/${vault}/admin/${tab}`;
const confirmPanel = (page: Page) => page.getByTestId("confirm-panel");

/** The one-click check: the first press opens the confirmation and sends nothing, and with a typed name, its button stays off until the name is right. */
const firstClickSendsNothing = async (page: Page, writes: string[], open: () => Promise<void>, confirmName: string, typed: string | null) => {
  const before = writes.length;
  await open();
  await expect(confirmPanel(page)).toBeVisible();
  const confirm = confirmPanel(page).getByRole("button", { name: confirmName, exact: true });
  if (typed !== null) {
    await expect(confirm).toBeDisabled();
    await confirmPanel(page).getByRole("textbox").fill(`${typed}x`);
    await expect(confirm).toBeDisabled();
    await confirmPanel(page).getByRole("textbox").press("Enter");
  } else {
    await expect(confirm).toBeEnabled();
  }
  // Give anything that was going to be sent the time to go.
  await page.waitForTimeout(300);
  expect(writes.slice(before), "nothing is sent before the confirmation").toEqual([]);
  return confirm;
};

test.beforeAll(async () => {
  test.setTimeout(180_000);
  await proxy.start();
  const proxied = new URL(database.href);
  proxied.hostname = "127.0.0.1";
  proxied.port = String(proxy.port);
  const ms = await startServer({
    MEMEX_DATABASE_URL: proxied.href,
    MEMEX_LLM_PROVIDER: "openai",
    MEMEX_LLM_MODEL: "stand-in-model",
    MEMEX_LLM_BASE_URL: `http://user:${MARKER}@127.0.0.1:9/v1?key=${MARKER}`,
    MEMEX_LLM_API_KEY: MARKER,
    MEMEX_CHAT_MODEL: "stand-in-chat",
    MEMEX_BACKEND_TRANSCRIBER: "gemini",
    MEMEX_TRANSCRIBE_API_MODEL: "stand-in-asr",
    MEMEX_TRANSCRIBE_API_KEY: MARKER,
    MEMEX_TRANSCRIBE_API_BASE_URL: "http://127.0.0.1:9",
  });
  evidence(`server started in ${String(ms)} ms`);
});

test.afterAll(async () => {
  await stopServer();
  await proxy.stop();
});

test("item 1: a vault is created, renamed and deleted from the console", async ({ page, request }) => {
  const problems = problemsOf(page);
  const writes = writesOf(page);
  await signIn(page, adminToken);
  // A fresh server has no vault, and its home page makes the first one.
  world.vaultName = `fleet-${run}`;
  const existing = await send<{ items: unknown[] }>(request, "GET", "/api/v1/vaults", null, 200);
  if (existing.items.length === 0) {
    await expect(page.getByText("There are no vaults you can open yet.")).toBeVisible();
    await page.getByLabel("New vault's name").fill(world.vaultName);
    await page.getByRole("button", { name: "Create vault" }).click();
    await expect(page).toHaveURL(/\/vaults\/[^/]+\/documents/);
    world.vault = new URL(page.url()).pathname.split("/")[2] ?? "";
  } else {
    await page.goto(adminUrl("vaults", "any"));
    await page.getByLabel("New vault's name").fill(world.vaultName);
    await page.getByRole("button", { name: "Create vault" }).click();
    await expect(page).toHaveURL(/\/admin\/vault$/);
    world.vault = new URL(page.url()).pathname.split("/")[2] ?? "";
  }
  expect((await send<{ name: string }>(request, "GET", `/api/v1/vaults/${world.vault}`, null, 200)).name).toBe(world.vaultName);

  await page.goto(adminUrl("vault"));
  const renamed = `fleet-renamed-${run}`;
  await page.getByLabel("Vault name").fill(renamed);
  await page.getByRole("button", { name: "Rename" }).click();
  await expect(page.getByText(`Renamed to ${renamed}.`)).toBeVisible();
  expect((await send<{ name: string }>(request, "GET", `/api/v1/vaults/${world.vault}`, null, 200)).name).toBe(renamed);
  world.vaultName = renamed;

  // A second vault, made and then deleted from the list of every vault.
  await page.goto(adminUrl("vaults"));
  const doomed = `doomed-${run}`;
  await page.getByLabel("New vault's name").fill(doomed);
  await page.getByRole("button", { name: "Create vault" }).click();
  await expect(page).toHaveURL(/\/admin\/vault$/);
  const doomedId = new URL(page.url()).pathname.split("/")[2] ?? "";
  const confirm = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Delete vault…" }).click(), "Delete this vault", doomed);
  await confirmPanel(page).getByRole("textbox").fill(doomed);
  await confirm.click();
  await expect(page).not.toHaveURL(new RegExp(doomedId));
  expect(await status(request, `/api/v1/vaults/${doomedId}`, adminToken)).toBe(404);
  expect(writes).toEqual(expect.arrayContaining([`DELETE /api/v1/vaults/${doomedId}`, `PATCH /api/v1/vaults/${world.vault}`]));
  expect(problems).toEqual([]);
});

test("items 1 and 2: principals and tokens, a new token shown once, and a revoked one refused at once", async ({ browser, page, request }) => {
  const problems = problemsOf(page);
  const writes = writesOf(page);
  const bodies: { url: string; body: string }[] = [];
  page.on("response", async (r) => {
    if (new URL(r.url()).pathname.startsWith("/api/")) bodies.push({ url: r.url(), body: await r.text().catch(() => "") });
  });
  await signIn(page, adminToken);
  await page.goto(adminUrl("tokens"));
  const reader = `reader-${run}`;
  await page.getByLabel("Principal's name").fill(reader);
  await page.getByRole("button", { name: "Add principal" }).click();
  await expect(page.getByRole("cell", { name: reader, exact: true })).toBeVisible();
  const principals = await send<{ items: { id: string; name: string }[] }>(request, "GET", "/api/v1/principals?limit=200", null, 200);
  const readerId = principals.items.find((p) => p.name === reader)?.id ?? "";

  const tokenName = `reader-token-${run}`;
  await page.getByLabel("Token name").fill(tokenName);
  await page.getByLabel("Whose token").selectOption(readerId);
  await page.getByLabel("Expires in days (blank for never)").fill("30");
  const created = page.waitForResponse((r) => r.url().endsWith("/api/v1/tokens") && r.request().method() === "POST");
  await page.getByRole("button", { name: "Create token" }).click();
  const createdAnswer = await created;
  await expect(page.getByTestId("new-token")).toBeVisible();
  const value = await page.getByRole("textbox", { name: "The new token" }).inputValue();
  expect(value).toMatch(/^mvx_/);
  await expect(page.getByRole("button", { name: "Copy the new token" })).toBeVisible();
  const details = (await createdAnswer.json()) as { details: { id: string; grants: { vault_id: string; scope: string }[]; expires_at_ms: number | null } };
  expect(details.details.grants).toEqual([{ vault_id: world.vault, scope: "read" }]);
  expect(details.details.expires_at_ms).not.toBeNull();
  const seenUntil = bodies.length;

  // It works, from the API and in a browser.
  expect(await status(request, "/api/v1/whoami", value)).toBe(200);
  const other = await newSession(browser, value);
  await expect(other.page).toHaveURL(new RegExp(`/vaults/${world.vault}/`));

  // Once the panel closes, the value is gone from the page, and nothing the console reads afterwards carries it.
  await page.getByRole("button", { name: "Done, close" }).click();
  await expect(page.getByTestId("new-token")).toBeHidden();
  expect(await page.content()).not.toContain(value);
  await page.reload();
  await expect(page.locator(`[data-token="${details.details.id}"]`)).toBeVisible();
  expect(await page.content()).not.toContain(value);
  const listed = await request.get("/api/v1/tokens?all=true&limit=200", { headers: bearer(adminToken) });
  expect(await listed.text()).not.toContain(value);
  const later = bodies.slice(seenUntil);
  expect(later.length).toBeGreaterThan(0);
  expect(later.filter((b) => b.body.includes(value)).map((b) => b.url)).toEqual([]);
  expect(serverLog()).not.toContain(value);
  evidence(`token secrecy: ${String(later.length)} API responses after creation and ${String(serverLog().length)} bytes of server log hold the token 0 times`);

  // Revoking: one press opens the confirmation and sends nothing.
  const revoke = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: `Revoke ${tokenName}` }).click(), "Revoke", null);
  const deleted = page.waitForResponse((r) => r.url().includes(`/api/v1/tokens/${details.details.id}`) && r.request().method() === "DELETE");
  await revoke.click();
  expect((await deleted).status()).toBe(204);
  const after = Date.now();
  expect(await status(request, "/api/v1/whoami", value)).toBe(401);
  evidence(`revoked token refused ${String(Date.now() - after)} ms after the revoke answered`);
  // The browser signed in with it is refused on its next request, and sent to sign in.
  await other.page.reload();
  await expect(other.page).toHaveURL(/\/login/);
  await other.context.close();
  await expect(page.locator(`[data-token="${details.details.id}"]`)).toContainText("revoked");

  // Disabling a principal stops its tokens, and needs its name typed.
  const agent = await principalWith(request, "agent", [{ vault_id: world.vault, scope: "propose" }], "agent");
  expect(await status(request, "/api/v1/whoami", agent.token)).toBe(200);
  await page.reload();
  const disable = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: `Disable ${agent.name}` }).click(), "Disable", agent.name);
  await confirmPanel(page).getByRole("textbox").fill(agent.name);
  await disable.click();
  await expect(page.locator(`[data-principal="${agent.id}"]`)).toContainText("disabled");
  expect(await status(request, "/api/v1/whoami", agent.token)).toBe(401);
  expect(problems).toEqual([]);
});

test("item 3: the settings page shows secrets redacted with spec 039's markers, and no answer or log holds one", async ({ page, request }) => {
  const problems = problemsOf(page);
  await signIn(page, adminToken);
  await page.goto(adminUrl("settings"));
  const row = (path: string) => page.locator(`[data-setting="${path}"]`);
  await expect(row("llm.api_key")).toHaveAttribute("data-kind", "secret_set");
  await expect(row("llm.api_key")).toContainText("secret, set");
  await expect(row("transcribe.api.api_key")).toHaveAttribute("data-kind", "secret_set");
  await expect(row("mcp_token")).toHaveAttribute("data-kind", "secret_not_set");
  await expect(row("database_url")).toHaveAttribute("data-kind", "redacted_url");
  await expect(row("database_url")).toContainText("<redacted>@127.0.0.1");
  await expect(row("llm.base_url")).toHaveAttribute("data-kind", "redacted_url");
  await expect(page.getByRole("heading", { name: "LLM usage" })).toBeVisible();
  const text = await page.locator("main").innerText();
  expect(text).not.toContain(MARKER);
  expect(await page.content()).not.toContain(MARKER);
  // The chat setting names the model and where vault content would go, without the credentials in its address.
  await page.goto(adminUrl("vault"));
  await expect(page.getByLabel("Where vault content goes")).toContainText("stand-in-chat");
  await expect(page.getByLabel("Where vault content goes")).toContainText("http://127.0.0.1:9");
  expect(await page.content()).not.toContain(MARKER);
  const answers: string[] = [];
  for (const path of ["/api/v1/system/settings", "/api/v1/system/status", "/api/v1/system/mcp", "/api/v1/system/usage", `/api/v1/vaults/${world.vault}/chat/settings`, "/api/v1/tokens?all=true"]) {
    const res = await request.get(path, { headers: bearer(adminToken) });
    expect(res.status(), path).toBe(200);
    answers.push(await res.text());
  }
  expect(answers.filter((a) => a.includes(MARKER))).toEqual([]);
  expect(serverLog()).not.toContain(MARKER);
  evidence(`secrets: ${String(answers.length)} admin answers, the settings and vault pages, and ${String(serverLog().length)} bytes of server log hold the marker 0 times`);
  expect(problems).toEqual([]);
});

interface Impact { removed: { table: string; id: string }[]; rewritten: { table: string; id: string }[]; flagged_claims: string[]; flagged_pages: string[]; sources: string[] }
const keys = (i: Impact) => ({ removed: i.removed.map((k) => `${k.table}:${k.id}`).sort(), rewritten: i.rewritten.map((k) => `${k.table}:${k.id}`).sort(), claims: [...i.flagged_claims].sort(), pages: [...i.flagged_pages].sort(), sources: [...i.sources].sort() });

test("item 4: an erasure shows a preview that matches its result, and waits for a person's approval", async ({ browser, page, request }) => {
  test.setTimeout(480_000);
  const problems = problemsOf(page);
  const writes = writesOf(page);
  const name = `erase-me-${run}.txt`;
  const upload = await request.post(`/api/v1/vaults/${world.vault}/sources?name=${name}`, { headers: bearer(adminToken), data: `The harbour of ${run} was dredged in 1702, and the work took three summers. A second quay followed in 1711.` });
  expect(upload.status(), await upload.text()).toBe(201);
  const source = ((await upload.json()) as { source: { id: string } }).source.id;
  // Erasing reaches everything derived from it, so it's done once the document has been read.
  await expect.poll(async () => {
    const renditions = await send<{ items: { id: string }[] }>(request, "GET", `/api/v1/vaults/${world.vault}/sources/${source}/renditions`, null, 200);
    const first = renditions.items[0];
    if (first === undefined) return 0;
    return (await send<{ items: unknown[] }>(request, "GET", `/api/v1/vaults/${world.vault}/renditions/${first.id}/segments`, null, 200)).items.length;
  }, { timeout: 180_000, intervals: [500] }).toBeGreaterThan(0);
  const approver = await principalWith(request, "approver", [{ vault_id: world.vault, scope: "admin" }]);

  await signIn(page, adminToken);
  await page.goto(adminUrl("erasure"));
  await page.getByRole("link", { name }).click();
  await expect(page).toHaveURL(new RegExp(`/admin/erasure/${source}$`));
  const preview = await send<Impact>(request, "GET", `/api/v1/vaults/${world.vault}/sources/${source}/erasure`, null, 200);
  const previewPanel = page.getByRole("region", { name: "Preview" });
  await expect(previewPanel.getByTestId("removed-count")).toHaveText(String(preview.removed.length));
  await expect(previewPanel.getByTestId("rewritten-count")).toHaveText(String(preview.rewritten.length));
  await page.getByLabel("Why it's being erased, for the person who approves it").fill(`a request to erase ${run}`);
  const propose = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Erase…" }).click(), "Propose the erasure", name);
  await confirmPanel(page).getByRole("textbox").fill(name);
  await propose.click();
  const progress = page.getByTestId("erasure-progress");
  await expect(progress).toHaveAttribute("data-state", "pending");
  // Proposing removed nothing: the document is still there, and the change waits for a person.
  expect(await status(request, `/api/v1/vaults/${world.vault}/sources/${source}`, adminToken)).toBe(200);
  const reviewLink = progress.getByRole("link", { name: "Open it in Review" });
  const reviewPath = (await reviewLink.getAttribute("href")) ?? "";
  const changesetId = decodeURIComponent(reviewPath.split("/").at(-1) ?? "");
  expect((await send<{ state: string }>(request, "GET", `/api/v1/vaults/${world.vault}/changesets/${changesetId}`, null, 200)).state).toBe("pending");

  // Another admin approves it in Review, while this page waits.
  const second = await newSession(browser, approver.token);
  await second.page.goto(reviewPath);
  await expect(second.page.getByRole("button", { name: "Approve (a)" })).toBeVisible();
  await second.page.getByRole("button", { name: "Approve (a)" }).click();
  await expect.poll(async () => (await send<{ state: string; decided_by: string | null }>(request, "GET", `/api/v1/vaults/${world.vault}/changesets/${changesetId}`, null, 200)).state).toBe("committed");
  await second.context.close();
  const approvedAt = Date.now();

  await expect(progress).toHaveAttribute("data-state", "erased", { timeout: 240_000 });
  evidence(`erasure report shown ${String(Date.now() - approvedAt)} ms after approval`);
  await expect(progress).toContainText("The result matches the preview exactly.");
  const report = page.getByRole("region", { name: "Result" });
  await expect(report.getByTestId("removed-count")).toHaveText(String(preview.removed.length));
  const records = await send<{ items: { changeset_id: string; impact: Impact; actor: string }[] }>(request, "GET", `/api/v1/vaults/${world.vault}/erasures`, null, 200);
  const record = records.items.find((r) => r.changeset_id === changesetId);
  expect(record).toBeDefined();
  expect(keys(record?.impact ?? preview)).toEqual(keys(preview));
  expect(record?.actor).toBe(approver.id);
  expect(await status(request, `/api/v1/vaults/${world.vault}/sources/${source}`, adminToken)).toBe(404);
  evidence(`erasure: preview and result both remove ${String(preview.removed.length)} rows, rewrite ${String(preview.rewritten.length)}, flag ${String(preview.flagged_claims.length)} claims and ${String(preview.flagged_pages.length)} pages; identical key sets`);
  expect(problems).toEqual([]);
});

test("item 5: no destructive action runs on a single click, and operations show their progress and report", async ({ page, request }) => {
  test.setTimeout(480_000);
  const problems = problemsOf(page);
  const writes = writesOf(page);
  await signIn(page, adminToken);
  const checked: string[] = ["delete a vault", "revoke a token", "disable a principal", "propose an erasure"];

  await page.goto(adminUrl("operations"));
  const pick = (label: string) => page.getByRole("radio", { name: new RegExp(`^${label}`) });
  for (const [label, confirmName, extra] of [
    ["Roll back an index", "Start roll back an index", null],
    ["Sweep blobs", "Start sweep blobs", null],
    ["Compact", "Start compact", null],
    ["Reconcile", "Start reconcile", "Repair an index that differs"],
  ] as const) {
    await pick(label).check();
    if (extra !== null) await page.getByLabel(extra).check();
    await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Start…" }).click(), confirmName, world.vaultName);
    await confirmPanel(page).getByRole("button", { name: "Cancel" }).click();
    checked.push(extra === null ? label.toLowerCase() : "reconcile with repair");
  }
  expect((await send<{ items: unknown[] }>(request, "GET", `/api/v1/vaults/${world.vault}/operations`, null, 200)).items, "no operation was started").toEqual([]);

  // A reconcile, which changes nothing, still takes two presses, then shows its progress and report.
  await page.getByLabel("Repair an index that differs").uncheck();
  await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Start…" }).click(), "Start reconcile", null);
  await confirmPanel(page).getByRole("button", { name: "Start reconcile" }).click();
  await expect(page).toHaveURL(/\/admin\/operations\/[^/]+$/);
  const detail = page.getByTestId("operation-detail");
  await expect(detail).toHaveAttribute("data-state", "done", { timeout: 120_000 });
  await expect(page.getByRole("table", { name: "What it found" })).toBeVisible();
  const reconcileId = new URL(page.url()).pathname.split("/").at(-1) ?? "";
  const job = await send<{ progress: { step: number; of: number } | null; report: Record<string, unknown> | null }>(request, "GET", `/api/v1/vaults/${world.vault}/jobs/${reconcileId}`, null, 200);
  expect(job.report).not.toBeNull();
  expect(job.progress?.step).toBe(job.progress?.of);

  // A compact, with the vault's name typed, runs to its end.
  await page.goto(adminUrl("operations"));
  await pick("Compact").check();
  const start = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Start…" }).click(), "Start compact", world.vaultName);
  await confirmPanel(page).getByRole("textbox").fill(world.vaultName);
  await start.click();
  await expect(page.getByTestId("operation-detail")).toHaveAttribute("data-state", "done", { timeout: 180_000 });

  // An export gives a download.
  await page.goto(adminUrl("operations"));
  await pick("Export").check();
  await page.getByRole("button", { name: "Start…" }).click();
  await confirmPanel(page).getByRole("button", { name: "Start export" }).click();
  await expect(page.getByTestId("operation-detail")).toHaveAttribute("data-state", "done", { timeout: 120_000 });
  const download = page.waitForEvent("download");
  await page.getByRole("link", { name: "Download the archive" }).click();
  const archive = await (await download).path();
  expect(archive).not.toBeNull();

  // An import that would replace a vault needs words typed. Nothing goes until then.
  await page.goto(adminUrl("operations"));
  await page.getByLabel("Archive from an export").setInputFiles({ name: "archive.bin", mimeType: "application/octet-stream", buffer: Buffer.from("not an archive") });
  await page.getByRole("radio", { name: "Under the archive's own id" }).check();
  await page.getByLabel(/Replace that vault if it exists/).check();
  await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Import…" }).click(), "Import", "replace the vault");
  await confirmPanel(page).getByRole("button", { name: "Cancel" }).click();
  checked.push("import replacing a vault");

  // Clearing the caches, on the status page.
  await page.goto(adminUrl("status"));
  const clear = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Clear caches…" }).click(), "Clear caches", null);
  await clear.click();
  await expect(page.getByText(/Cleared the caches of \d+ vaults\./)).toBeVisible();
  checked.push("clear caches");

  // Turning chat on sends vault content to the model, and off stops everyone's chat: both ask first.
  await page.goto(adminUrl("vault"));
  const on = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Turn chat on…" }).click(), "Turn chat on", null);
  await expect(confirmPanel(page)).toContainText("stand-in-chat");
  await on.click();
  await expect(page.getByLabel("Where vault content goes")).toContainText("On");
  const off = await firstClickSendsNothing(page, writes, () => page.getByRole("button", { name: "Turn chat off" }).click(), "Turn chat off", null);
  await off.click();
  await expect(page.getByLabel("Where vault content goes")).toContainText("Off");
  checked.push("turn chat on", "turn chat off");
  evidence(`one-click check passed for: ${checked.join(", ")}`);
  expect(problems).toEqual([]);
});

const ADMIN_TABS = ["vault", "tokens", "operations", "erasure", "status", "settings", "mcp", "vaults"];

test("item 6: every admin screen tells a non-admin why, and the API refuses them too", async ({ browser, request }) => {
  const reader = await principalWith(request, "only-reads", [{ vault_id: world.vault, scope: "read" }]);
  const proposer = await principalWith(request, "only-proposes", [{ vault_id: world.vault, scope: "propose" }]);
  const vaultAdmin = await principalWith(request, "vault-admin", [{ vault_id: world.vault, scope: "admin" }]);
  for (const who of [reader, proposer]) {
    const { context, page } = await newSession(browser, who.token);
    await expect(page.getByRole("navigation", { name: "Screens" }).getByRole("link", { name: "Admin" })).toHaveCount(0);
    for (const tab of ADMIN_TABS) {
      await page.goto(adminUrl(tab));
      await expect(page.getByRole("alert").filter({ hasText: "You don't have permission to use Admin in this vault." })).toBeVisible();
      await expect(page.getByRole("navigation", { name: "Administration" })).toHaveCount(0);
    }
    await context.close();
  }
  const { context, page } = await newSession(browser, vaultAdmin.token);
  for (const tab of ADMIN_TABS) {
    await page.goto(adminUrl(tab));
    const label = { vault: "Vault", tokens: "Tokens", operations: "Operations", erasure: "Erasure", status: "Status", settings: "Settings", mcp: "MCP", vaults: "All vaults" }[tab] ?? tab;
    const refused = page.getByRole("alert").filter({ hasText: `You don't have permission to use ${label} here.` });
    if (["status", "settings", "mcp", "vaults"].includes(tab)) await expect(refused).toBeVisible();
    else await expect(page.getByRole("navigation", { name: "Administration" }).getByRole("link", { name: label })).toHaveAttribute("aria-current", "page");
  }
  await context.close();

  const v = `/api/v1/vaults/${world.vault}`;
  const server = ["/api/v1/system/settings", "/api/v1/system/status", "/api/v1/system/usage", "/api/v1/system/mcp", "/api/v1/caches", "/api/v1/principals", "/api/v1/tokens?all=true"];
  const vaultRoutes = [`${v}/operations`, `${v}/chat/settings`];
  const refusals: string[] = [];
  let writes = 0;
  for (const who of [reader, proposer, vaultAdmin]) {
    for (const path of server) {
      const code = await status(request, path, who.token);
      expect(code, `${who.name} ${path}`).toBe(403);
      refusals.push(`${String(code)} ${path}`);
    }
    for (const [method, path, body] of [["POST", "/api/v1/vaults", { name: "x" }], ["POST", "/api/v1/principals", { name: "x", kind: "human" }], ["POST", "/api/v1/caches/clear", {}]] as const) {
      const res = await request.fetch(path, { method, headers: bearer(who.token), data: body });
      expect(res.status(), `${who.name} ${method} ${path}`).toBe(403);
      writes += 1;
    }
  }
  for (const who of [reader, proposer]) {
    expect(await status(request, vaultRoutes[0] ?? "", who.token)).toBe(403);
    refusals.push(`403 ${vaultRoutes[0] ?? ""}`);
    for (const [method, path, body] of [["PATCH", v, { name: "x" }], ["DELETE", v, null], ["POST", `${v}/operations`, { op: "sweep" }], ["PUT", `${v}/chat/settings`, { enabled: true }]] as const) {
      const res = await request.fetch(path, { method, headers: bearer(who.token), ...(body === null ? {} : { data: body }) });
      expect(res.status(), `${who.name} ${method} ${path}`).toBe(403);
      writes += 1;
    }
  }
  expect(await status(request, vaultRoutes[0] ?? "", vaultAdmin.token)).toBe(200);
  evidence(`permissions: all 8 admin screens show the refusal to a read and a propose token, and the 4 server screens to a vault admin; the API refused ${String(refusals.length)} reads and ${String(writes)} writes across the three`);
});

test("item 7: with Postgres down, the status page explains the problem instead of failing", async ({ page }) => {
  const problems = problemsOf(page);
  await signIn(page, adminToken);
  await page.goto(adminUrl("status"));
  const pg = page.locator('[data-health="postgres"]');
  await expect(pg).toHaveAttribute("data-tone", "success");
  await expect(page.locator('[data-health="llm"]')).toHaveAttribute("data-tone", "danger");
  proxy.cut();
  const cutAt = Date.now();
  await expect(pg).toHaveAttribute("data-tone", "danger", { timeout: 30_000 });
  const shownAfter = Date.now() - cutAt;
  await expect(pg).toContainText("Postgres is down");
  await expect(pg).toContainText("Nothing can be read or written until it's back");
  await expect(page.locator('[data-health="memory"]')).toBeVisible();
  await expect(page.getByRole("region", { name: "Vaults" })).toContainText("Not available");
  // Still updating, not stuck on its last answer.
  const updated = await page.getByTestId("status-updated").innerText();
  await expect(page.getByTestId("status-updated")).not.toHaveText(updated, { timeout: 10_000 });
  proxy.restore();
  const restoredAt = Date.now();
  await expect(pg).toHaveAttribute("data-tone", "success", { timeout: 60_000 });
  evidence(`postgres down: shown ${String(shownAfter)} ms after the cut, and back ${String(Date.now() - restoredAt)} ms after it returned`);
  expect(problems).toEqual([]);
});

test("every admin screen passes axe", async ({ page }) => {
  await signIn(page, adminToken);
  const failures: string[] = [];
  for (const tab of ADMIN_TABS) {
    await page.goto(adminUrl(tab));
    await expect(page.getByRole("navigation", { name: "Administration" })).toBeVisible();
    await page.waitForLoadState("networkidle").catch(() => undefined);
    const { violations } = await new AxeBuilder({ page }).analyze();
    failures.push(...violations.map((v) => `${tab}: ${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`));
  }
  expect(failures).toEqual([]);
});
