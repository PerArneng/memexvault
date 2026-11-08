import { defineConfig } from "@playwright/test";

// Spec 041: runs against a real `memexvault serve`, which the tests start and stop themselves, through a proxy the tests can cut. See AGENTS.md.
const bind = process.env.MEMEX_HTTP_BIND;
if (!bind || !process.env.MEMEX_E2E_TOKEN || !process.env.MEMEX_E2E_BIN) throw new Error("set MEMEX_E2E_BIN, MEMEX_E2E_TOKEN and MEMEX_HTTP_BIND, as well as what `memexvault serve` needs");
const port = Number(bind.split(":")[1]);

export default defineConfig({
  testDir: "e2e-ingest",
  fullyParallel: false,
  workers: 1,
  timeout: 600_000,
  // A generous wait, as these often run on a loaded machine, where the server can stall for many seconds.
  expect: { timeout: 60_000 },
  use: { baseURL: `http://127.0.0.1:${String(port + 100)}`, colorScheme: "dark", viewport: { width: 1400, height: 1400 }, deviceScaleFactor: 1 },
});
