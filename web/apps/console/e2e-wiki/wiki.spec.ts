import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { prepareWikiText } from "@memex/core";
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Locator, type Page } from "@playwright/test";
import { startServer, stopServer } from "../e2e-shell/server";

// Spec 044. Every test runs on a real server with the real CSP. Timings on a busy machine aren't asserted, beyond generous waits.
const here = dirname(fileURLToPath(import.meta.url));
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);

interface World {
  readonly vault: string;
  readonly readToken: string;
  readonly agentToken: string;
  readonly agentId: string;
  readonly otherToken: string;
  readonly current: string;
  readonly old: string;
  readonly target: { readonly id: string; readonly slug: string; readonly oldSlug: string };
}
let world: World;

const baseURL = () => test.info().project.use.baseURL ?? "";
const api = (path: string) => `/api/v1/vaults/${world.vault}${path}`;
const send = async <T>(request: APIRequestContext, path: string, data: unknown, status = 200, token = adminToken): Promise<T> => {
  const res = await request.post(path, { headers: { authorization: `Bearer ${token}` }, data });
  expect(res.status(), `${path}: ${await res.text()}`).toBe(status);
  return (await res.json()) as T;
};
const get = async <T>(request: APIRequestContext, path: string): Promise<T> => {
  const res = await request.get(path, { headers: bearer });
  expect(res.status(), path).toBe(200);
  return (await res.json()) as T;
};

const addClaim = (id: string, segment: string, value: string, quote: string) => ({
  op: "add_assertion", id, subject: "pump room", predicate: "opened in", value, epistemic: "asserted", machine: false, confidence_permille: 900, valid_from_ms: null, valid_to_ms: null,
  evidence: [[`${id}-e`, { segment_id: segment, quote, stance: "supports" }]],
});

test.beforeAll(async ({ request }) => {
  test.setTimeout(240_000);
  console.log(`server started in ${String(await startServer())} ms`);
  const vault = await send<{ id: string }>(request, "/api/v1/vaults", { name: `wiki-${run}` }, 201);
  const person = await send<{ id: string }>(request, "/api/v1/principals", { name: `reader-${run}`, kind: "human" }, 201);
  const read = await send<{ token: string }>(request, "/api/v1/tokens", { name: "read", principal_id: person.id, grants: [{ vault_id: vault.id, scope: "read" }] }, 201);
  const other = await send<{ id: string }>(request, "/api/v1/principals", { name: `other-${run}`, kind: "human" }, 201);
  const otherToken = await send<{ token: string }>(request, "/api/v1/tokens", { name: "other", principal_id: other.id, grants: [{ vault_id: vault.id, scope: "propose" }] }, 201);
  const agent = await send<{ id: string }>(request, "/api/v1/principals", { name: `agent-${run}`, kind: "agent" }, 201);
  const agentToken = await send<{ token: string }>(request, "/api/v1/tokens", { name: "agent", principal_id: agent.id, grants: [{ vault_id: vault.id, scope: "propose" }] }, 201);

  const upload = await request.post(`/api/v1/vaults/${vault.id}/sources?name=pump-history.txt`, { headers: bearer, data: `The pump room opened in 1912 and again in 1913. Marker ${run}.` });
  expect(upload.status()).toBe(201);
  const { source } = (await upload.json()) as { source: { id: string } };
  let segment: string | undefined;
  await expect.poll(async () => {
    const renditions = await get<{ items: { id: string }[] }>(request, `/api/v1/vaults/${vault.id}/sources/${source.id}/renditions`);
    const rendition = renditions.items[0];
    if (rendition === undefined) return false;
    const segments = await get<{ items: { id: string }[] }>(request, `/api/v1/vaults/${vault.id}/renditions/${rendition.id}/segments`);
    segment = segments.items[0]?.id;
    return segment !== undefined;
  }, { timeout: 120_000, intervals: [500] }).toBe(true);
  const seg = segment ?? "";
  const current = `claim-current-${run}`;
  const old = `claim-old-${run}`;
  const replacement = `claim-new-${run}`;
  const propose = (ops: unknown[], key: string) => send(request, `/api/v1/vaults/${vault.id}/proposals`, { ops, rationale: "fixture", idempotency_key: `${key}-${run}` });
  await propose([addClaim(current, seg, "1912", "opened in 1912")], "c1");
  await propose([addClaim(old, seg, "1912 (old)", "opened in 1912")], "c2");
  await propose([{ op: "supersede_assertion", target: old, base_version: 1, replacement: addClaim(replacement, seg, "1913", "again in 1913") }], "c3");

  const made = await send<{ page_id: string }>(request, `/api/v1/vaults/${vault.id}/pages`, { title: `Target ${run}`, body: "The target page.\n", rationale: "fixture", idempotency_key: `t1-${run}` });
  const oldSlug = `target-${run}`;
  const slug = `target-moved-${run}`;
  await send(request, `/api/v1/vaults/${vault.id}/pages/${made.page_id}/rename`, { base_version: 1, title: `Target moved ${run}`, slug, rationale: "fixture", idempotency_key: `t2-${run}` });
  world = { vault: vault.id, readToken: read.token, agentToken: agentToken.token, agentId: agent.id, otherToken: otherToken.token, current, old, target: { id: made.page_id, slug, oldSlug } };
});

test.afterAll(async () => {
  await stopServer();
});

const signIn = async (page: Page, token = adminToken) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  // A generous wait: the first reads after a server starts are slow on a busy machine.
  await expect(page).toHaveURL(/\/vaults\//, { timeout: 60_000 });
};

const newSession = async (browser: Browser, token = adminToken): Promise<{ context: BrowserContext; page: Page }> => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: baseURL() });
  const page = await context.newPage();
  await signIn(page, token);
  return { context, page };
};

const pagesUrl = () => `/vaults/${world.vault}/pages`;

/** Anything the browser reports as wrong. A blocked style or script is a console error, and this includes them. */
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    const path = m.location().url === "" ? "" : new URL(m.location().url).pathname;
    // A browser logs these answers, and each is what the screen is built to handle: nobody signed in, a link with no page yet, a stale save.
    const expected = (m.text().includes("401") && path === "/api/v1/whoami") || (m.text().includes("404") && path.includes("/pages/by-slug/")) || (m.text().includes("409") && path.endsWith("/edit"));
    if (m.type() === "error" && !expected) problems.push(m.text());
  });
  return problems;
};

const editor = (page: Page) => page.locator(".cm-content");
const preview = (page: Page) => page.getByTestId("preview");
/** The saved page's text. The preview has the same component, so this has to say which. */
const reader = (page: Page) => page.locator("article").getByTestId("page-markdown");

