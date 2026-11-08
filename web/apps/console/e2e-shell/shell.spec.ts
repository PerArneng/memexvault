import { gzipSync } from "node:zlib";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { startServer, stopServer } from "./server";

const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const SECTIONS = ["Ingest", "Documents", "Search", "Pages", "Entities", "Review", "Audit", "Chat", "Admin"];

interface World {
  readonly alpha: { readonly id: string; readonly name: string };
  readonly beta: { readonly id: string; readonly name: string };
  readonly readToken: string;
  readonly noneToken: string;
  readonly sourceId: string;
}
let world: World;

const post = async <T>(request: APIRequestContext, path: string, data: unknown): Promise<T> => {
  const res = await request.post(path, { headers: bearer, data });
  expect(res.status(), path).toBe(201);
  return (await res.json()) as T;
};

test.beforeAll(async ({ request }) => {
  console.log(`server started in ${await startServer()} ms`);
  const alpha = await post<{ id: string; name: string }>(request, "/api/v1/vaults", { name: `alpha-${run}` });
  const beta = await post<{ id: string; name: string }>(request, "/api/v1/vaults", { name: `beta-${run}` });
  const reader = await post<{ id: string }>(request, "/api/v1/principals", { name: `reader-${run}`, kind: "human" });
  const read = await post<{ token: string }>(request, "/api/v1/tokens", { name: "read", principal_id: reader.id, grants: [{ vault_id: alpha.id, scope: "read" }] });
  const nobody = await post<{ id: string }>(request, "/api/v1/principals", { name: `nobody-${run}`, kind: "human" });
  const none = await post<{ token: string }>(request, "/api/v1/tokens", { name: "none", principal_id: nobody.id, grants: [] });
  const upload = await request.post(`/api/v1/vaults/${alpha.id}/sources?name=quarterly-report.txt`, { headers: bearer, data: `report ${run}` });
  expect(upload.status()).toBe(201);
  const { source } = (await upload.json()) as { source: { id: string } };
  world = { alpha, beta, readToken: read.token, noneToken: none.token, sourceId: source.id };
});

test.afterAll(async () => {
  await stopServer();
});

const signIn = async (page: Page, token = adminToken) => {
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
};

const screen = (page: Page, name: string) => page.getByRole("heading", { name, level: 1 });
const nav = (page: Page) => page.getByRole("navigation", { name: "Screens" });
const switcher = (page: Page) => page.getByRole("button", { name: /Switch vault/ });
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    const path = m.location().url === "" ? "" : new URL(m.location().url).pathname;
    // Asking who's signed in gets a 401 when nobody is, and a browser logs that. It's the one expected message.
    if (m.type() === "error" && !(m.text().includes("401") && path === "/api/v1/whoami")) problems.push(m.text());
  });
  return problems;
};

