import { defineConfig } from "@playwright/test";

// Spec 042: runs against a real `memexvault serve`, which the tests start and stop themselves, as `e2e:shell` does. See AGENTS.md.
const bind = process.env.MEMEX_HTTP_BIND;
if (!bind || !process.env.MEMEX_E2E_TOKEN || !process.env.MEMEX_E2E_BIN) throw new Error("set MEMEX_E2E_BIN, MEMEX_E2E_TOKEN and MEMEX_HTTP_BIND, as well as what `memexvault serve` needs");

export default defineConfig({
  testDir: "e2e-documents",
  fullyParallel: false,
  workers: 1,
  timeout: 300_000,
  // A generous wait, as these often run on a loaded machine, where the server can stall for many seconds.
  expect: { timeout: 60_000 },
  use: { baseURL: `http://${bind}`, colorScheme: "dark", viewport: { width: 1400, height: 1200 }, deviceScaleFactor: 1 },
});
