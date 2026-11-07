import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const origin = process.env.MEMEX_E2E_URL ?? "";
const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };

/** Signs in from the page, as the console does, so the browser holds the cookie. */
async function signIn(page: Page, token = adminToken) {
  await page.goto("/");
  const status = await page.evaluate(async (t) => {
    const res = await fetch("/api/v1/session", {
      method: "POST",
      headers: { "content-type": "application/json", "x-memex-csrf": "1" },
      body: JSON.stringify({ token: t }),
    });
    return res.status;
  }, token);
  expect(status).toBe(201);
}

const whoamiStatus = (page: Page) => page.evaluate(async () => (await fetch("/api/v1/whoami")).status);

async function newVault(request: APIRequestContext) {
  const res = await request.post("/api/v1/vaults", { headers: bearer, data: { name: `e2e-${Date.now()}-${Math.floor(Math.random() * 1e6)}` } });
  expect(res.status()).toBe(201);
  return ((await res.json()) as { id: string }).id;
}

test("verification 3: the session cookie can't be read from page script", async ({ page, context }) => {
  await signIn(page);
  expect(await whoamiStatus(page)).toBe(200);
  const cookie = (await context.cookies()).find((c) => c.name === "memex_session");
  expect(cookie).toMatchObject({ httpOnly: true, sameSite: "Strict", path: "/api/v1" });
  expect(cookie?.value.startsWith("mxs_")).toBe(true);
  expect(await page.evaluate(() => document.cookie)).toBe("");
  // The cookie's own path too, since a Path attribute alone would also hide it from `/`.
  await page.goto("/api/v1/whoami");
  await expect(page.locator("body")).toContainText("e2e-admin");
  expect(await page.evaluate(() => document.cookie)).toBe("");
});

test("verification 4: an uploaded HTML file with a script doesn't run in the console's origin", async ({ page, request }) => {
  const vault = await newVault(request);
  const html = `<!doctype html><script>localStorage.setItem("pwned", "yes")</script><p>${Date.now()}</p>`;
  const upload = await request.post(`/api/v1/vaults/${vault}/sources?name=evil.html`, { headers: bearer, data: html });
  expect(upload.status()).toBe(201);
  const source = ((await upload.json()) as { source: { id: string } }).source.id;
  const url = `/api/v1/vaults/${vault}/sources/${source}/file`;
  await signIn(page);
  const pwned = () => page.evaluate(() => localStorage.getItem("pwned"));

  // A control: served as a plain page on this origin, the same file does run, so this test could fail.
  await page.route("**/control-page", (route) => route.fulfill({ contentType: "text/html", body: html }));
  await page.goto("/control-page");
  expect(await pwned()).toBe("yes");
  await page.unroute("**/control-page");
  await page.evaluate(() => localStorage.removeItem("pwned"));

  // Opened directly, it downloads and nothing runs.
  await page.goto("/");
  const download = page.waitForEvent("download");
  await page.goto(url).catch(() => undefined);
  expect((await download).suggestedFilename()).toBe("evil.html");
  await page.goto("/");
  expect(await pwned()).toBeNull();

  // As a link click from the console, in a frame, and as a script: none of them run it either.
  const viaClick = page.waitForEvent("download");
  await page.evaluate((href) => {
    const a = document.createElement("a");
    a.href = href;
    a.textContent = "open";
    document.body.append(a);
    a.click();
  }, url);
  await (await viaClick).cancel();
  await page.evaluate((src) => {
    const frame = document.createElement("iframe");
    frame.src = src;
    document.body.append(frame);
    const script = document.createElement("script");
    script.src = src;
    document.body.append(script);
  }, url);
  await page.waitForTimeout(500);
  expect(await pwned()).toBeNull();

  // The route itself answers as the spec says.
  const headers = await page.evaluate(async (u) => {
    const res = await fetch(u);
    return { status: res.status, disposition: res.headers.get("content-disposition"), csp: res.headers.get("content-security-policy") };
  }, url);
  expect(headers.status).toBe(200);
  expect(headers.disposition).toBe('attachment; filename="evil.html"');
  expect(headers.csp).toContain("sandbox");
});

