import { execFileSync } from "node:child_process";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { makePdf } from "../e2e-documents/make-pdf";
import { startServer, stopServer } from "../e2e-shell/server";

// Spec 046. Every test runs on a real server with the real CSP, and proposals come through the API's own write path, so no LLM is called.
// A low threshold makes one approval earn auto-commit, so a test can see it on without 73 samples. What the threshold means is the server's, and spec 025 tests it.
process.env.MEMEX_AUTO_COMMIT_THRESHOLD = "0.2";

const adminToken = process.env.MEMEX_E2E_TOKEN ?? "";
const bearer = { authorization: `Bearer ${adminToken}` };
const run = Date.now().toString(36);
const HOSTILE = `<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script><b>bold-${run}</b>`;

interface World {
  readonly vault: string;
  readonly reviewer: { readonly token: string; readonly id: string };
  readonly second: { readonly token: string; readonly id: string };
  readonly reader: string;
  readonly agent: { readonly token: string; readonly id: string };
  readonly adminAgent: { readonly token: string; readonly id: string };
  readonly segment: string;
  readonly pdfSegment: string;
  readonly pdfSource: string;
  readonly trustedSegment: string;
}
let world: World;
let counter = 0;

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

const SENTENCE = (i: number) => `Vessel number ${String(i)} was built in Stockholm in ${String(1600 + i)}.`;
const TEXT = Array.from({ length: 40 }, (_, i) => SENTENCE(i)).join(" ");

const claimOp = (id: string, segment: string, value: string, quote: string, subject = `vessel ${id}`) => ({
  op: "add_assertion", id, subject, predicate: "built in", value, epistemic: "asserted", machine: true, confidence_permille: 900, valid_from_ms: null, valid_to_ms: null,
  evidence: [[`${id}-e`, { segment_id: segment, quote, stance: "supports" }]],
});

interface Proposed { readonly id: string; readonly state: string }

/** A proposal an agent makes through the API, as any agent's does. */
const propose = async (request: APIRequestContext, ops: unknown[], rationale = "fixture", token = world.agent.token): Promise<Proposed> => {
  counter += 1;
  const out = await send<{ changeset: Proposed }>(request, api("/proposals"), { ops, rationale, idempotency_key: `k${String(counter)}-${run}` }, 200, token);
  return out.changeset;
};
const proposeClaim = async (request: APIRequestContext, rationale = "fixture"): Promise<{ readonly id: string; readonly claim: string }> => {
  counter += 1;
  const claim = `claim-${String(counter)}-${run}`;
  const cs = await propose(request, [claimOp(claim, world.segment, String(1600 + (counter % 6)), SENTENCE(counter % 6))], rationale);
  expect(cs.state, "a claim citing an unverified source waits").toBe("pending");
  return { id: cs.id, claim };
};
const changeset = (request: APIRequestContext, id: string) => get<{ state: string; decided_by: string | null; decision_reason: string | null; decided_at_ms: number | null; reverted_by: string | null; reverts: string | null; original_ops: unknown[] | null; ops: unknown[] }>(request, api(`/changesets/${id}`));

/** A plain approval is on its way when the queue has moved on, so a read has to wait for it. */
const decidedAll = async (request: APIRequestContext, ids: readonly string[]) => {
  await expect.poll(async () => (await Promise.all(ids.map((id) => changeset(request, id)))).filter((c) => c.state === "pending").length).toBe(0);
  return Promise.all(ids.map((id) => changeset(request, id)));
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

test.beforeAll(async ({ request }) => {
  test.setTimeout(420_000);
  console.log(`server started in ${String(await startServer())} ms`);
  const vault = await send<{ id: string }>(request, "/api/v1/vaults", { name: `review-${run}` }, 201);
  const make = async (name: string, kind: string, scope: string) => {
    const p = await send<{ id: string }>(request, "/api/v1/principals", { name: `${name}-${run}`, kind }, 201);
    const t = await send<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: p.id, grants: [{ vault_id: vault.id, scope }] }, 201);
    return { token: t.token, id: p.id };
  };
  const reviewer = await make("reviewer", "human", "admin");
  const second = await make("second", "human", "admin");
  const reader = await make("reader", "human", "read");
  const agent = await make("agent", "agent", "propose");
  const adminAgent = await make("admin-agent", "agent", "admin");
  const upload = async (name: string, body: string | Buffer, tier = "") => {
    const res = await request.post(`/api/v1/vaults/${vault.id}/sources?name=${name}${tier === "" ? "" : `&trust_tier=${tier}`}`, { headers: bearer, data: body });
    expect(res.status(), await res.text()).toBe(201);
    return ((await res.json()) as { source: { id: string } }).source.id;
  };
  const text = await upload(`fleet-${run}.txt`, TEXT);
  const pdf = await upload(`pdf-${run}.pdf`, makePdf(2, (page) => `Document page ${String(page + 1)} of the fleet ${run}`));
  const trusted = await upload(`trusted-${run}.txt`, `The harbour master wrote that the quay opened in 1711. ${run}`, "trusted");
  world = {
    vault: vault.id,
    reviewer,
    second,
    reader: reader.token,
    agent,
    adminAgent,
    segment: await segmentOf(request, vault.id, text),
    pdfSegment: await segmentOf(request, vault.id, pdf),
    pdfSource: pdf,
    trustedSegment: await segmentOf(request, vault.id, trusted),
  };
});

