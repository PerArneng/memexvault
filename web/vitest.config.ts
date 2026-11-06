import { defineConfig } from "vitest/config";

// Pure packages run in plain Node: no DOM, no sleeping.
export default defineConfig({
  test: { include: ["packages/*/src/**/*.test.ts"], environment: "node" },
});
