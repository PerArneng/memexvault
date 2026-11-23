// Spec 051, verification 1: every OpenAPI operation must be mapped in web/ui-coverage.json to a screen or marked no-UI with a reason.
// Usage: check-coverage.mjs [--openapi <file>] [--coverage <file>]
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args[i + 1];
};

const openapiPath = flag("--openapi") ?? join(root, "packages/api/openapi.json");
const coveragePath = flag("--coverage") ?? join(root, "ui-coverage.json");

const METHODS = new Set(["get", "post", "put", "patch", "delete"]);

export const checkCoverage = (openapi, coverage) => {
  const errors = [];
  const docOps = new Set();
  for (const [path, item] of Object.entries(openapi.paths ?? {})) {
    if (!item || typeof item !== "object") continue;
    for (const method of Object.keys(item)) {
      if (METHODS.has(method.toLowerCase())) {
        docOps.add(`${method.toUpperCase()} ${path}`);
      }
    }
  }

  const mapped = coverage?.operations;
  if (!mapped || typeof mapped !== "object" || Array.isArray(mapped)) {
    return { errors: ["ui-coverage.json must have an 'operations' object"], total: docOps.size, uiCount: 0, noUiCount: 0 };
  }

  const mappedKeys = new Set(Object.keys(mapped));
  for (const op of [...docOps].sort()) {
    if (!mappedKeys.has(op)) {
      errors.push(`Unmapped OpenAPI operation: ${op}`);
    }
  }
  for (const key of [...mappedKeys].sort()) {
    if (!docOps.has(key)) {
      errors.push(`Stale ui-coverage.json entry (not in OpenAPI): ${key}`);
    }
  }

  let uiCount = 0;
  let noUiCount = 0;
  for (const [key, entry] of Object.entries(mapped)) {
    if (!entry || typeof entry !== "object" || typeof entry.ui !== "boolean") {
      errors.push(`Invalid entry for ${key}: must have boolean 'ui'`);
      continue;
    }
    if (entry.ui) {
      uiCount += 1;
      if (typeof entry.screen !== "string" || entry.screen.trim() === "") {
        errors.push(`Invalid UI entry for ${key}: 'screen' must be a non-empty string`);
      }
    } else {
      noUiCount += 1;
      if (typeof entry.reason !== "string" || entry.reason.trim() === "") {
        errors.push(`Invalid no-UI entry for ${key}: 'reason' must be a non-empty string`);
      }
    }
  }

  return { errors, total: docOps.size, uiCount, noUiCount };
};

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const openapi = JSON.parse(readFileSync(openapiPath, "utf8"));
  const coverage = JSON.parse(readFileSync(coveragePath, "utf8"));
  const result = checkCoverage(openapi, coverage);
  if (result.errors.length > 0) {
    for (const err of result.errors) console.error(err);
    console.error(`UI coverage check failed: ${result.errors.length} problem(s) across ${result.total} OpenAPI operations.`);
    process.exit(1);
  }
  console.log(`UI coverage OK: ${result.total} OpenAPI operations mapped (${result.uiCount} UI, ${result.noUiCount} no-UI with reasons).`);
}