test.afterAll(async () => {
  await stopServer();
});

const signIn = async (page: Page, token: string) => {
  await page.goto("/login");
  await page.getByLabel("Access token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  // A generous wait: the first reads after a server starts are slow on a busy machine.
  await expect(page).toHaveURL(/\/vaults\//, { timeout: 60_000 });
};

const newSession = async (browser: Browser, token: string, viewport = { width: 1800, height: 1100 }): Promise<{ context: BrowserContext; page: Page }> => {
  const context = await browser.newContext({ colorScheme: "dark", baseURL: baseURL(), viewport });
  const page = await context.newPage();
  await signIn(page, token);
  return { context, page };
};

/** Anything the browser reports as wrong. A blocked style or script is a console error, and this includes them. */
const problemsOf = (page: Page) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    const path = m.location().url === "" ? "" : new URL(m.location().url).pathname;
    // A browser logs these answers, and each is what the screen is built to handle: nobody signed in, and a decision someone else made first or an agent isn't allowed.
    const expected = (m.text().includes("401") && path === "/api/v1/whoami") || /40[0-9]/.test(m.text());
    if (m.type() === "error" && !expected) problems.push(m.text());
  });
  return problems;
};

const reviewUrl = (tail = "") => `/vaults/${world.vault}/review${tail}`;
const list = (page: Page) => page.getByTestId("queue-list");
const currentId = async (page: Page) => (await list(page).getAttribute("aria-activedescendant"))?.replace(/^queue-/, "") ?? "";
const item = (page: Page, id: string) => page.locator(`[data-testid=item][data-item="${id}"]`);
const notice = (page: Page) => page.getByTestId("queue-notice");

/** Waits until the keys act on this item: it's current, and on screen. */
const onItem = async (page: Page, id: string) => {
  await expect(list(page)).toHaveAttribute("aria-activedescendant", `queue-${id}`);
  await expect(item(page, id)).toBeVisible();
};

/** Only this run's items count, as a reviewer's queue here also holds what earlier tests left. */
const clearQueue = async (request: APIRequestContext) => {
  for (;;) {
    const queue = await get<{ items: { changeset: { id: string } }[] }>(request, api("/review/queue?state=pending&limit=200"));
    if (queue.items.length === 0) return;
    await send(request, api("/review/batch"), { decisions: queue.items.map((i) => ({ id: i.changeset.id, decision: "reject", reason: "clearing the queue for the next test" })) });
  }
};

test("item 1: 20 items, every kind of decision, by keyboard alone", async ({ browser, request }) => {
  await clearQueue(request);
  const made: { readonly id: string; readonly claim: string }[] = [];
  for (let i = 0; i < 20; i += 1) made.push(await proposeClaim(request));
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  // A click anywhere after this would be a failure, so nothing here may use the mouse.
  await page.goto(reviewUrl());
  await expect(page.getByTestId("pending-count")).toHaveText("20");
  const ids = made.map((m) => m.id);
  const started = Date.now();
  const at = (i: number) => ids[i] ?? "";

  // 1 to 8: approve.
  for (let i = 0; i < 8; i += 1) {
    await onItem(page, at(i));
    await page.keyboard.press("a");
  }
  // 9 to 11: approve with a note.
  for (let i = 8; i < 11; i += 1) {
    await onItem(page, at(i));
    await page.keyboard.press("n");
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.insertText(`Looks right ${String(i)}`);
    await page.keyboard.press("Enter");
  }
  // 12 to 15: reject with a reason. An empty reason is refused and the form stays.
  for (let i = 11; i < 15; i += 1) {
    await onItem(page, at(i));
    await page.keyboard.press("r");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("reason-form")).toContainText("needs a reason");
    await page.keyboard.insertText(`Wrong year ${String(i)}`);
    await page.keyboard.press("Enter");
  }
  // 16: Esc backs out of a form and decides nothing.
  await onItem(page, at(15));
  await page.keyboard.press("r");
  await expect(page.getByTestId("reason-form")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reason-form")).toBeHidden();
  await expect(list(page)).toBeFocused();
  expect(await currentId(page)).toBe(at(15));
  // 16: edit, then approve.
  await page.keyboard.press("e");
  await expect(page.getByTestId("edit-form")).toBeVisible();
  const box = page.getByLabel("The ops, as JSON");
  await expect(box).toBeFocused();
  const edited = (await box.inputValue()).replace('"value": "', '"value": "edited ');
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.insertText(edited);
  await page.keyboard.press("Tab");
  await page.keyboard.insertText("Corrected the value");
  await page.keyboard.press("ControlOrMeta+Enter");
  // 17 and 18: a batch approval.
  await onItem(page, at(16));
  await page.keyboard.press("x");
  await page.keyboard.press("j");
  await onItem(page, at(17));
  await page.keyboard.press("x");
  await expect(page.locator("[data-queue-row][data-checked=true]")).toHaveCount(2);
  await page.keyboard.press("A");
  await page.keyboard.press("Enter");
  // 19 and 20: a batch rejection, going back up the list by key on the way.
  await onItem(page, at(18));
  await page.keyboard.press("j");
  await page.keyboard.press("k");
  await onItem(page, at(18));
  await page.keyboard.press("X");
  await expect(page.locator("[data-queue-row][data-checked=true]")).toHaveCount(2);
  await page.keyboard.press("R");
  await page.keyboard.insertText("Not what the source says");
  await page.keyboard.press("Enter");

  await expect(page.getByText("Nothing is waiting for a decision.")).toBeVisible();
  await expect(page.getByTestId("pending-count")).toHaveText("0");
  console.log(`item 1: 20 items by keyboard in ${String(Date.now() - started)} ms, with waits for the screen`);

  const decided = await decidedAll(request, ids);
  expect(decided.map((c) => c.state)).toEqual([...Array<string>(11).fill("committed"), ...Array<string>(4).fill("rejected"), ...Array<string>(3).fill("committed"), "rejected", "rejected"]);
  expect(decided.every((c) => c.decided_by === world.reviewer.id), "every decision is the person's").toBe(true);
  expect(decided[0]?.decision_reason).toBe("Approved in the console");
  expect(decided[8]?.decision_reason).toBe("Looks right 8");
  expect(decided[11]?.decision_reason).toBe("Wrong year 11");
  expect(decided[15]?.decision_reason).toBe("Corrected the value");
  expect(decided[15]?.original_ops).not.toBeNull();
  expect(JSON.stringify(decided[15]?.ops)).toContain("edited ");
  expect(decided[18]?.decision_reason).toBe("Not what the source says");
  expect(problems).toEqual([]);
  await context.close();
});