test("verification 2: a cross-site POST changes nothing, and the custom header can't be sent from there", async ({ page, request }) => {
  await signIn(page);
  const vaultName = `csrf-${Date.now()}`;
  // Same origin without the header: the guard itself refuses it, with the cookie attached.
  const refused = await page.evaluate(async (name) => {
    const res = await fetch("/api/v1/vaults", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) });
    return { status: res.status, code: ((await res.json()) as { error: { code: string } }).error.code };
  }, vaultName);
  expect(refused).toEqual({ status: 403, code: "csrf" });

  // A different site: localhost and 127.0.0.1 are separate sites to a browser. Both kinds of cross-site request fail.
  const other = origin.replace("127.0.0.1", "localhost");
  expect(other).not.toBe(origin);
  const attacker = await page.context().newPage();
  await attacker.goto(other);
  const outcome = await attacker.evaluate(
    async ({ target, name }) => {
      await fetch(`${target}/api/v1/vaults`, { method: "POST", mode: "no-cors", credentials: "include", headers: { "content-type": "text/plain" }, body: JSON.stringify({ name }) }).catch(() => undefined);
      return fetch(`${target}/api/v1/vaults`, { method: "POST", credentials: "include", headers: { "content-type": "application/json", "x-memex-csrf": "1" }, body: JSON.stringify({ name }) })
        .then((r) => `sent ${r.status}`)
        .catch((e: Error) => `blocked: ${e.name}`);
    },
    { target: origin, name: vaultName },
  );
  expect(outcome).toBe("blocked: TypeError");
  const listed = (await (await request.get("/api/v1/vaults?limit=200", { headers: bearer })).json()) as { items: { name: string }[] };
  expect(listed.items.map((v) => v.name)).not.toContain(vaultName);
  await attacker.close();
});

test("verification 2: revoking the token ends its browser session at once", async ({ page, request }) => {
  const me = (await (await request.get("/api/v1/whoami", { headers: bearer })).json()) as { principal: { id: string } };
  const created = await request.post("/api/v1/tokens", { headers: bearer, data: { name: "to-revoke", principal_id: me.principal.id, grants: [] } });
  expect(created.status()).toBe(201);
  const { token, details } = (await created.json()) as { token: string; details: { id: string } };
  await signIn(page, token);
  expect(await whoamiStatus(page)).toBe(200);
  expect((await request.delete(`/api/v1/tokens/${details.id}`, { headers: bearer })).status()).toBe(204);
  expect(await whoamiStatus(page)).toBe(401);
});

test("an <img> tag sends the cookie, which a bearer header can't do", async ({ page, request }) => {
  const vault = await newVault(request);
  await page.goto("/");
  const imageStatus = async () => {
    const path = `/api/v1/vaults/${vault}/sources/no-such-source/pages/0/thumbnail`;
    const response = page.waitForResponse((r) => r.url().endsWith(path));
    await page.evaluate((src) => { new Image().src = src; }, path);
    return (await response).status();
  };
  expect(await imageStatus()).toBe(401);
  await signIn(page);
  expect(await imageStatus()).toBe(404);
});

test("logout ends the session", async ({ page }) => {
  await signIn(page);
  expect(await whoamiStatus(page)).toBe(200);
  const status = await page.evaluate(async () => (await fetch("/api/v1/session", { method: "DELETE", headers: { "x-memex-csrf": "1" } })).status);
  expect(status).toBe(204);
  expect(await whoamiStatus(page)).toBe(401);
});

test("verification 6: the binary serves the console alone, under its CSP, with deep links and no remote requests", async ({ page }) => {
  const remote: string[] = [];
  const problems: string[] = [];
  page.on("request", (r) => { if (!r.url().startsWith(origin) && !r.url().startsWith("data:")) remote.push(r.url()); });
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => { if (m.type() === "error") problems.push(`console: ${m.text()}`); });
  const main = await page.goto("/");
  expect(main?.headers()["cache-control"]).toBe("no-store");
  expect(main?.headers()["content-security-policy"]).toContain("default-src 'self'");
  await expect(page.getByRole("heading", { name: "memexvault" })).toBeVisible();
  const deep = await page.goto("/gallery");
  expect(deep?.status()).toBe(200);
  await page.waitForLoadState("networkidle");
  // Overlays are where libraries tend to add inline styles, which the CSP would block.
  await page.getByRole("button", { name: "Open dialog" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Open menu" }).click();
  await page.keyboard.press("Escape");
  await page.reload();
  await page.waitForLoadState("networkidle");
  expect(remote).toEqual([]);
  // Radix's modal scroll lock adds a <style> element, which this CSP blocks. Spec 037's Result records the cost and the choice. Anything else is a failure.
  expect(problems.filter((p) => !p.startsWith("console: Applying inline style violates"))).toEqual([]);
});
