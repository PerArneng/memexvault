// Spec 040 (verification 6) and Spec 051 (verification 5):
// - Sign-in route initial JavaScript is at most 200 kB gzip.
// - Every console route's initial JavaScript (entry + static closure of its lazy chunk) stays at or under its recorded gzip budget.
// Usage: check-bundle.mjs [dist folder] [login limit in bytes].
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { brotliCompressSync, gzipSync } from "node:zlib";

const dist = process.argv[2] ?? "apps/console/dist";
const loginLimit = Number(process.argv[3] ?? 200_000);

// Recorded gzip budgets (bytes) per route's initial JavaScript closure.
const ROUTE_BUDGETS = [
  { name: "login", prefixes: [], limit: loginLimit },
  { name: "shell", prefixes: ["shell-"], limit: 240_000 },
  { name: "gallery", prefixes: ["gallery-"], limit: 250_000 },
  { name: "ingest", prefixes: ["shell-", "ingest-screen-"], limit: 260_000 },
  { name: "documents", prefixes: ["shell-", "documents-screen-"], limit: 270_000 },
  { name: "search", prefixes: ["shell-", "search-screen-"], limit: 250_000 },
  { name: "wiki", prefixes: ["shell-", "wiki-screen-"], limit: 320_000 },
  { name: "entities", prefixes: ["shell-", "entities-screen-"], limit: 270_000 },
  { name: "review", prefixes: ["shell-", "review-screen-"], limit: 260_000 },
  { name: "audit", prefixes: ["shell-", "audit-screen-"], limit: 260_000 },
  { name: "chat", prefixes: ["shell-", "chat-panel-"], limit: 310_000 },
  { name: "admin", prefixes: ["shell-", "admin-screen-"], limit: 260_000 },
];

const html = readFileSync(join(dist, "index.html"), "utf8");
const entryFiles = [...html.matchAll(/<(?:script[^>]*src|link[^>]*rel="modulepreload"[^>]*href)="\/([^"]+\.js)"/g)].map((m) => m[1]);
if (entryFiles.length === 0) {
  console.error(`no scripts named in ${dist}/index.html`);
  process.exit(1);
}

const assetsDir = join(dist, "assets");
const allAssets = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
const sizes = new Map();
const deps = new Map();

for (const file of allAssets) {
  const bytes = readFileSync(join(assetsDir, file));
  const text = bytes.toString("utf8");
  sizes.set(file, {
    raw: bytes.length,
    gzip: gzipSync(bytes, { level: 9 }).length,
    brotli: brotliCompressSync(bytes).length,
  });
  const imports = [
    ...[...text.matchAll(/from"\.\/([^"]+\.js)"/g)].map((m) => m[1]),
    ...[...text.matchAll(/import"\.\/([^"]+\.js)"/g)].map((m) => m[1]),
  ];
  deps.set(file, [...new Set(imports)]);
}

const entryBaseNames = [...new Set(entryFiles)].map((f) => f.replace(/^assets\//, ""));

const closureFor = (roots) => {
  const seen = new Set();
  const stack = [...roots];
  while (stack.length > 0) {
    const file = stack.pop();
    if (!file || seen.has(file) || !sizes.has(file)) continue;
    seen.add(file);
    for (const dep of deps.get(file) ?? []) stack.push(dep);
  }
  let raw = 0;
  let gzip = 0;
  let brotli = 0;
  for (const file of seen) {
    const s = sizes.get(file);
    raw += s.raw;
    gzip += s.gzip;
    brotli += s.brotli;
  }
  return { raw, gzip, brotli, chunks: seen.size };
};

let failed = false;
for (const route of ROUTE_BUDGETS) {
  const extras = route.prefixes.flatMap((prefix) => allAssets.filter((f) => f.startsWith(prefix)));
  if (route.prefixes.length > 0 && extras.length < route.prefixes.length) {
    console.error(`route ${route.name}: missing expected chunk(s) for ${route.prefixes.join(", ")}`);
    failed = true;
    continue;
  }
  const totals = closureFor([...entryBaseNames, ...extras]);
  const status = totals.gzip > route.limit ? "FAIL" : "OK";
  if (status === "FAIL") failed = true;
  console.log(
    `${route.name.padEnd(10)} ${status.padEnd(4)} raw=${String(totals.raw).padStart(7)}  gzip=${String(totals.gzip).padStart(7)} / ${String(route.limit).padStart(7)}  brotli=${String(totals.brotli).padStart(7)}  (${totals.chunks} chunks)`,
  );
}

process.exit(failed ? 1 : 0);
