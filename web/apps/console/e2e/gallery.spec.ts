import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const shots = process.env.SHOT_DIR;

test("console page loads with no errors or remote requests", async ({ page }) => {
  const errors: string[] = [];
  const remote: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("request", (r) => { if (!r.url().startsWith("http://127.0.0.1:4173")) remote.push(r.url()); });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "memexvault" })).toBeVisible();
  expect(errors).toEqual([]);
  await page.goto("/gallery");
  await page.waitForLoadState("networkidle");
  expect(remote).toEqual([]);
});

test("axe finds no violations in the gallery, closed and with overlays open", async ({ page }) => {
  await page.goto("/gallery");
  // An open modal hides the page from assistive tech as it should, so page-structure rules don't apply to those states.
  const check = async (when: string) => {
    const builder = new AxeBuilder({ page });
    if (when === "dialog" || when === "palette") builder.disableRules(["region", "landmark-one-main", "page-has-heading-one"]);
    if (when === "menu") builder.disableRules(["region"]); // A transient popover, portalled outside the landmarks.
    const { violations } = await builder.analyze();
    expect(violations.map((v) => `${when}: ${v.id}: ${v.nodes[0]?.target} ${v.nodes[0]?.failureSummary}`)).toEqual([]);
  };
  await check("closed");
  await page.getByRole("button", { name: "Open dialog" }).click();
  await check("dialog");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Open palette" }).click();
  await check("palette");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Open menu" }).click();
  await check("menu");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Show toast" }).click();
  await expect(page.getByRole("button", { name: "Dismiss" })).toBeVisible();
  await check("toast");
});

test("every interactive component works by keyboard with visible focus", async ({ page }) => {
  await page.goto("/gallery");
  const focus = async (name: string) => {
    const b = page.getByRole("button", { name, exact: true });
    await b.focus();
    const outline = await b.evaluate((el) => getComputedStyle(el).outlineStyle);
    return outline;
  };
  // The gallery is a lazy chunk since spec 040, so wait for it. Tab order reaches the first button, and focus is visible.
  await expect(page.getByRole("button", { name: "Approve" })).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Approve" })).toBeFocused();
  expect(await page.getByRole("button", { name: "Approve" }).evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("solid");
  // Dialog: Enter opens, Escape closes, focus returns.
  await focus("Open dialog");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "Reject proposal" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open dialog" })).toBeFocused();
  // Menu: arrows and Enter.
  await focus("Open menu");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("menuitem", { name: "Reprocess" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("menuitem", { name: "Download original" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("ran")).toContainText("Download");
  // Select.
  await page.getByRole("combobox", { name: "Language" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("option", { name: "Swedish" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("option", { name: "German" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("combobox", { name: "Language" })).toContainText("German");
  await expect(page.getByRole("combobox", { name: "Language" })).toBeFocused();
  // Tabs with arrows.
  await page.getByRole("tab", { name: "Pages" }).focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("tab", { name: "Claims" })).toHaveAttribute("data-state", "active");
  // Tooltip on focus.
  await focus("Tooltip");
  await expect(page.getByRole("tooltip").first()).toBeVisible();
  // Palette: filter, arrow, Enter.
  await focus("Open palette");
  await page.keyboard.press("Enter");
  await page.keyboard.type("go to");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("ran")).toContainText("Go to review queue");
  // Split pane with arrows.
  const sep = page.getByRole("separator", { name: "Resize panes" });
  await sep.focus();
  await page.keyboard.press("ArrowRight");
  await expect(sep).toHaveAttribute("aria-valuenow", "52");
  // Table scrolls by keyboard.
  const table = page.getByRole("table", { name: "Documents" });
  await table.focus();
  await page.keyboard.press("PageDown");
  await expect.poll(() => table.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  // Toast dismissal by keyboard.
  await focus("Show toast");
  await page.keyboard.press("Enter");
  await page.getByRole("button", { name: "Dismiss" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: "Dismiss" })).toHaveCount(0);
});

test("the table scrolls 100,000 rows at 50 fps or better and keeps the DOM small", async ({ page }) => {
  await page.goto("/gallery");
  const table = page.getByRole("table", { name: "Documents" });
  await table.scrollIntoViewIfNeeded();
  await expect(table).toHaveAttribute("aria-rowcount", "100001");
  const result = await table.evaluate(async (el) => {
    const frames: number[] = [];
    const duration = 4000;
    const max = el.scrollHeight - el.clientHeight;
    const start = performance.now();
    await new Promise<void>((done) => {
      const tick = (now: number) => {
        frames.push(now);
        const t = (now - start) / duration;
        // A fast scroll: the whole table twice in four seconds, which replaces all visible rows each frame.
        el.scrollTop = (t * 2 % 1) * max;
        if (t < 1) requestAnimationFrame(tick); else done();
      };
      requestAnimationFrame(tick);
    });
    const gaps = frames.slice(1).map((f, i) => f - (frames[i] ?? f)).sort((a, b) => a - b);
    return { fps: (frames.length - 1) / (duration / 1000), p95Ms: gaps[Math.floor(gaps.length * 0.95)], rowsInDom: el.querySelectorAll('[role="row"]').length };
  });
  console.log(`table: ${JSON.stringify(result)}`);
  expect(result.fps).toBeGreaterThanOrEqual(50);
  expect(result.rowsInDom).toBeLessThan(60);
});

for (const width of [1024, 1920]) {
  test(`gallery screenshot at ${width} px has no horizontal overflow`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto("/gallery");
    await page.waitForLoadState("networkidle");
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    if (shots) await page.screenshot({ path: `${shots}/gallery-${width}.png`, fullPage: true });
  });
}