test("the keys are all listed, and don't act while typing", async ({ browser, request }) => {
  await clearQueue(request);
  const one = await proposeClaim(request);
  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.goto(reviewUrl());
  await onItem(page, one.id);
  await page.keyboard.press("?");
  await expect(page.getByTestId("key-help")).toContainText("Reject, with a reason");
  await page.keyboard.press("?");
  await expect(page.getByTestId("key-help")).toBeHidden();
  // Typing a's and r's into the filter decides nothing.
  await page.getByLabel("Proposer").focus();
  await page.keyboard.type("arear");
  await expect(page.getByTestId("reason-form")).toBeHidden();
  expect((await changeset(request, one.id)).state).toBe("pending");
  await context.close();
});

test("item 2: each decision records the signed-in person and the reason, shown through the API", async ({ browser, request }) => {
  await clearQueue(request);
  const approved = await proposeClaim(request);
  const rejected = await proposeClaim(request);
  const edited = await proposeClaim(request);
  const { context, page } = await newSession(browser, world.second.token);
  await page.goto(reviewUrl());
  await onItem(page, approved.id);
  await page.getByRole("button", { name: "With a note (n)" }).click();
  await page.getByLabel("A note on this approval").fill(`Approved by the second reviewer ${run}`);
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await onItem(page, rejected.id);
  await page.getByRole("button", { name: "Reject (r)" }).click();
  await page.getByLabel("Why this is rejected").fill(`Rejected by the second reviewer ${run}`);
  await page.getByRole("button", { name: "Reject", exact: true }).click();
  await onItem(page, edited.id);
  await page.getByRole("button", { name: "Edit, then approve (e)" }).click();
  const box = page.getByLabel("The ops, as JSON");
  await box.fill((await box.inputValue()).replace('"value": "', '"value": "fixed '));
  await page.getByLabel("Why it changed").fill(`Edited by the second reviewer ${run}`);
  await page.getByRole("button", { name: "Approve the edit" }).click();
  await expect(page.getByText("Nothing is waiting for a decision.")).toBeVisible();

  const [a, r, e] = (await decidedAll(request, [approved.id, rejected.id, edited.id])) as [Awaited<ReturnType<typeof changeset>>, Awaited<ReturnType<typeof changeset>>, Awaited<ReturnType<typeof changeset>>];
  for (const [row, state, reason] of [[a, "committed", "Approved"], [r, "rejected", "Rejected"], [e, "committed", "Edited"]] as const) {
    expect(row.state).toBe(state);
    expect(row.decided_by).toBe(world.second.id);
    expect(row.decision_reason).toBe(`${reason} by the second reviewer ${run}`);
  }
  expect(e.original_ops).not.toBeNull();
  // The console shows the same record.
  await page.goto(reviewUrl(`/-/changesets/${edited.id}`));
  await expect(page.getByTestId("decided-by")).toHaveText(world.second.id);
  await expect(page.getByTestId("decision-reason")).toHaveText(`Edited by the second reviewer ${run}`);
  await expect(page.getByRole("region", { name: "What was first proposed" })).toBeVisible();
  await expect(page.getByRole("region", { name: "What it does" })).toContainText("fixed ");
  await context.close();
});

test("a refused edit keeps what was typed, and decides nothing", async ({ browser, request }) => {
  await clearQueue(request);
  const one = await proposeClaim(request);
  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.goto(reviewUrl());
  await onItem(page, one.id);
  await page.keyboard.press("e");
  const box = page.getByLabel("The ops, as JSON");
  // Valid JSON and a known op, with a quote the source doesn't hold, which only the server can tell.
  const wrong = (await box.inputValue()).replace(/"quote": "[^"]*"/, '"quote": "words that are nowhere in the source"');
  await box.fill(wrong);
  await page.getByLabel("Why it changed").fill("my reason");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByTestId("edit-form").getByRole("alert")).toBeVisible();
  await expect(box).toHaveValue(wrong);
  await expect(page.getByLabel("Why it changed")).toHaveValue("my reason");
  expect((await changeset(request, one.id)).state).toBe("pending");
  await page.keyboard.press("Escape");
  await expect(list(page)).toBeFocused();
  await context.close();
});