test("verification 1: sign in, switch vault and sign out on the real backend", async ({ page, context }) => {
  const problems = problemsOf(page);
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  await expect(page).toHaveURL(new RegExp("/vaults/[^/]+/documents$"));
  const cookie = (await context.cookies()).find((c) => c.name === "memex_session");
  expect(cookie?.httpOnly).toBe(true);
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(adminToken);

  // Into alpha by URL, then to beta through the switcher, keeping the screen.
  await page.goto(`/vaults/${world.alpha.id}/search`);
  await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
  await expect(switcher(page)).toContainText(world.alpha.name);
  await switcher(page).click();
  await page.getByRole("menuitem", { name: world.beta.name }).click();
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.beta.id}/search$`));
  await expect(page.getByText(`In vault ${world.beta.name}`)).toBeVisible();
  await expect(screen(page, "Search")).toBeVisible();
  await expect(switcher(page)).toContainText(world.beta.name);

  // Every screen is one click away for an admin.
  for (const label of SECTIONS) {
    await nav(page).getByRole("link", { name: label, exact: true }).click();
    await expect(screen(page, label)).toBeVisible();
    await expect(nav(page).getByRole("link", { name: label, exact: true })).toHaveAttribute("aria-current", "page");
  }

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByText("You're signed out.")).toBeVisible();
  expect((await context.cookies()).find((c) => c.name === "memex_session")).toBeUndefined();
  expect(await page.evaluate(async () => (await fetch("/api/v1/whoami")).status)).toBe(401);
  await page.goto(`/vaults/${world.alpha.id}/search`);
  await expect(page).toHaveURL(/\/login\?next=/);
  // No modal was opened, so the strict CSP has nothing to report. The switcher is a menu, which has no scroll lock.
  expect(problems).toEqual([]);
});

test("a wrong token says so and doesn't echo it", async ({ page }) => {
  await page.goto("/login");
  await signIn(page, `mxt_wrong_${run}`);
  await expect(page.getByText("That token wasn't accepted")).toBeVisible();
  await expect(page.getByLabel("Access token")).toHaveValue("");
  await expect(page.locator("body")).not.toContainText(`mxt_wrong_${run}`);
  await expect(page).toHaveURL(/\/login$/);
});

test("too many sign-ins shows the wait, as the server states it", async ({ page }) => {
  await page.route("**/api/v1/session", (route) =>
    route.fulfill({ status: 429, headers: { "retry-after": "42" }, json: { error: { code: "rate_limited", message: "slow down", request_id: "r" } } }),
  );
  await page.goto("/login");
  await signIn(page, "anything");
  await expect(page.getByText("Try again in 42 s")).toBeVisible();
});

test("a sign-in link can't send you to another site", async ({ page }) => {
  await page.goto(`/login?next=${encodeURIComponent("//evil.example/")}`);
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  expect(new URL(page.url()).host).toBe(new URL(test.info().project.use.baseURL ?? "").host);
});

test("verification 2: a read token sees no admin navigation, and direct links explain why", async ({ page, request }) => {
  await page.goto("/login");
  await signIn(page, world.readToken);
  await expect(screen(page, "Documents")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.alpha.id}/documents$`));
  const links = await nav(page).getByRole("link").allTextContents();
  expect(links).toEqual(["Documents", "Search", "Pages", "Entities", "Review", "Audit", "Chat"]);

  for (const path of ["admin", "admin/tokens"]) {
    await page.goto(`/vaults/${world.alpha.id}/${path}`);
    const message = page.getByRole("alert");
    await expect(message).toContainText("You don't have permission to use");
    await expect(message).toContainText("your token has read");
    await expect(page.locator("main")).not.toContainText("isn't built yet");
  }

  // The palette offers the same.
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("dialog", { name: "Command palette" })).toBeVisible();
  await expect(page.getByRole("option", { name: "Go to Search" })).toBeVisible();
  await expect(page.getByRole("option", { name: /Go to Admin/ })).toHaveCount(0);
  await page.keyboard.press("Escape");

  // A vault this token has no grant on: the same answer whether or not it exists, as the server gives.
  for (const id of [world.beta.id, "no-such-vault"]) {
    await page.goto(`/vaults/${id}/documents`);
    await expect(page.getByRole("alert")).toContainText("This vault doesn't exist, or your token has no access to it.");
    await expect(nav(page).getByRole("link")).toHaveCount(0);
  }

  // The API refuses too, so hiding the link isn't the only defence.
  const api = await request.delete(`/api/v1/vaults/${world.alpha.id}`, { headers: { authorization: `Bearer ${world.readToken}` } });
  expect(api.status()).toBe(403);
});

test("a token with no vaults gets an empty state, not an error", async ({ page }) => {
  await page.goto("/login");
  await signIn(page, world.noneToken);
  await expect(screen(page, "No vaults")).toBeVisible();
  await expect(page.getByText("There are no vaults you can open yet.")).toBeVisible();
});

