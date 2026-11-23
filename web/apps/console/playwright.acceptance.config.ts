import { defineConfig } from "@playwright/test";

// Spec 051: full console acceptance journey and target measurements against a real `memexvault serve` and Postgres,
// with a local stand-in LLM on 127.0.0.1 (zero external calls).
const bind = process.env.MEMEX_HTTP_BIND;
if (!bind || !process.env.MEMEX_E2E_TOKEN || !process.env.MEMEX_E2E_BIN || !process.env.MEMEX_DATABASE_URL) {
  throw new Error("set MEMEX_E2E_BIN, MEMEX_E2E_TOKEN, MEMEX_HTTP_BIND and MEMEX_DATABASE_URL, plus MEMEX_MODELS_DIR");
}

export default defineConfig({
  testDir: "e2e-acceptance",
  fullyParallel: false,
  workers: 1,
  timeout: 600_000,
  expect: { timeout: 60_000 },
  use: {
    baseURL: `http://${bind}`,
    colorScheme: "dark",
    viewport: { width: 1440, height: 960 },
    deviceScaleFactor: 1,
    actionTimeout: 20_000,
  },
});
