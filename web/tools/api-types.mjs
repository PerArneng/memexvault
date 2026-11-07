// Spec 040: the API types come from the server's OpenAPI document, and this keeps them from drifting.
//   gen                  writes packages/api/openapi.json and src/generated/schema.ts from the current document.
//   check                fails if the committed document differs from the current one, or the committed types from the document.
//   check --offline      compares only the committed types with the committed document, which needs no server.
// The current document comes from --from <file>, or else MEMEX_OPENAPI_URL, such as http://127.0.0.1:8080/api/v1/openapi.json.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import openapiTS, { astToString } from "openapi-typescript";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const docPath = join(root, "packages/api/openapi.json");
const typesPath = join(root, "packages/api/src/generated/schema.ts");
const HEADER = "// Generated from packages/api/openapi.json by `pnpm api:gen`. Don't edit it.\n";

const [command, ...args] = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args[i + 1];
};

const die = (message) => {
  console.error(message);
  process.exit(1);
};

/** Key order carries no meaning, and sorted keys give a readable diff in review. */
const canonical = (value) => {
  if (Array.isArray(value)) return value.map(canonical);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map((k) => [k, canonical(value[k])]));
};
const text = (doc) => `${JSON.stringify(canonical(doc), null, 2)}\n`;

const current = async () => {
  const file = flag("--from");
  if (file !== undefined) return JSON.parse(readFileSync(file, "utf8"));
  const url = process.env.MEMEX_OPENAPI_URL;
  if (url === undefined) die("No current document to compare: set MEMEX_OPENAPI_URL or pass --from <file>. With --offline, only the committed types are compared with the committed document.");
  const res = await fetch(url).catch((e) => die(`Can't fetch ${url}: ${e.message}`));
  if (!res.ok) die(`Can't fetch ${url}: ${res.status}`);
  return res.json();
};

/**
 * The generator refuses a document with a repeated operationId, and the server's has one: `redirect`, on pages and on entities.
 * Clients address operations by path, so the ids are left out of the generator's input only. The committed document keeps them.
 */
const withoutOperationIds = (doc) => {
  const paths = Object.fromEntries(
    Object.entries(doc.paths).map(([path, item]) => [
      path,
      Object.fromEntries(Object.entries(item).map(([method, op]) => {
        const { operationId: _dropped, ...rest } = op;
        return [method, rest];
      })),
    ]),
  );
  return { ...doc, paths };
};

const typesFor = async (doc) => HEADER + astToString(await openapiTS(withoutOperationIds(canonical(doc))));

/** The first line that differs, which is enough to see what changed. */
const firstDifference = (a, b) => {
  const [x, y] = [a.split("\n"), b.split("\n")];
  const i = x.findIndex((line, n) => line !== y[n]);
  const at = i === -1 ? x.length : i;
  return `line ${at + 1}:\n  committed: ${x[at] ?? "(end)"}\n  current:   ${y[at] ?? "(end)"}`;
};

if (command === "gen") {
  const doc = await current();
  writeFileSync(docPath, text(doc));
  mkdirSync(dirname(typesPath), { recursive: true });
  writeFileSync(typesPath, await typesFor(doc));
  console.log(`wrote ${docPath} and ${typesPath}`);
} else if (command === "check") {
  const committed = readFileSync(docPath, "utf8");
  if (!args.includes("--offline")) {
    const now = text(await current());
    if (now !== committed) die(`The committed OpenAPI document differs from the server's, at ${firstDifference(committed, now)}\nRun \`pnpm api:gen\`, then \`pnpm typecheck\`.`);
  }
  const expected = await typesFor(JSON.parse(committed));
  const types = readFileSync(typesPath, "utf8");
  if (expected !== types) die(`The committed API types differ from packages/api/openapi.json, at ${firstDifference(types, expected)}\nRun \`pnpm api:gen\`.`);
  console.log(args.includes("--offline") ? "API types match the committed document. The server's document wasn't compared." : "API types and document match the server's document.");
} else {
  die("usage: api-types.mjs gen | check [--offline] [--from <file>]");
}