/** Replaces the editor's text as a paste would. */
const setText = async (page: Page, text: string) => {
  await editor(page).focus();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  await page.evaluate((value) => {
    const content = document.querySelector("[data-testid=markdown-editor]")?.shadowRoot?.querySelector(".cm-content");
    const data = new DataTransfer();
    data.setData("text/plain", value);
    content?.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, text);
  await expect.poll(() => editorText(page)).toBe(text);
};
const editorText = (page: Page) => page.evaluate(() => {
  const root = document.querySelector("[data-testid=markdown-editor]")?.shadowRoot;
  const lines = [...(root?.querySelectorAll(".cm-line") ?? [])];
  return lines.map((l) => (l.textContent ?? "")).join("\n");
});

/** CodeMirror ignores Enter for 75 ms after its list changes, so a keypress meant for the text isn't taken for a choice. */
const acceptCompletion = async (page: Page) => {
  await page.waitForTimeout(200);
  await page.keyboard.press("Enter");
};

const createInEditor = async (page: Page, title: string, body: string) => {
  await page.goto(`${pagesUrl()}/-/new`);
  await page.getByLabel("Title").fill(title);
  await expect(editor(page)).toBeVisible();
  await setText(page, body);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("heading", { name: title, level: 2 })).toBeVisible();
  const url = new URL(page.url());
  return url.pathname.split("/").at(-1) ?? "";
};

const fixture = (name: string) =>
  readFileSync(join(here, "fixtures", name), "utf8")
    .replaceAll("@TARGET@", world.target.slug)
    .replaceAll("@OLD_SLUG@", world.target.oldSlug)
    .replaceAll("@CURRENT@", world.current)
    .replaceAll("@OLD@", world.old);

const pageBody = async (request: APIRequestContext, id: string) => (await get<{ body: string; current_revision: number }>(request, api(`/pages/${id}`)));

test("codemirror: the editor is styled and works under the real CSP, with no error and no style element", async ({ page }) => {
  const problems = problemsOf(page);
  const csp: string[] = [];
  page.on("response", (r) => { if (r.request().resourceType() === "document") csp.push(r.headers()["content-security-policy"] ?? ""); });
  await signIn(page);
  await page.goto(`${pagesUrl()}/-/new`);
  await expect(editor(page)).toBeVisible();
  expect(new Set(csp)).toEqual(new Set(["default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'"]));
  const look = () => page.evaluate(() => {
    const root = document.querySelector("[data-testid=markdown-editor]")?.shadowRoot;
    const of = (selector: string) => { const el = root?.querySelector(selector); return el ? getComputedStyle(el) : null; };
    return {
      adoptedRules: [...(root?.adoptedStyleSheets ?? [])].reduce((n, s) => n + s.cssRules.length, 0),
      styleElements: document.querySelectorAll("style").length + (root?.querySelectorAll("style").length ?? 0),
      editorDisplay: of(".cm-editor")?.display,
      scrollerDisplay: of(".cm-scroller")?.display,
      contentWhiteSpace: of(".cm-content")?.whiteSpace,
      font: of(".cm-scroller")?.fontFamily,
      background: of(".cm-editor")?.backgroundColor,
      tooltipBackground: of(".cm-tooltip-autocomplete")?.backgroundColor,
    };
  });
  const atStart = await look();
  expect(atStart.adoptedRules).toBeGreaterThan(50);
  expect(atStart).toMatchObject({ styleElements: 0, editorDisplay: "flex", scrollerDisplay: "flex", contentWhiteSpace: "break-spaces" });
  expect(atStart.font).toContain("JetBrains Mono");
  await editor(page).focus();
  await page.keyboard.type("[[targ");
  await expect(page.locator(".cm-tooltip-autocomplete")).toBeVisible();
  const withTooltip = await look();
  console.log(`codemirror under the real CSP: ${JSON.stringify(withTooltip)}`);
  // Not fully transparent, which is what a blocked style would leave.
  expect(withTooltip.tooltipBackground).not.toMatch(/, 0\)$/);
  expect(withTooltip.styleElements).toBe(0);
  expect(problems).toEqual([]);
});