const dump = () => execFileSync(process.env.MEMEX_E2E_DUMP_BIN ?? "", [world.vault], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });

test("item 3: a revert from the console restores the prior state, by the store's dump", async ({ browser, request }) => {
  await clearQueue(request);
  const before = dump();
  const fixture = await proposeClaim(request, "to be reverted");
  await send(request, api(`/changesets/${fixture.id}/approve`), { reason: "fixture" });
  const committed = dump();
  expect(committed, "the commit changed something").not.toBe(before);
  expect(committed).toContain(fixture.claim);

  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  await page.goto(reviewUrl(`/-/changesets/${fixture.id}`));
  await expect(page.getByTestId("rationale")).toHaveText("to be reverted");
  await expect(page.getByTestId("inverse-list")).toContainText("Remove");
  await page.getByRole("button", { name: "Revert", exact: true }).click();
  await page.getByRole("button", { name: "Revert", exact: true }).click();
  await expect(page.getByTestId("reason-form")).toContainText("needs a reason");
  await page.getByLabel("Why this is reverted").fill(`Undone ${run}`);
  await page.getByRole("button", { name: "Revert", exact: true }).click();
  await expect(page.getByTestId("reverted")).toBeVisible();

  const after = dump();
  console.log(`item 3: dump before ${String(before.length)} bytes in ${String(before.split("\n").length - 1)} lines, committed ${String(committed.length)} bytes, after the revert ${String(after.length)} bytes`);
  expect(after === before, "the dump is the same, byte for byte").toBe(true);
  const row = await changeset(request, fixture.id);
  expect(row.reverted_by).not.toBeNull();
  const revert = await changeset(request, row.reverted_by ?? "");
  expect(revert.reverts).toBe(fixture.id);
  expect(revert.decided_by).toBe(world.reviewer.id);
  await page.goto(reviewUrl(`/-/changesets/${row.reverted_by ?? ""}`));
  await expect(page.getByTestId("rationale")).toContainText(`Undone ${run}`);
  // A second try is refused by what the screen shows: it says it's been reverted, and offers no button.
  await page.goto(reviewUrl(`/-/changesets/${fixture.id}`));
  await expect(page.getByText("This has been reverted.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Revert", exact: true })).toBeHidden();
  expect(problems).toEqual([]);
  await context.close();
});

test("item 4: what always needs a person shows no automatic path, even with its combination earned", async ({ browser, request }) => {
  await clearQueue(request);
  const made = await send<{ page_id: string }>(request, api("/pages"), { title: `Doomed ${run}`, body: "A page.\n", rationale: "fixture", idempotency_key: `doomed-${run}` });
  const del = await send<{ changeset: Proposed }>(request, api(`/pages/${made.page_id}/delete`), { base_version: 1, rationale: "agent wants this gone", idempotency_key: `del-${run}` }, 200, world.agent.token);
  expect(del.changeset.state).toBe("pending");
  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.goto(reviewUrl(`/${del.changeset.id}`));
  await onItem(page, del.changeset.id);
  const path = page.getByTestId("automatic-path");
  await expect(path).toHaveAttribute("data-path", "never");
  await expect(path).toContainText("Only a person can approve this.");
  await expect(path).toContainText("no agent can approve it");
  const detail = await get<{ needs_a_person: boolean }>(request, api(`/review/items/${del.changeset.id}`));
  expect(detail.needs_a_person).toBe(true);

  // A real claim below trusted says so, in the server's words, and that holds with the combination on.
  await clearQueue(request);
  const below = await proposeClaim(request);
  await page.goto(reviewUrl(`/${below.id}`));
  await onItem(page, below.id);
  await expect(path).toHaveAttribute("data-path", "never");
  await expect(path).toContainText("it cites a source below trusted");
  await expect(path).toContainText("Only sources a person marked trusted can auto-commit");
  await context.close();
});

const QUAY = "The harbour master wrote that the quay opened in 1711.";

/** An agent of its own, so nothing else counts towards its combination, whose one approved claim earns auto-commit at this threshold. Rejecting what it left waiting would count against it. */
const earner = async (request: APIRequestContext, name: string) => {
  const principal = await send<{ id: string }>(request, "/api/v1/principals", { name: `${name}-${run}`, kind: "agent" }, 201);
  const made = await send<{ token: string }>(request, "/api/v1/tokens", { name, principal_id: principal.id, grants: [{ vault_id: world.vault, scope: "propose" }] }, 201);
  const first = await propose(request, [claimOp(`${name}-1-${run}`, world.trustedSegment, "1711", QUAY)], "earning", made.token);
  expect(first.state).toBe("pending");
  await send(request, api(`/changesets/${first.id}/approve`), { reason: "one sample, at a threshold of 0.2" }, 200, world.reviewer.token);
  return { id: principal.id, token: made.token };
};

