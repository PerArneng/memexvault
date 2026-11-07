import { defineConfig } from "@playwright/test";

// Spec 040: runs against a real `memexvault serve`, which the tests start and stop themselves. See AGENTS.md.
const bind = process.env.MEMEX_HTTP_BIND;
if (!bind || !process.env.MEMEX_E2E_TOKEN || !process.env.MEMEX_E2E_BIN) throw new Error("set MEMEX_E2E_BIN, MEMEX_E2E_TOKEN and MEMEX_HTTP_BIND, as well as what `memexvault serve` needs");

export default defineConfig({
  testDir: "e2e-shell",
  fullyParallel: false,
  workers: 1,
  timeout: 120_000,
  use: { baseURL: `http://${bind}`, colorScheme: "dark" },
});