test("verification 1: create, edit, link with autocomplete, rename keeping links, and revert", async ({ page, request }) => {
  const problems = problemsOf(page);
  await signIn(page);

  // Create. A link to the target comes from autocomplete, by keyboard.
  await page.goto(`${pagesUrl()}/-/new`);
  const title = `Journal ${run}`;
  await page.getByLabel("Title").fill(title);
  await editor(page).focus();
  await page.keyboard.type("First entry, about [[targ");
  const option = page.locator(".cm-tooltip-autocomplete li", { hasText: `Target moved ${run}` });
  await expect(option).toBeVisible();
  await acceptCompletion(page);
  await expect.poll(() => editorText(page)).toBe(`First entry, about [[${world.target.slug}]]`);
  await page.keyboard.type(" and on.");
  await expect(preview(page).getByRole("link", { name: world.target.slug })).toHaveAttribute("data-link-state", "resolved");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("heading", { name: title, level: 2 })).toBeVisible();
  const id = new URL(page.url()).pathname.split("/").at(-1) ?? "";
  await expect(reader(page)).toContainText("First entry, about");
  expect((await pageBody(request, id)).current_revision).toBe(1);

  // The link leads there, and the target shows the backlink.
  await reader(page).getByRole("link", { name: world.target.slug }).click();
  await expect(page.getByRole("heading", { name: `Target moved ${run}`, level: 2 })).toBeVisible();
  await expect(page.getByRole("complementary", { name: "Links" }).getByRole("region", { name: "Backlinks" }).getByRole("link", { name: title })).toBeVisible();

  // Edit: add a broken link and a second link through the old slug.
  await page.goto(`${pagesUrl()}/${id}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(editor(page)).toBeVisible();
  await setText(page, `Second entry, about [[${world.target.oldSlug}|the old name]] and [[not written yet ${run}]].\n`);
  await page.getByLabel("Why").fill("A better sentence");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(reader(page)).toContainText("Second entry");
  await expect(reader(page).getByRole("link", { name: "the old name" })).toHaveAttribute("data-link-state", "resolved");
  const broken = reader(page).getByRole("link", { name: /not written yet/ });
  await expect(broken).toHaveAttribute("data-link-state", "broken");
  const links = page.getByRole("complementary", { name: "Links" });
  await expect(links.getByRole("region", { name: "Links from this page" })).toContainText(world.target.oldSlug);
  await expect(links.getByRole("region", { name: "Unresolved links" })).toContainText(`not-written-yet-${run}`);

  // A broken link offers to create the page, and the screen of broken links lists it.
  await page.goto(`${pagesUrl()}/-/broken-links`);
  await expect(page.getByRole("region", { name: "Broken links" })).toContainText(`not-written-yet-${run}`);
  await page.getByRole("region", { name: "Broken links" }).getByRole("link", { name: `not-written-yet-${run}` }).click();
  await expect(page.getByText(`No page has the slug not-written-yet-${run} yet.`)).toBeVisible();
  await page.getByRole("link", { name: "Create it" }).click();
  await expect(page.getByLabel("Title")).toHaveValue(`not-written-yet-${run}`);
  await page.getByRole("link", { name: "Close" }).click();

  // Rename: the old slug keeps working, in this page's link to it, by URL, and in the list.
  const linker = await createInEditor(page, `Linker ${run}`, `See [[journal-${run}]].\n`);
  await page.goto(`${pagesUrl()}/${id}`);
  await page.getByRole("button", { name: "Rename" }).click();
  const renamed = `Diary ${run}`;
  await page.getByLabel("New title").fill(renamed);
  await page.getByRole("region", { name: "Rename this page" }).getByRole("button", { name: "Rename" }).click();
  await expect(page.getByRole("heading", { name: renamed, level: 2 })).toBeVisible();
  await expect(page.getByText(`Also reached as journal-${run}`)).toBeVisible();
  await page.goto(`${pagesUrl()}/${linker}`);
  const kept = reader(page).getByRole("link", { name: `journal-${run}` });
  await expect(kept).toHaveAttribute("data-link-state", "resolved");
  await kept.click();
  await expect(page.getByRole("heading", { name: renamed, level: 2 })).toBeVisible();
  await page.goto(`${pagesUrl()}/-/by-slug/journal-${run}`);
  await expect(page.getByRole("heading", { name: renamed, level: 2 })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/pages/${id}$`));
  await page.goto(pagesUrl());
  await expect(page.getByRole("link", { name: renamed })).toBeVisible();

  // History, diff and revert.
  await page.goto(`${pagesUrl()}/${id}/history`);
  await expect(page.getByRole("region", { name: "History" }).getByRole("listitem")).toHaveCount(3);
  await expect(page.getByRole("region", { name: "History" }).getByText("Person").first()).toBeVisible();
  await page.getByRole("listitem").filter({ hasText: "Revision 2" }).getByRole("link", { name: "Changes", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Revision 1 to revision 2" })).toBeVisible();
  await expect(page.getByText("First entry, about")).toBeVisible();
  await expect(page.getByText("Second entry, about")).toBeVisible();
  await page.goto(`${pagesUrl()}/${id}?revision=1`);
  await expect(page.getByText("You're reading revision 1. The page is now at revision 3.")).toBeVisible();
  await expect(reader(page)).toContainText("First entry");
  await page.goto(`${pagesUrl()}/${id}/history`);
  await page.getByRole("button", { name: "Revert to revision 1" }).click();
  await page.getByRole("region", { name: "Revert to revision 1" }).getByRole("button", { name: "Revert", exact: true }).click();
  await expect(reader(page)).toContainText("First entry, about");
  await expect(page.getByText("Restored from revision 1")).toBeVisible();
  const after = await pageBody(request, id);
  expect(after.current_revision).toBe(4);
  expect(after.body).toBe(`First entry, about [[${world.target.slug}]] and on.\n`);
  // A rename survives a revert of the body's history: the link to the old slug still leads here.
  await page.goto(`${pagesUrl()}/${linker}`);
  await expect(reader(page).getByRole("link", { name: `journal-${run}` })).toHaveAttribute("data-link-state", "resolved");

  // No modal in any of it, so no CSP error.
  expect(problems).toEqual([]);
});