test("item 4: only a source a person marked trusted auto-commits once earned", async ({ browser, request }) => {
  await clearQueue(request);
  const agent = await earner(request, "earner");
  // Now earned: the same combination on a trusted source commits by itself, and on an unverified one waits.
  const trusted = await propose(request, [claimOp(`earn-2-${run}`, world.trustedSegment, "1711 again", QUAY)], "earned", agent.token);
  expect(trusted.state).toBe("committed");
  expect((await changeset(request, trusted.id)).decided_by).toBe("auto-commit");
  const unverified = await propose(request, [claimOp(`earn-3-${run}`, world.segment, "1605", SENTENCE(5))], "earned but unverified", agent.token);
  expect(unverified.state, "an earned combination still waits on a source below trusted").toBe("pending");

  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.goto(reviewUrl(`/${unverified.id}`));
  await onItem(page, unverified.id);
  await expect(page.getByTestId("automatic-path")).toHaveAttribute("data-path", "never");
  await expect(page.getByTestId("automatic-path")).toContainText("however much its combination has earned");
  await page.goto(reviewUrl(`/-/changesets/${trusted.id}`));
  await expect(page.getByTestId("decided-by")).toHaveText("auto-commit");
  await context.close();
});

test("item 4: a chat's proposal shows no automatic path and no samples", async ({ browser, request }) => {
  // The server only makes these from a chat, which needs a model, so this one answer is rewritten in the browser. Spec 049's tests show the server's side.
  await clearQueue(request);
  const one = await proposeClaim(request);
  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.route(`**/review/items/${one.id}`, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as { changeset: Record<string, unknown>; needs_a_person: boolean; auto_commit_block: string };
    body.changeset.requested_by = "a-person";
    body.changeset.proposer = "chat-agent";
    body.needs_a_person = true;
    body.auto_commit_block = "it came from a chat, and a person approves those";
    await route.fulfill({ response, json: body });
  });
  await page.goto(reviewUrl(`/${one.id}`));
  await onItem(page, one.id);
  const path = page.getByTestId("automatic-path");
  await expect(path).toHaveAttribute("data-path", "never");
  await expect(path).toContainText("A chat's proposals never auto-commit, and deciding one adds no earned samples");
  await context.close();
});

test("item 4: an agent can't decide through a session, whatever its scope, and its token keeps what spec 025 recorded", async ({ browser, request }) => {
  await clearQueue(request);
  const one = await proposeClaim(request);
  const committed = await proposeClaim(request);
  await send(request, api(`/changesets/${committed.id}/approve`), { reason: "fixture" });

  const { context, page } = await newSession(browser, world.adminAgent.token);
  await page.goto(reviewUrl());
  await onItem(page, one.id);
  // The screen offers nothing, and a key is refused with a reason.
  await expect(page.getByTestId("no-decisions")).toContainText("agents can't approve");
  await expect(page.getByRole("group", { name: "Decide this item" })).toBeHidden();
  await page.keyboard.press("a");
  await expect(notice(page)).toContainText("You can read this queue, but not decide in it.");
  await page.keyboard.press("r");
  await expect(page.getByTestId("reason-form")).toBeHidden();

  // The server refuses it as well, on each route, through the session this page holds.
  const headers = { "x-memex-csrf": "1" };
  const attempts: readonly (readonly [string, unknown])[] = [
    [`/changesets/${one.id}/approve`, { reason: "r" }],
    [`/changesets/${one.id}/reject`, { reason: "r" }],
    [`/review/items/${one.id}/edit-approve`, { ops: [], reason: "r" }],
    ["/review/batch", { decisions: [{ id: one.id, decision: "approve" }] }],
    [`/changesets/${committed.id}/revert`, { reason: "r" }],
  ];
  for (const [path, data] of attempts) {
    const res = await page.request.post(api(path), { headers, data });
    expect(res.status(), path).toBe(403);
    expect(((await res.json()) as { error: { code: string } }).error.code, path).toBe("agent_session");
  }
  expect((await changeset(request, one.id)).state, "none of that decided anything").toBe("pending");
  expect((await changeset(request, committed.id)).reverted_by).toBeNull();
  await page.goto(reviewUrl(`/-/changesets/${committed.id}`));
  await expect(page.getByTestId("inverse")).toBeVisible();
  await expect(page.getByRole("button", { name: "Revert", exact: true })).toBeHidden();
  await context.close();

  // A lower scope in a session has a different answer: it's not allowed at all.
  const low = await newSession(browser, world.agent.token);
  const refused = await low.page.request.post(api(`/changesets/${one.id}/approve`), { headers, data: { reason: "r" } });
  expect(refused.status()).toBe(403);
  await low.context.close();
  // A person with read scope reads, and is told why they can't decide.
  const reader = await newSession(browser, world.reader);
  await reader.page.goto(reviewUrl());
  await onItem(reader.page, one.id);
  await expect(reader.page.getByTestId("no-decisions")).toContainText("Deciding needs admin scope");
  await reader.context.close();

  // The remaining gap, on record: the same admin agent with its token, not a session, may approve an ordinary claim.
  const approved = await send<{ state: string; decided_by: string }>(request, api(`/changesets/${one.id}/approve`), { reason: "as a token" }, 200, world.adminAgent.token);
  expect(approved.state).toBe("committed");
  expect(approved.decided_by).toBe(world.adminAgent.id);
});