test("verification 7: reloading any URL returns to the same vault and screen", async ({ page }) => {
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  const urls: [string, string][] = SECTIONS.map((label) => [`/vaults/${world.beta.id}/${label.toLowerCase()}`, label]);
  urls.push([`/vaults/${world.alpha.id}/documents/${world.sourceId}`, "Documents"], [`/vaults/${world.alpha.id}/search?q=hello&as_of=5#top`, "Search"], [`/vaults/${world.alpha.id}/admin/tokens`, "Admin"]);
  for (const [url, label] of urls) {
    await page.goto(url);
    await expect(screen(page, label)).toBeVisible();
    await page.reload();
    await expect(screen(page, label)).toBeVisible();
    const vault = url.includes(world.alpha.id) ? world.alpha : world.beta;
    await expect(page.getByText(`In vault ${vault.name}`)).toBeVisible();
    await expect(switcher(page)).toContainText(vault.name);
    const now = new URL(page.url());
    expect(now.pathname + now.search + now.hash).toBe(url);
  }
  // `/` returns to the last vault.
  await page.goto(`/vaults/${world.beta.id}/pages`);
  await expect(screen(page, "Pages")).toBeVisible();
  await page.goto("/");
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.beta.id}/documents$`));
});

test("a signed-out deep link comes back after signing in", async ({ page }) => {
  const url = `/vaults/${world.alpha.id}/search?q=hello`;
  await page.goto(url);
  await expect(page).toHaveURL(/\/login\?next=/);
  await signIn(page);
  await expect(screen(page, "Search")).toBeVisible();
  const now = new URL(page.url());
  expect(now.pathname + now.search).toBe(url);
});

test("session expiry: a revoked token sends you to sign in, and back to the same place", async ({ page, request }) => {
  const me = (await (await request.get("/api/v1/whoami", { headers: bearer })).json()) as { principal: { id: string } };
  const made = await post<{ token: string; details: { id: string } }>(request, "/api/v1/tokens", { name: "expiring", principal_id: me.principal.id, grants: [] });
  await page.goto("/login");
  await signIn(page, made.token);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/pages`);
  await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
  expect((await request.delete(`/api/v1/tokens/${made.details.id}`, { headers: bearer })).status()).toBe(204);
  // The switcher's next read gets the 401.
  await switcher(page).click();
  await page.getByRole("menuitem", { name: world.beta.name }).click();
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.getByText("Your session has ended.")).toBeVisible();
  await signIn(page);
  await expect(screen(page, "Pages")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.beta.id}/pages$`));
});

test("the palette goes to screens, objects and vaults, by keyboard", async ({ page }) => {
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(screen(page, "Documents")).toBeVisible();
  const opener = page.getByRole("button", { name: /Commands/ });

  await opener.click();
  await page.keyboard.type("go to rev");
  await page.keyboard.press("Enter");
  await expect(screen(page, "Review")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);

  // An object from the real API.
  await page.keyboard.press("Control+k");
  await page.keyboard.type("quarterly");
  await expect(page.getByRole("option", { name: /quarterly-report\.txt/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.alpha.id}/documents/${world.sourceId}$`));

  await page.keyboard.press("Control+k");
  await expect(page.getByLabel("Search commands")).toHaveValue("");
  await page.keyboard.type(`switch to ${world.beta.name}`);
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/vaults/${world.beta.id}/documents$`));
});

test("CSP measurement: what the palette's scroll lock does under the strict policy", async ({ page }) => {
  const messages: string[] = [];
  page.on("console", (m) => { if (m.text().includes("Applying inline style violates")) messages.push(m.text()); });
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(screen(page, "Documents")).toBeVisible();
  expect(messages).toHaveLength(0);

  // A long screen, as a real list would be, to find out what scrolls behind the palette. CSSOM sizes it, which the CSP allows.
  await page.locator("main").evaluate((el) => {
    const tall = document.createElement("div");
    tall.textContent = "tall";
    tall.style.height = "4000px";
    el.append(tall);
  });
  const scrolled = () => page.evaluate(() => ({ window: window.scrollY, main: document.querySelector("main")?.scrollTop ?? -1 }));
  await page.mouse.move(600, 500);
  await page.mouse.wheel(0, 300);
  await expect.poll(async () => (await scrolled()).main).toBeGreaterThan(0);
  const beforeOpen = await scrolled();

  const opens = 3;
  for (let i = 0; i < opens; i += 1) {
    await page.keyboard.press("Control+k");
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.mouse.move(600, 600);
    await page.mouse.wheel(0, 300);
    await page.waitForTimeout(200);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  }
  const afterWheels = await scrolled();
  console.log(`palette csp: ${JSON.stringify({ opens, inlineStyleErrors: messages.length, beforeOpen, afterWheels })}`);
  // The page behind didn't move: the shell's layout never scrolls the window, and the wheel over the overlay doesn't reach `main`.
  expect(afterWheels).toEqual(beforeOpen);
  // The error is still there, one per opening. This fails if that changes, so a decision on it gets recorded rather than drifting.
  expect(messages).toHaveLength(opens);
});

test("errors: a toast and the screen give the request id, and a retry works", async ({ page }) => {
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  let broken = true;
  await page.route(`**/api/v1/vaults/${world.alpha.id}`, async (route) => {
    if (broken) await route.fulfill({ status: 500, json: { error: { code: "internal", message: "the request failed", request_id: "req-e2e-1" } } });
    else await route.fallback();
  });
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(page.getByRole("alert").filter({ hasText: "request id: req-e2e-1" })).toBeVisible();
  const toast = page.getByRole("listitem").filter({ hasText: "the request failed" });
  await expect(toast).toContainText("request id: req-e2e-1");
  broken = false;
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
});

test("the error boundary catches a chunk that won't load", async ({ page }) => {
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.route(/\/assets\/shell-[^/]*\.js$/, (route) => route.abort());
  await page.reload();
  await expect(page.getByText("Something went wrong on this page.")).toBeVisible();
  await page.unroute(/\/assets\/shell-[^/]*\.js$/);
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(screen(page, "Documents")).toBeVisible();
});

test("verification 6: the sign-in route's initial JavaScript is at most 200 kB compressed", async ({ page }) => {
  const scripts: Promise<{ url: string; bytes: number; gzip: number }>[] = [];
  page.on("response", (r) => {
    if (r.request().resourceType() === "script") scripts.push(r.body().then((b) => ({ url: new URL(r.url()).pathname, bytes: b.length, gzip: gzipSync(b, { level: 9 }).length })));
  });
  await page.goto("/login");
  await expect(page.getByLabel("Access token")).toBeVisible();
  await page.waitForLoadState("networkidle");
  const loaded = await Promise.all(scripts);
  const gzip = loaded.reduce((sum, s) => sum + s.gzip, 0);
  console.log(`login route scripts: ${JSON.stringify(loaded)} total gzip ${gzip}`);
  expect(gzip).toBeLessThanOrEqual(200_000);
  expect(loaded.map((s) => s.url).filter((u) => /shell|overlays|gallery/.test(u))).toEqual([]);
});

test("axe finds no violations on the sign-in page, the shell, a permission message and the palette", async ({ page }) => {
  const check = async (when: string, disabled: string[] = []) => {
    const { violations } = await new AxeBuilder({ page }).disableRules(disabled).analyze();
    expect(violations.map((v) => `${when}: ${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
  };
  await page.goto("/login");
  await expect(page.getByLabel("Access token")).toBeVisible();
  await check("login");
  await signIn(page, world.readToken);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
  await check("shell");
  await page.goto(`/vaults/${world.alpha.id}/admin`);
  await expect(page.getByRole("alert")).toBeVisible();
  await check("permission");
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("dialog")).toBeVisible();
  // An open modal hides the page from assistive tech as it should, as in the gallery's test.
  await check("palette", ["region", "landmark-one-main", "page-has-heading-one"]);
});