test("verification 2: two sessions: a stale save shows the conflict view and never overwrites", async ({ browser, request }) => {
  const a = await newSession(browser);
  const b = await newSession(browser, world.otherToken);
  const problems = [...[a.page, b.page].map(problemsOf)];
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Shared ${run}`, body: "Intro.\n\nMiddle.\n\nEnd.\n", rationale: "fixture", idempotency_key: `shared-${run}` });
  const id = made.page_id;
  const edit = `${pagesUrl()}/${id}/edit`;

  // Both open the same version.
  await a.page.goto(edit);
  await b.page.goto(edit);
  await expect(editor(a.page)).toBeVisible();
  await expect(editor(b.page)).toBeVisible();

  // A saves first, changing the middle and the end. B changes the middle another way and the intro.
  await setText(a.page, "Intro.\n\nMiddle from A.\n\nEnd from A.\n");
  await a.page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(reader(a.page)).toContainText("Middle from A.");
  const afterA = await pageBody(request, id);
  expect(afterA.current_revision).toBe(2);

  await setText(b.page, "Intro from B.\n\nMiddle from B.\n\nEnd.\n");
  await b.page.getByRole("button", { name: "Save", exact: true }).click();
  const conflict = b.page.getByTestId("conflict-view");
  await expect(conflict).toBeVisible();
  await expect(conflict).toContainText("This page changed while you were editing, and your version hasn't been saved.");
  await expect(conflict.getByRole("region", { name: "Your version" })).toContainText("Middle from B.");
  await expect(conflict.getByRole("region", { name: "Current version, revision 2" })).toContainText("Middle from A.");
  // Nothing was written.
  const still = await pageBody(request, id);
  expect(still.current_revision).toBe(2);
  expect(still.body).toBe(afterA.body);

  // Nothing can be saved until the one real conflict has a choice. No modal, so no CSP error.
  const saveMerged = conflict.getByRole("button", { name: "Save merged version" });
  await expect(saveMerged).toBeDisabled();
  const part = conflict.getByRole("group", { name: "Both changed this part" });
  await expect(part).toHaveCount(1);
  await part.getByLabel("Keep both").check();
  // Neighbouring changes combine by themselves, and only the block both changed needs a choice.
  await expect(part).toContainText("Middle from B.");
  await expect(part).not.toContainText("Intro from B.");
  await expect(conflict.getByRole("region", { name: "Merged result" })).toContainText("Intro from B.");
  await expect(conflict.getByRole("region", { name: "Merged result" })).toContainText("End from A.");

  // A third save lands before this one, so the merge itself goes stale, and still overwrites nothing.
  await send(request, api(`/pages/${id}/edit`), { base_version: 2, body: "Intro.\n\nMiddle from A.\n\nEnd from A.\n\nAdded by a third.\n", rationale: "third", idempotency_key: `third-${run}` });
  await saveMerged.click();
  await expect(conflict.getByRole("region", { name: "Current version, revision 3" })).toContainText("Added by a third.");
  const third = await pageBody(request, id);
  expect(third.current_revision).toBe(3);
  expect(third.body).toContain("Added by a third.");
  expect(third.body).not.toContain("from B");

  // Merged again, this time it lands, keeping everyone's text.
  const again = conflict.getByRole("group", { name: "Both changed this part" });
  if ((await again.count()) > 0) await again.first().getByLabel("Keep mine").check();
  await conflict.getByRole("button", { name: "Save merged version" }).click();
  await expect(reader(b.page)).toContainText("Added by a third.");
  const merged = await pageBody(request, id);
  expect(merged.current_revision).toBe(4);
  expect(merged.body).toContain("Intro from B.");
  expect(merged.body).toBe("Intro from B.\n\nMiddle from B.\n\nMiddle from A.\n\nEnd from A.\n\nAdded by a third.\n");
  console.log(`merged body: ${JSON.stringify(merged.body)}`);

  // The other way out: keep the current version and drop mine. The draft goes too.
  await a.page.goto(edit);
  await expect(editor(a.page)).toBeVisible();
  await send(request, api(`/pages/${id}/edit`), { base_version: 4, body: "Moved on.\n", rationale: "other", idempotency_key: `other-${run}` });
  await setText(a.page, "Mine, to be dropped.\n");
  await a.page.getByRole("button", { name: "Save", exact: true }).click();
  await a.page.getByTestId("conflict-view").getByRole("button", { name: "Use theirs and discard mine" }).click();
  await expect(reader(a.page)).toContainText("Moved on.");
  expect((await pageBody(request, id)).body).toBe("Moved on.\n");
  expect(await a.page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith("memex.draft.")).length)).toBe(0);

  expect(problems.flat()).toEqual([]);
  await a.context.close();
  await b.context.close();
});

const EVIDENCE = process.env.MEMEX_E2E_EVIDENCE;

const hostileRun = async (browser: Browser, id: string, bypassCSP: boolean, body: string) => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: baseURL(), bypassCSP });
  const page = await context.newPage();
  const origin = new URL(baseURL()).origin;
  const requests: { url: string; type: string }[] = [];
  const dialogs: string[] = [];
  page.on("request", (r) => {
    if (!r.url().startsWith("blob:")) requests.push({ url: r.url(), type: r.resourceType() });
  });
  page.on("dialog", (d) => { dialogs.push(d.message()); void d.dismiss(); });
  // Should anything try to reach out, it's answered, so a script fetched this way would run and be caught.
  await context.route("https://evil.example/**", (route) => route.fulfill({ status: 200, contentType: "application/javascript", body: "window.__pwned = 'remote script';" }));
  const violations: string[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => { if (/Content Security Policy|violates/i.test(m.text())) violations.push(m.text()); });
  await signIn(page);
  requests.length = 0;

  const check = async (where: string, root: Locator) => {
    await expect(root).toContainText("Hostile page");
    await page.waitForLoadState("networkidle");
    const found = await page.evaluate((selector) => {
      const el = document.querySelector(selector);
      const bad = el === null ? [] : [...el.querySelectorAll("script, img, iframe, object, embed, video, audio, svg:not([aria-hidden]) script, link, style, meta, base, form, [srcdoc], [style*='url']")].map((e) => e.outerHTML.slice(0, 120));
      const attrs = el === null ? [] : [...el.querySelectorAll("*")].flatMap((e) => e.getAttributeNames().filter((n) => n.startsWith("on")).map((n) => `${e.tagName} ${n}`));
      const hrefs = el === null ? [] : [...el.querySelectorAll("a")].map((a) => a.getAttribute("href") ?? "");
      return { pwned: (window as unknown as { __pwned?: unknown }).__pwned ?? null, bad, attrs, hrefs, title: document.title };
    }, where === "reader" ? "article" : "[data-testid=preview]");
    expect(found.pwned, where).toBeNull();
    expect(found.bad, where).toEqual([]);
    expect(found.attrs, where).toEqual([]);
    const refused = found.hrefs.filter((h) => !/^(https?:|mailto:|\/)/.test(h));
    expect(refused, where).toEqual([]);
    // Only the one safe link goes off-site, and it's marked.
    const offsite = found.hrefs.filter((h) => /^https?:|^\/\//.test(h) && !h.startsWith(origin));
    expect(offsite.filter((h) => h.includes("evil.example") && !h.endsWith("/linked")), where).toEqual([]);
    return found;
  };

  await page.goto(`${pagesUrl()}/${id}`);
  const inReader = await check("reader", reader(page));
  // Raw HTML is on the page as words, so nothing is hidden from the reader.
  await expect(reader(page)).toContainText("<script>window.__pwned");
  await expect(reader(page)).toContainText("Image not loaded: remote image");
  // Moving over and clicking what was raw HTML runs nothing.
  await reader(page).getByText("event handlers").click();
  await reader(page).getByText("plain javascript", { exact: true }).click();
  expect(page.url()).toContain(`/pages/${id}`);
  const external = reader(page).getByRole("link", { name: /a real external link/ });
  await expect(external).toHaveAttribute("rel", "noopener noreferrer");
  await expect(external).toHaveAttribute("target", "_blank");
  await expect(external).toContainText("(external link)");

  await page.goto(`${pagesUrl()}/${id}/edit`);
  await expect(editor(page)).toBeVisible();
  const inPreview = await check("preview", preview(page));
  // The same text arriving by typing, rather than from the server.
  await setText(page, `${body}\nTyped here.\n`);
  await check("preview", preview(page));
  // And coming back from a draft in storage.
  await page.reload();
  await expect(editor(page)).toBeVisible();
  await check("preview", preview(page));
  await page.getByRole("button", { name: "Discard draft" }).click();
  await page.waitForLoadState("networkidle");

  const pwned = await page.evaluate(() => (window as unknown as { __pwned?: unknown }).__pwned ?? null);
  const outside = requests.filter((r) => !r.url.startsWith(origin));
  const log = { bypassCSP, requestsAfterSignIn: requests.length, outside, pwned, dialogs, violations, errors, hrefsInReader: inReader.hrefs, hrefsInPreview: inPreview.hrefs, sameOriginPaths: [...new Set(requests.map((r) => `${r.type} ${new URL(r.url).pathname.replace(world.vault, "<vault>").replace(id, "<page>").replace(/-[\w-]{8}\.(js|css|woff2)$/, "-<hash>.$1")}`))].sort() };
  await context.close();
  return log;
};

test("verification 3: a hostile page runs nothing and loads nothing from outside, with and without the CSP's help", async ({ browser, request }) => {
  test.setTimeout(480_000);
  const body = fixture("hostile.md");
  const title = `<img src=x onerror="window.__pwned='title'"> Hostile ${run}`;
  const made = await send<{ page_id: string }>(request, api("/pages"), { title, slug: `hostile-${run}`, body, rationale: "fixture", idempotency_key: `hostile-${run}` });
  const withCsp = await hostileRun(browser, made.page_id, false, body);
  const withoutCsp = await hostileRun(browser, made.page_id, true, body);
  console.log(`hostile page: ${JSON.stringify({ withCsp: { requests: withCsp.requestsAfterSignIn, outside: withCsp.outside.length, violations: withCsp.violations.length }, withoutCsp: { requests: withoutCsp.requestsAfterSignIn, outside: withoutCsp.outside.length, violations: withoutCsp.violations.length } })}`);
  for (const log of [withCsp, withoutCsp]) {
    expect(log.outside).toEqual([]);
    expect(log.pwned).toBeNull();
    expect(log.dialogs).toEqual([]);
    expect(log.violations).toEqual([]);
    expect(log.errors).toEqual([]);
    expect(log.requestsAfterSignIn).toBeGreaterThan(0);
  }

  // The checks can fail. In this same page, a script and an image put in by hand are seen: by the CSP with it on, and as a real run and a real request with it off.
  const control = async (bypassCSP: boolean) => {
    const context = await browser.newContext({ colorScheme: "dark", baseURL: baseURL(), bypassCSP });
    const page = await context.newPage();
    const outside: string[] = [];
    const violations: string[] = [];
    page.on("request", (r) => { if (r.url().includes("evil.example")) outside.push(r.url()); });
    page.on("console", (m) => { if (/Content Security Policy|violates/i.test(m.text())) violations.push(m.text().slice(0, 100)); });
    await context.route("https://evil.example/**", (route) => route.fulfill({ status: 200, contentType: "image/png", body: "" }));
    await signIn(page);
    await page.goto(`${pagesUrl()}/${made.page_id}`);
    await expect(reader(page)).toContainText("Hostile page");
    await page.evaluate(() => {
      const script = document.createElement("script");
      script.textContent = "window.__pwned = 'control';";
      document.body.append(script);
      const image = document.createElement("img");
      image.src = "https://evil.example/control.png";
      document.body.append(image);
    });
    await page.waitForLoadState("networkidle");
    await expect.poll(() => outside.length + violations.length).toBeGreaterThan(0);
    const pwned = await page.evaluate(() => (window as unknown as { __pwned?: unknown }).__pwned ?? null);
    await context.close();
    return { bypassCSP, pwned, outside, violations: violations.length };
  };
  const controlOn = await control(false);
  const controlOff = await control(true);
  console.log(`positive control: ${JSON.stringify([controlOn, controlOff])}`);
  expect(controlOn.pwned).toBeNull();
  expect(controlOn.violations).toBeGreaterThanOrEqual(2);
  expect(controlOff.pwned).toBe("control");
  expect(controlOff.outside).toEqual(["https://evil.example/control.png"]);

  if (EVIDENCE !== undefined) {
    mkdirSync(dirname(EVIDENCE), { recursive: true });
    writeFileSync(EVIDENCE, `${JSON.stringify({ note: "Spec 044, item 3, from wiki.spec.ts. Requests after sign-in, in the reader and the preview.", withCsp, withoutCsp, positiveControl: [controlOn, controlOff] }, null, 2)}\n`);
  }
});

test("verification 4: reader and preview give the same output for each fixture, links and claims included", async ({ page, request }) => {
  test.setTimeout(480_000);
  await signIn(page);
  const rendered = (root: Locator) => root.evaluate((el) => el.innerHTML);
  const settled = async (read: () => Promise<string>) => {
    // The same HTML twice in a row, with nothing still loading, means it has finished arriving.
    let last = "";
    await expect.poll(async () => {
      const now = await read();
      // A link reads as unknown until the page list has come, and here that list is always complete.
      const same = now === last && now !== "" && !now.includes("Loading a claim") && !now.includes("animate-pulse") && !now.includes('data-link-state="unknown"');
      last = now;
      return same;
    }, { intervals: [300] }).toBe(true);
    return last;
  };
  const results: { name: string; bytes: number; links: number; chips: number; same: boolean }[] = [];
  for (const name of ["basics.md", "links.md", "claims.md", "awkward.md", "hostile.md"]) {
    const body = fixture(name);
    const title = `Fixture ${name} ${run}`;
    await page.goto(`${pagesUrl()}/-/new`);
    await page.getByLabel("Title").fill(title);
    await expect(editor(page)).toBeVisible();
    await setText(page, body);
    const previewRoot = preview(page);
    // Before saving: every claim here came by the lookup a draft uses.
    const fromDraft = await settled(() => rendered(previewRoot));
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("heading", { name: title, level: 2 })).toBeVisible();
    const id = new URL(page.url()).pathname.split("/").at(-1) ?? "";
    const markdown = reader(page);
    const evidence = page.getByTestId("claim-evidence");
    const readMarkdown = await settled(() => rendered(markdown));
    const readEvidence = (await evidence.count()) > 0 ? await settled(() => rendered(evidence)) : "";

    await page.goto(`${pagesUrl()}/${id}/edit`);
    await expect(editor(page)).toBeVisible();
    const inPreview = previewRoot.getByTestId("page-markdown");
    const fromPage = await settled(() => rendered(inPreview));
    const fromPageEvidence = readEvidence === "" ? "" : await settled(() => rendered(previewRoot.getByTestId("claim-evidence")));

    expect(fromPage, `${name}: preview of the saved page against the reader`).toBe(readMarkdown);
    expect(fromPageEvidence, `${name}: evidence`).toBe(readEvidence);
    const draftMarkdown = await page.evaluate((html) => {
      const box = document.createElement("div");
      box.innerHTML = html.replaceAll(/<(script|img)/g, "<x-$1");
      return box.querySelector("[data-testid=page-markdown]")?.innerHTML ?? "";
    }, fromDraft);
    expect(draftMarkdown, `${name}: preview of the unsaved draft against the reader`).toBe(readMarkdown);
    results.push({ name, bytes: readMarkdown.length, links: (readMarkdown.match(/data-link-state/g) ?? []).length, chips: (readMarkdown.match(/data-claim=/g) ?? []).length, same: true });
    // The console's idea of what's a reference is the server's, on the same text.
    const saved = (await pageBody(request, id)).body;
    expect(saved.trimEnd()).toBe(body.trimEnd());
    const edges = await get<{ items: { kind: string; target: string }[] }>(request, api(`/pages/${id}/links?limit=200`));
    const onServer = edges.items.map((e) => `${e.kind === "claim_ref" ? "claim" : "link"} ${e.target}`);
    const inConsole = prepareWikiText(saved).tokens.map((t) => (t.kind === "claim" ? `claim ${t.id}` : `link ${t.slug}`));
    expect([...new Set(inConsole)].sort(), `${name}: references`).toEqual([...new Set(onServer)].sort());
  }
  console.log(`reader and preview: ${JSON.stringify(results)}`);
  const claims = results.find((r) => r.name === "claims.md");
  expect(claims?.chips).toBe(4);
  // Three to the target, two broken, the `[[ and ]]` that really is a link to `and`, and one in the table. The one inside a Markdown link is plain text.
  const links = results.find((r) => r.name === "links.md");
  expect(links?.links).toBe(7);
});

test("claim references show the claim, its status, its trust tier and its evidence, in the reader", async ({ page, request }) => {
  await signIn(page);
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Claims shown ${run}`, body: fixture("claims.md"), rationale: "fixture", idempotency_key: `cs-${run}` });
  await page.goto(`${pagesUrl()}/${made.page_id}`);
  const body = reader(page);
  const current = body.locator(`[data-claim="${world.current}"]`).first();
  await expect(current).toContainText("pump room opened in 1912");
  await expect(current).toContainText("Asserted");
  const old = body.locator(`[data-claim="${world.old}"]`);
  await expect(old).toContainText("1913");
  await expect(old).toContainText("Superseded");
  await expect(body.getByText("Missing claim no-such-claim")).toBeVisible();
  const evidence = page.getByTestId("claim-evidence");
  await expect(evidence.getByRole("link", { name: "pump-history.txt" }).first()).toBeVisible();
  await expect(evidence).toContainText("opened in 1912");
  await expect(evidence).toContainText("again in 1913");
  await current.click();
  await expect(page.locator(`[id="claim-evidence-${world.current}"]`)).toBeFocused();
});