test("item 5: two reviewers on one item: the second gets a clear conflict, and there's one decision", async ({ browser, request }) => {
  await clearQueue(request);
  const one = await proposeClaim(request);
  const two = await proposeClaim(request);
  const first = await newSession(browser, world.reviewer.token);
  const second = await newSession(browser, world.second.token);
  await first.page.goto(reviewUrl());
  await second.page.goto(reviewUrl());
  await onItem(first.page, one.id);
  await onItem(second.page, one.id);
  // Stop the second reviewer's page from hearing about the first's decision, so it acts on what it has on screen, as it would if both pressed at once.
  await second.page.route("**/activity*", (route) => route.abort());
  await second.page.route("**/review/queue*", (route) => route.abort());

  await first.page.keyboard.press("a");
  await expect(first.page.getByTestId("queue-notice")).toContainText("Approved.");
  const decided = await changeset(request, one.id);
  expect(decided.decided_by).toBe(world.reviewer.id);

  await second.page.keyboard.press("r");
  await second.page.keyboard.insertText("I disagree");
  await second.page.keyboard.press("Enter");
  const conflict = second.page.getByTestId("queue-notice");
  await expect(conflict).toHaveAttribute("data-tone", "danger");
  await expect(conflict).toContainText("This couldn't be done, and nothing changed.");
  await expect(conflict).toContainText(world.reviewer.id);
  console.log(`item 5: the second reviewer read: ${await conflict.innerText()}`);

  const after = await changeset(request, one.id);
  expect(after).toEqual(decided);
  expect(after.decision_reason).not.toBe("I disagree");
  // The same person pressing again is the same decision, with nothing new.
  await send(request, api(`/changesets/${one.id}/approve`), { reason: "again" }, 200, world.reviewer.token);
  expect((await changeset(request, one.id)).decided_at_ms).toBe(decided.decided_at_ms);
  expect((await changeset(request, two.id)).state).toBe("pending");
  await first.context.close();
  await second.context.close();
});

test("item 6: hostile proposal text and rationale render as plain text on every screen", async ({ browser, request }) => {
  await clearQueue(request);
  const hostileClaim = `hostile-${run}`;
  const cs = await propose(request, [claimOp(hostileClaim, world.segment, HOSTILE, SENTENCE(3), HOSTILE)], HOSTILE);
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  const external: string[] = [];
  page.on("request", (r) => {
    if (new URL(r.url()).origin !== new URL(baseURL()).origin) external.push(r.url());
  });
  const check = async () => {
    expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
    expect(await page.locator("main img[src='x'], main script").count()).toBe(0);
    expect(await page.locator(`main b:has-text("bold-${run}")`).count()).toBe(0);
  };
  // A model's words, which the server only gets from a reviewer agent, are rewritten into this one answer to reach their place on the screen.
  await page.route(`**/review/items/${cs.id}`, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as { verdict?: unknown; changeset: { validators: unknown[] } };
    body.verdict = { verdict: "approve", reason: HOSTILE, model: HOSTILE, prompt_hash: "h", tokens: { input_tokens: 0, output_tokens: 0, thinking_tokens: 0 }, attempts: 1, created_at_ms: 0 };
    body.changeset.validators.push({ validator: HOSTILE, passed: false, detail: HOSTILE });
    await route.fulfill({ response, json: body });
  });
  await page.goto(reviewUrl(`/${cs.id}`));
  await onItem(page, cs.id);
  await expect(page.getByTestId("rationale")).toHaveText(HOSTILE);
  await expect(page.getByTestId("op").first()).toContainText(HOSTILE);
  await expect(page.getByTestId("verdict")).toContainText(HOSTILE);
  await expect(page.getByTestId("validators")).toContainText(HOSTILE);
  await expect(page.locator(`[data-queue-row="${cs.id}"]`)).toContainText("Add a claim");
  await check();
  await page.keyboard.press("e");
  await expect(page.getByLabel("The ops, as JSON")).toHaveValue(/onerror/);
  await check();
  await page.keyboard.press("Escape");

  await page.goto(reviewUrl("/-/changesets?state=pending"));
  await expect(page.getByTestId("changeset-table")).toBeVisible();
  await check();
  await page.goto(reviewUrl(`/-/changesets/${cs.id}`));
  await expect(page.getByTestId("rationale")).toHaveText(HOSTILE);
  await check();
  // A hostile claim that has committed, on the screen that reads claims back.
  await send(request, api(`/changesets/${cs.id}/approve`), { reason: HOSTILE });
  await page.goto(reviewUrl(`/-/changesets/${cs.id}`));
  await expect(page.getByTestId("decision-reason")).toHaveText(HOSTILE);
  await check();
  expect(external, "nothing went to another origin").toEqual([]);
  expect(problems).toEqual([]);
  await context.close();
});