test("the shell works by keyboard with visible focus, at the narrowest supported width", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 700 });
  await page.goto("/login");
  // The form is ready to type into, and Enter submits it.
  await expect(page.getByLabel("Access token")).toBeFocused();
  await page.keyboard.type(adminToken);
  await page.keyboard.press("Enter");
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(switcher(page)).toContainText(world.alpha.name);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  await switcher(page).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("menuitem").first()).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(switcher(page)).toBeFocused();
  expect(await switcher(page).evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("solid");
  const pagesLink = nav(page).getByRole("link", { name: "Pages" });
  await pagesLink.focus();
  expect(await pagesLink.evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("solid");
  await page.keyboard.press("Enter");
  await expect(screen(page, "Pages")).toBeVisible();
});

// Last, as it stops and restarts the server for everything after it.
test("verification 3: stopping the server shows the banner within 5 s, and starting it recovers with no reload", async ({ page }) => {
  await page.goto("/login");
  await signIn(page);
  await expect(screen(page, "Documents")).toBeVisible();
  await page.goto(`/vaults/${world.alpha.id}/documents`);
  await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
  const mark = () => page.evaluate(() => { (window as unknown as { noReload: boolean }).noReload = true; });
  await mark();
  const banner = page.getByTestId("offline-banner");
  await expect(banner).toHaveCount(0);

  const rounds = [];
  for (let round = 0; round < 3; round += 1) {
    const stopped = Date.now();
    await stopServer();
    await expect(banner).toBeVisible({ timeout: 5000 });
    const toBanner = Date.now() - stopped;
    expect(toBanner).toBeLessThanOrEqual(5000);
    // Data on screen stays.
    await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
    // A screen with nothing loaded says why, and loads by itself on recovery.
    if (round === 0) {
      await switcher(page).click();
      await page.getByRole("menuitem", { name: world.beta.name }).click();
      await expect(page.getByText("Can't load this vault while the server is out of reach.")).toBeVisible();
      await expect(page).toHaveURL(new RegExp(`/vaults/${world.beta.id}/documents$`));
    }
    const restarted = Date.now();
    await startServer();
    const serverUp = Date.now() - restarted;
    const up = Date.now();
    await expect(banner).toHaveCount(0, { timeout: 5000 });
    const bannerGone = Date.now() - up;
    if (round === 0) {
      await expect(page.getByText(`In vault ${world.beta.name}`)).toBeVisible({ timeout: 5000 });
      expect(await page.evaluate(() => (window as unknown as { noReload?: boolean }).noReload)).toBe(true);
      await switcher(page).click();
      await page.getByRole("menuitem", { name: world.alpha.name }).click();
      await expect(page.getByText(`In vault ${world.alpha.name}`)).toBeVisible();
    }
    expect(await page.evaluate(() => (window as unknown as { noReload?: boolean }).noReload)).toBe(true);
    rounds.push({ toBanner, serverUp, bannerGone });
  }
  console.log(`offline banner: ${JSON.stringify(rounds)}`);
});