test("claim autocomplete offers what a keyword search finds, and inserts the id", async ({ page, request }) => {
  await signIn(page);
  await page.goto(`${pagesUrl()}/-/new`);
  await page.getByLabel("Title").fill(`Claim typing ${run}`);
  await editor(page).focus();
  // A new claim reaches the keyword index a moment after it's written, and suggestions come from there.
  await expect.poll(async () => (await get<{ hits: { id: string }[] }>(request, api("/search/lexical?q=pump&kind=claim"))).hits.map((h) => h.id), { timeout: 120_000, intervals: [500] }).toContain(world.current);
  await page.keyboard.type("{{claim:pump");
  const option = page.locator(".cm-tooltip-autocomplete li", { hasText: "1912" }).first();
  await expect(option).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".cm-tooltip-autocomplete li", { hasText: "1912 (old)" })).toHaveCount(0);
  await acceptCompletion(page);
  await expect.poll(() => editorText(page)).toBe(`{{claim:${world.current}}}`);
  await expect(preview(page).locator(`[data-claim="${world.current}"]`)).toContainText("pump room opened in 1912");
  await page.getByRole("button", { name: /Discard/ }).first().click();
});

test("verification 5: an unsaved draft survives a reload, and is discarded after it's saved", async ({ page, request }) => {
  const problems = problemsOf(page);
  await signIn(page);
  const drafts = () => page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith("memex.draft.")));
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Drafted ${run}`, body: "Saved text.\n", rationale: "fixture", idempotency_key: `d-${run}` });
  const edit = `${pagesUrl()}/${made.page_id}/edit`;

  await page.goto(edit);
  await expect(editor(page)).toBeVisible();
  await setText(page, "Unsaved text.\n");
  // Reloaded at once, inside the pause before a write, so this is the flush on leaving.
  await page.reload();
  await expect(editor(page)).toBeVisible();
  await expect(page.getByText(/Restored your unsaved draft/)).toBeVisible();
  await expect.poll(() => editorText(page)).toBe("Unsaved text.\n");
  await expect(preview(page)).toContainText("Unsaved text.");
  expect((await pageBody(request, made.page_id)).body).toBe("Saved text.\n");
  expect(await drafts()).toHaveLength(1);

  // A real crash, with no chance to flush: the timer wrote it.
  await setText(page, "Text before a crash.\n");
  await expect.poll(async () => page.evaluate(() => Object.entries(localStorage).find(([k]) => k.startsWith("memex.draft."))?.[1] ?? "")).toContain("Text before a crash.");
  await page.close();
  const crashed = await page.context().newPage();
  await crashed.goto(edit);
  await expect(crashed.locator(".cm-content")).toBeVisible();
  await expect.poll(() => editorText(crashed)).toBe("Text before a crash.\n");

  // Saved: the draft is gone and doesn't come back.
  await crashed.getByRole("button", { name: "Save", exact: true }).click();
  await expect(reader(crashed)).toContainText("Text before a crash.");
  expect((await pageBody(request, made.page_id)).body).toBe("Text before a crash.\n");
  const after = () => crashed.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith("memex.draft.")));
  expect(await after()).toEqual([]);
  await crashed.goto(edit);
  await expect(crashed.locator(".cm-content")).toBeVisible();
  await expect(crashed.getByText(/Restored your unsaved draft/)).toHaveCount(0);
  await expect.poll(() => editorText(crashed)).toBe("Text before a crash.\n");

  // A draft of a new page, with its title. Discarding it clears it.
  await crashed.goto(`${pagesUrl()}/-/new`);
  await crashed.getByLabel("Title").fill(`Never saved ${run}`);
  await expect(crashed.locator(".cm-content")).toBeVisible();
  await setText(crashed, "New words.\n");
  await crashed.reload();
  await expect(crashed.getByLabel("Title")).toHaveValue(`Never saved ${run}`);
  await expect.poll(() => editorText(crashed)).toBe("New words.\n");
  await crashed.getByRole("button", { name: "Discard draft" }).click();
  await expect.poll(() => editorText(crashed)).toBe("");
  await expect(crashed.getByLabel("Title")).toHaveValue("");
  await expect.poll(after).toEqual([]);
  await crashed.reload();
  await expect(crashed.getByLabel("Title")).toHaveValue("");
  await expect(crashed.getByText(/Restored your unsaved draft/)).toHaveCount(0);

  // A draft started before someone else's save still can't overwrite it.
  await crashed.goto(edit);
  await expect(crashed.locator(".cm-content")).toBeVisible();
  await setText(crashed, "A stale draft.\n");
  await crashed.reload();
  await send(request, api(`/pages/${made.page_id}/edit`), { base_version: 2, body: "Someone else's.\n", rationale: "other", idempotency_key: `d2-${run}` });
  await crashed.reload();
  await expect(crashed.getByText(/This draft started from revision 2, and the page is now at 3/)).toBeVisible();
  await crashed.getByRole("button", { name: "Save", exact: true }).click();
  await expect(crashed.getByTestId("conflict-view")).toBeVisible();
  expect((await pageBody(request, made.page_id)).body).toBe("Someone else's.\n");

  expect(problems).toEqual([]);
});

test("agent and human edits are attributed, and a held edit says it's waiting for review", async ({ browser, page, request }) => {
  const problems = problemsOf(page);
  await signIn(page);
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Attributed ${run}`, body: "By a person.\n", rationale: "fixture", idempotency_key: `at-${run}` });
  const out = await send<{ changeset: { id: string; state: string } }>(request, api(`/pages/${made.page_id}/edit`), { base_version: 1, body: "By an agent.\n", rationale: "agent's reason", idempotency_key: `at2-${run}` }, 200, world.agentToken);
  expect(out.changeset.state).toBe("pending");
  await send(request, api(`/changesets/${out.changeset.id}/approve`), { reason: "fine" });
  await page.goto(`${pagesUrl()}/${made.page_id}`);
  await expect(page.locator("article header")).toContainText("Agent");
  await expect(page.locator("article header")).toContainText(world.agentId);
  await page.goto(`${pagesUrl()}/${made.page_id}/history`);
  const rows = page.getByRole("region", { name: "History" }).getByRole("listitem");
  await expect(rows.filter({ hasText: "Revision 2" })).toContainText("Agent");
  await expect(rows.filter({ hasText: "Revision 1" })).toContainText("Person");

  // Deleting always goes to review, and the page says what happened and stays.
  await page.goto(`${pagesUrl()}/${made.page_id}`);
  await page.getByRole("button", { name: "Delete" }).click();
  const panel = page.getByRole("region", { name: new RegExp(`Delete Attributed ${run}`) });
  await panel.getByRole("button", { name: "Propose delete" }).click();
  await expect(page.getByText("This delete is waiting for review.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Open it in Review" })).toHaveAttribute("href", /\/review\/.+/);
  await expect(reader(page)).toContainText("By an agent.");

  // An agent using the console sees its edit held too.
  const agent = await newSession(browser, world.agentToken);
  await agent.page.goto(`${pagesUrl()}/${made.page_id}/edit`);
  await expect(agent.page.locator(".cm-content")).toBeVisible();
  await setText(agent.page, "An agent's second edit.\n");
  await agent.page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(agent.page.getByText("Your edit is waiting for review.")).toBeVisible();
  await expect(reader(agent.page)).toContainText("By an agent.");
  expect((await pageBody(request, made.page_id)).body).toBe("By an agent.\n");
  expect(await agent.page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith("memex.draft.")))).toEqual([]);
  await agent.context.close();
  expect(problems).toEqual([]);
});