test("the claim sits next to its highlighted source, on a PDF and in plain text", async ({ browser, request }) => {
  await clearQueue(request);
  const pdfClaim = await propose(request, [claimOp(`pdf-${run}`, world.pdfSegment, "page one", "of the fleet")]);
  const textClaim = await proposeClaim(request);
  const detail = await get<{ evidence: { rects: { page: number }[] }[] }>(request, api(`/review/items/${pdfClaim.id}`));
  expect(detail.evidence[0]?.rects.length, "the API gives a pending quote its place").toBeGreaterThan(0);
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  await page.goto(reviewUrl());
  await onItem(page, pdfClaim.id);
  await expect(page.getByTestId("source-name")).toContainText(`pdf-${run}.pdf`);
  await expect(page.getByTestId("evidence-quote")).toHaveText("of the fleet");
  await expect(page.locator("[data-highlight]").first()).toBeAttached();
  await expect(page.locator("[data-page-image], img").first()).toBeVisible();
  await page.keyboard.press("j");
  await onItem(page, textClaim.id);
  await expect(page.getByTestId("quote-mark")).toContainText("was built in Stockholm");
  await expect(page.getByTestId("source-name")).toContainText(`fleet-${run}.txt`);
  // o opens it in spec 042's viewer.
  await page.keyboard.press("o");
  await expect(page).toHaveURL(/\/documents\//);
  expect(problems).toEqual([]);

  const small = await newSession(browser, world.reviewer.token, { width: 1024, height: 768 });
  await small.page.goto(reviewUrl());
  await onItem(small.page, pdfClaim.id);
  await expect(small.page.getByTestId("source-pane")).toBeVisible();
  const overflow = await small.page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow, "no sideways scrolling at the smallest width").toBeLessThanOrEqual(0);
  await small.context.close();
  await context.close();
});

test("the counts follow the activity stream with no reload, and a link from the wiki lands on its item", async ({ browser, request }) => {
  await clearQueue(request);
  const { context, page } = await newSession(browser, world.reviewer.token);
  await page.goto(reviewUrl());
  await expect(page.getByTestId("live-state")).toContainText("Live");
  await expect(page.getByTestId("pending-count")).toHaveText("0");
  const navigations: string[] = [];
  page.on("framenavigated", (f) => navigations.push(f.url()));
  const started = Date.now();
  const one = await proposeClaim(request);
  await expect(page.getByTestId("pending-count")).toHaveText("1");
  await expect(page.locator(`[data-queue-row="${one.id}"]`)).toBeVisible();
  console.log(`live: a new proposal reached the count in ${String(Date.now() - started)} ms`);
  const two = await proposeClaim(request);
  await expect(page.getByTestId("pending-count")).toHaveText("2");
  await send(request, api(`/changesets/${two.id}/reject`), { reason: "someone else, elsewhere" }, 200, world.second.token);
  await expect(page.getByTestId("pending-count")).toHaveText("1");
  await expect(page.locator(`[data-queue-row="${two.id}"]`)).toBeHidden();
  expect(navigations.filter((u) => !u.includes("/review")), "it never left the screen").toEqual([]);

  // A filter narrows the list, and is in the URL, so it can be linked to and reloaded.
  await page.getByLabel("Proposer").fill("nobody");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(page).toHaveURL(/proposer=nobody/);
  await expect(page.getByText("Nothing waiting matches these filters.")).toBeVisible();
  await page.reload();
  await expect(page.getByText("Nothing waiting matches these filters.")).toBeVisible();
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect(page.locator(`[data-queue-row="${one.id}"]`)).toBeVisible();

  // Spec 044's notice links here, for a delete or an edit waiting. The placeholder is gone, and an item that's been decided still opens, saying who decided.
  await page.goto(reviewUrl(`/${two.id}`));
  await expect(item(page, two.id)).toBeVisible();
  await expect(page.getByTestId("decided-by")).toHaveText(world.second.id);
  await expect(page.locator("main")).not.toContainText("isn't built yet");
  await context.close();
});

