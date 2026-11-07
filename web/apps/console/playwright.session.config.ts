import { defineConfig } from "@playwright/test";

// Spec 037: runs against a real `memexvault serve`, which the caller starts. See AGENTS.md.
const baseURL = process.env.MEMEX_E2E_URL;
if (!baseURL || !process.env.MEMEX_E2E_TOKEN) throw new Error("set MEMEX_E2E_URL and MEMEX_E2E_TOKEN");

export default defineConfig({
  testDir: "e2e-session",
  fullyParallel: false,
  workers: 1,
  use: { baseURL, colorScheme: "dark" },
});