test("a read token can read but has nothing to edit with, and a direct link says why", async ({ page }) => {
  await signIn(page, world.readToken);
  await page.goto(`${pagesUrl()}/${world.target.id}`);
  await expect(page.getByRole("heading", { name: `Target moved ${run}`, level: 2 })).toBeVisible();
  for (const name of ["Rename", "Delete"]) await expect(page.getByRole("button", { name })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Edit" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "History" })).toBeVisible();
  await page.goto(`${pagesUrl()}/${world.target.id}/edit`);
  await expect(page.getByRole("alert")).toContainText("You don't have permission to edit pages in this vault.");
  await expect(page.locator(".cm-content")).toHaveCount(0);
  await page.goto(`${pagesUrl()}/-/new`);
  await expect(page.getByRole("alert")).toContainText("You don't have permission to edit pages in this vault.");
  await page.goto(pagesUrl());
  await expect(page.getByRole("link", { name: "New page" })).toHaveCount(0);
});

test("export and import round-trip through files, and a bad file says why", async ({ page, request }) => {
  await signIn(page);
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Export me ${run}`, body: "Exported [[text]].\n", rationale: "fixture", idempotency_key: `ex-${run}` });
  await page.goto(`${pagesUrl()}/${made.page_id}`);
  const downloading = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export" }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(`export-me-${run}.md`);
  const path = await download.path();
  const file = readFileSync(path, "utf8");
  expect(file).toContain(`title: "Export me ${run}"`);
  expect(file).toContain("Exported [[text]].");

  // Over the same page: changing only the text.
  const edited = file.replace("Exported", "Imported over");
  await page.locator("label", { hasText: "Import over this page" }).locator("input").setInputFiles({ name: "edited.md", mimeType: "text/markdown", buffer: Buffer.from(edited) });
  await expect(reader(page)).toContainText("Imported over");
  expect((await pageBody(request, made.page_id)).current_revision).toBe(2);

  // A new page from a file, from the list. A file naming a slug that's taken doesn't overwrite it, and says so.
  const title = `From a file ${run}`;
  await page.goto(pagesUrl());
  const input = page.locator("label", { hasText: /^Import$/ }).locator("input");
  await input.setInputFiles({ name: "new.md", mimeType: "text/markdown", buffer: Buffer.from(`# ${title}\n\nBody from a file.\n`) });
  await expect(page.getByRole("heading", { name: title, level: 2 })).toBeVisible();
  await expect(reader(page)).toContainText("Body from a file.");
  await page.goto(pagesUrl());
  await input.setInputFiles({ name: "same.md", mimeType: "text/markdown", buffer: Buffer.from(edited) });
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await pageBody(request, made.page_id)).current_revision).toBe(2);
  await input.setInputFiles({ name: "empty.md", mimeType: "text/markdown", buffer: Buffer.from("no heading here") });
  await expect(page.getByRole("alert")).toContainText("needs a front matter block");
});