test("the auto-commit table shows precision, its bound, its state and history, and a person can turn one off", async ({ browser, request }) => {
  const agent = await earner(request, "table-earner");
  const table = await get<{ threshold: number; items: { id: string; extractor: string; on: boolean; wilson_bound: number; precision: number | null }[] }>(request, api("/review/auto-commit?limit=200"));
  const on = table.items.find((e) => e.on && e.extractor === agent.id);
  expect(on, "the combination this test earned").toBeDefined();
  const id = on?.id ?? "";
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  await page.goto(reviewUrl("/-/auto-commit"));
  const rules = page.getByTestId("auto-commit-rules");
  await expect(rules).toContainText("Only sources a person marked trusted can auto-commit");
  await expect(rules).toContainText("A chat's proposals never auto-commit, and deciding one adds no earned samples");
  await expect(rules).toContainText("One failed audit turns off auto-commit for that combination");
  await expect(page.getByTestId("threshold")).toHaveText("20.0%");
  const row = page.locator(`tr[data-combination="${id}"]`);
  const explain = page.locator(`tr[data-explain="${id}"]`);
  await expect(row.getByTestId("combination-state")).toHaveText("On");
  await expect(row.getByTestId("wilson")).toHaveText(`${((on?.wilson_bound ?? 0) * 100).toFixed(1)}%`);
  await expect(row.getByTestId("precision")).toHaveText(`${((on?.precision ?? 0) * 100).toFixed(1)}%`);
  await expect(explain.getByTestId("state-text")).toContainText("only when every source they cite is one a person marked trusted");
  await explain.getByTestId("history").locator("summary").click();
  await expect(explain.getByTestId("history")).toContainText("Earned");

  await row.getByRole("button", { name: /Turn off/ }).click();
  await expect(explain).toContainText("There's no way to turn it on by hand");
  await explain.getByLabel("Why this is turned off").fill(`Bad run ${run}`);
  await explain.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(row.getByTestId("combination-state")).toHaveText("Off");
  await expect(explain.getByTestId("state-text")).toContainText(`${world.reviewer.id} turned it off`);
  await explain.getByTestId("history").locator("summary").click();
  await expect(explain.getByTestId("history")).toContainText("Turned off");
  await expect(explain.getByTestId("history")).toContainText(`Bad run ${run}`);
  const after = await get<{ items: { id: string; on: boolean; correct: number; total_correct: number }[] }>(request, api("/review/auto-commit?limit=200"));
  const row2 = after.items.find((e) => e.id === id);
  expect(row2).toMatchObject({ on: false, correct: 0 });
  expect(row2?.total_correct).toBeGreaterThan(0);

  // The server only turns one off for a failed audit given a model or a damaged source, so this one answer is rewritten. Spec 026's tests show the server's side, and this shows the words.
  await page.route("**/review/auto-commit*", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as { items: { id: string; history: { by: string; kind: string }[]; correct: number; incorrect: number }[] };
    for (const e of body.items) {
      if (e.id !== id) continue;
      e.correct = 0;
      e.incorrect = 0;
      e.history = [...e.history, { at_ms: 1, by: "audit-agent", kind: "disabled", reason: "audit run r, claim c", correct: 80, incorrect: 1 } as never];
    }
    await route.fulfill({ response, json: body });
  });
  await page.reload();
  await expect(explain.getByTestId("state-text")).toContainText("because an audit failed one of its claims");
  await expect(explain.getByTestId("state-text")).toContainText("One failed audit turns off auto-commit");
  await explain.getByTestId("history").locator("summary").click();
  await expect(explain.getByTestId("history")).toContainText("by an audit");
  expect(problems).toEqual([]);
  await context.close();
});

test("the contradictions list shows both claims and their sources", async ({ browser, request }) => {
  await clearQueue(request);
  const subject = `disputed quay ${run}`;
  const one = `quay-one-${run}`;
  const two = `quay-two-${run}`;
  await send(request, api("/proposals"), { ops: [claimOp(one, world.segment, "1601", SENTENCE(1), subject)], rationale: "first", idempotency_key: `q1-${run}` });
  const second = await send<{ contradictions: unknown[] }>(request, api("/proposals"), { ops: [claimOp(two, world.segment, "1602", SENTENCE(2), subject)], rationale: "second", idempotency_key: `q2-${run}` });
  expect(second.contradictions.length, "the server found the contradiction").toBeGreaterThan(0);
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  await page.goto(reviewUrl("/-/contradictions"));
  const pair = page.getByTestId("contradiction").filter({ hasText: subject });
  await expect(pair.locator(`[data-claim="${one}"]`)).toContainText("1601");
  await expect(pair.locator(`[data-claim="${two}"]`)).toContainText("1602");
  await expect(pair.locator(`[data-claim="${one}"]`)).toContainText(SENTENCE(1));
  await expect(pair.locator(`[data-claim="${two}"]`)).toContainText(`fleet-${run}.txt`);
  await expect(pair.locator(`[data-claim="${two}"]`).getByText("Unverified")).toBeVisible();
  await pair.locator(`[data-claim="${two}"]`).getByRole("link", { name: `fleet-${run}.txt` }).click();
  await expect(page).toHaveURL(/\/documents\/.*claim=/);
  expect(problems).toEqual([]);
  await context.close();
});

test("axe finds no violations on any tab, and no modal or CSP error shows up", async ({ browser, request }) => {
  await clearQueue(request);
  const one = await proposeClaim(request);
  await send(request, api(`/changesets/${one.id}/approve`), { reason: "fixture" });
  const waiting = await proposeClaim(request);
  const { context, page } = await newSession(browser, world.reviewer.token);
  const problems = problemsOf(page);
  const scan = async (url: string, ready: () => Promise<void>) => {
    await page.goto(url);
    await ready();
    const { violations } = await new AxeBuilder({ page }).analyze();
    expect(violations.map((v) => `${url}: ${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
  };
  await scan(reviewUrl(), async () => {
    await onItem(page, waiting.id);
  });
  await page.keyboard.press("r");
  await expect(page.getByTestId("reason-form")).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations.map((v) => v.id)).toEqual([]);
  await scan(reviewUrl("/-/changesets"), () => expect(page.getByTestId("changeset-table")).toBeVisible());
  await scan(reviewUrl(`/-/changesets/${one.id}`), () => expect(page.getByTestId("inverse")).toBeVisible());
  await scan(reviewUrl("/-/contradictions"), () => expect(page.getByRole("heading", { name: "Review", level: 1 })).toBeVisible());
  await scan(reviewUrl("/-/auto-commit"), () => expect(page.getByTestId("auto-commit-rules")).toBeVisible());
  expect(problems).toEqual([]);
  await context.close();
});