test("the list and the tree show pages, filter them, and keep that in the URL", async ({ page, request }) => {
  const parent = `tree-${run}`;
  for (const [title, slug] of [[`Tree root ${run}`, parent], [`Tree child ${run}`, `${parent}-child`], [`Tree grandchild ${run}`, `${parent}-child-deep`]] as const) {
    await send(request, api("/pages"), { title, slug, body: "x\n", rationale: "fixture", idempotency_key: `tree-${slug}` });
  }
  await signIn(page);
  await page.goto(`${pagesUrl()}?q=${encodeURIComponent(`tree-${run}`)}`);
  const table = page.getByRole("table", { name: "Pages" });
  await expect(table.getByRole("row")).toHaveCount(4);
  await page.getByRole("button", { name: "Tree" }).click();
  await expect(page).toHaveURL(/view=tree/);
  await page.getByLabel("Filter by title or slug").fill("");
  await expect(page).toHaveURL(/view=tree$/);
  const indent = (name: string) => table.getByRole("link", { name }).evaluate((el) => (el.parentElement ? parseFloat(getComputedStyle(el.parentElement).paddingLeft) : -1));
  await expect.poll(() => indent(`Tree root ${run}`)).toBe(0);
  const child = await indent(`Tree child ${run}`);
  expect(child).toBeGreaterThan(0);
  expect(await indent(`Tree grandchild ${run}`)).toBeGreaterThan(child);
  await page.reload();
  await expect(page.getByRole("button", { name: "Tree" })).toHaveAttribute("aria-pressed", "true");
  await page.getByLabel("Filter by title or slug").fill("nothing has this name");
  await expect(page.getByText("No page matches this filter.")).toBeVisible();
});

test("states: a missing page, a failed save that keeps the text, and an unknown address", async ({ page, request }) => {
  await signIn(page);
  await page.goto(`${pagesUrl()}/no-such-page`);
  await expect(page.getByText("Couldn't find this page.")).toBeVisible();
  await page.goto(`${pagesUrl()}/-/nope`);
  await expect(page.getByText("There's nothing at this address.")).toBeVisible();

  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Fails ${run}`, body: "Start.\n", rationale: "fixture", idempotency_key: `f-${run}` });
  await page.goto(`${pagesUrl()}/${made.page_id}/edit`);
  await expect(editor(page)).toBeVisible();
  await setText(page, "Survives a failure.\n");
  await page.route(`**/pages/${made.page_id}/edit`, (route) => route.fulfill({ status: 500, json: { error: { code: "internal", message: "the request failed", request_id: "req-wiki-1" } } }));
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "the request failed" }).first()).toContainText("req-wiki-1");
  await expect.poll(() => editorText(page)).toBe("Survives a failure.\n");
  await page.unroute(`**/pages/${made.page_id}/edit`);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(reader(page)).toContainText("Survives a failure.");
});

test("a new title that makes a taken slug is refused, keeping the text", async ({ page }) => {
  await signIn(page);
  await page.goto(`${pagesUrl()}/-/new`);
  await page.getByLabel("Title").fill(`Target moved ${run}`);
  await expect(editor(page)).toBeVisible();
  await setText(page, "Text to keep.\n");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText(/Your text is still here/)).toBeVisible();
  await expect.poll(() => editorText(page)).toBe("Text to keep.\n");
  await page.getByLabel("Title").fill(`Target moved again ${run}`);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("heading", { name: `Target moved again ${run}`, level: 2 })).toBeVisible();
});

test("axe finds no violations on the list, reader, editor, conflict, history and diff", async ({ page, request }) => {
  const check = async (when: string) => {
    const { violations } = await new AxeBuilder({ page }).analyze();
    expect(violations.flatMap((v) => v.nodes.map((n) => `${when}: ${v.id}: ${String(n.target)} ${n.failureSummary ?? ""}`))).toEqual([]);
  };
  await signIn(page);
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Accessible ${run}`, body: fixture("links.md"), rationale: "fixture", idempotency_key: `ax-${run}` });
  await send(request, api(`/pages/${made.page_id}/edit`), { base_version: 1, body: fixture("claims.md"), rationale: "fixture", idempotency_key: `ax2-${run}` });
  await page.goto(pagesUrl());
  await expect(page.getByRole("table", { name: "Pages" })).toBeVisible();
  await check("list");
  await page.goto(`${pagesUrl()}?view=tree`);
  await expect(page.getByRole("table", { name: "Pages" })).toBeVisible();
  await check("tree");
  await page.goto(`${pagesUrl()}/${made.page_id}`);
  await expect(page.getByTestId("claim-evidence")).toContainText("opened in 1912");
  await check("reader");
  await page.goto(`${pagesUrl()}/${made.page_id}/history`);
  await expect(page.getByRole("region", { name: "History" }).getByRole("listitem")).toHaveCount(2);
  await check("history");
  await page.goto(`${pagesUrl()}/${made.page_id}/diff?from=1&to=2`);
  await expect(page.getByText("Before").first()).toBeVisible();
  await check("diff");
  await page.goto(`${pagesUrl()}/-/new`);
  await expect(editor(page)).toBeVisible();
  await page.getByLabel("Title").fill("Typed");
  await setText(page, fixture("basics.md"));
  await page.getByRole("button", { name: /Discard/ }).first().waitFor();
  await check("editor");
  await page.getByRole("button", { name: /Discard/ }).first().click();
  await page.goto(`${pagesUrl()}/${made.page_id}/edit`);
  await expect(editor(page)).toBeVisible();
  await send(request, api(`/pages/${made.page_id}/edit`), { base_version: 2, body: "Moved.\n", rationale: "fixture", idempotency_key: `ax3-${run}` });
  await setText(page, "Mine.\n");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByTestId("conflict-view")).toBeVisible();
  await check("conflict");
});

test("the editor works by keyboard at the narrowest supported width", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 700 });
  await signIn(page);
  await page.goto(`${pagesUrl()}/-/new`);
  await expect(editor(page)).toBeVisible();
  const overflow = () => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(await overflow()).toBeLessThanOrEqual(0);
  await page.getByLabel("Title").fill(`Keyboard ${run}`);
  await page.keyboard.press("Tab");
  await expect(editor(page)).toBeFocused();
  await page.keyboard.type("By keyboard, with a link to [[targ");
  await expect(page.locator(".cm-tooltip-autocomplete")).toBeVisible();
  await acceptCompletion(page);
  // Tab leaves the text, as it mustn't trap anyone, and the separator is next and moves by arrows.
  await page.keyboard.press("Tab");
  await expect(page.getByRole("separator", { name: "Resize the editor and preview" })).toBeFocused();
  expect(await page.getByRole("separator").evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("solid");
  await page.keyboard.press("ArrowLeft");
  await expect(page.getByRole("separator")).toHaveAttribute("aria-valuenow", "48");
  await editor(page).focus();
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("heading", { name: `Keyboard ${run}`, level: 2 })).toBeVisible();
});

test("chunks: the sign-in route loads no wiki code, and the wiki's sizes are reported", async ({ page }) => {
  const scripts = new Set<string>();
  page.on("response", (r) => { if (r.request().resourceType() === "script") scripts.add(new URL(r.url()).pathname); });
  await page.goto("/login");
  await expect(page.getByLabel("Access token")).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect([...scripts].filter((u) => /wiki|markdown-editor|codemirror/.test(u))).toEqual([]);
  await signIn(page);
  await page.goto(pagesUrl());
  await expect(page.getByRole("table", { name: "Pages" })).toBeVisible();
  await page.waitForLoadState("networkidle");
  // The list and the reader don't load CodeMirror.
  expect([...scripts].filter((u) => u.includes("markdown-editor"))).toEqual([]);
  await page.goto(`${pagesUrl()}/-/new`);
  await expect(editor(page)).toBeVisible();
  expect([...scripts].filter((u) => u.includes("markdown-editor"))).toHaveLength(1);
});
