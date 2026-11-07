// Spec 040, verification 6: the JavaScript the sign-in route loads first is at most 200 kB compressed.
// Counts what index.html names, which is everything that loads before the page can run, and fails past the limit.
// Usage: check-bundle.mjs [dist folder] [limit in bytes].
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { brotliCompressSync, gzipSync } from "node:zlib";

const dist = process.argv[2] ?? "apps/console/dist";
const limit = Number(process.argv[3] ?? 200_000);
const html = readFileSync(join(dist, "index.html"), "utf8");
const files = [...html.matchAll(/<(?:script[^>]*src|link[^>]*rel="modulepreload"[^>]*href)="\/([^"]+\.js)"/g)].map((m) => m[1]);
if (files.length === 0) {
  console.error(`no scripts named in ${dist}/index.html`);
  process.exit(1);
}
let raw = 0;
let gzip = 0;
let brotli = 0;
for (const file of new Set(files)) {
  const bytes = readFileSync(join(dist, file));
  raw += bytes.length;
  gzip += gzipSync(bytes, { level: 9 }).length;
  brotli += brotliCompressSync(bytes).length;
  console.log(`${file}: ${bytes.length} bytes`);
}
console.log(`initial JavaScript: ${raw} bytes, ${gzip} gzip, ${brotli} brotli. The limit is ${limit} gzip.`);
process.exit(gzip > limit ? 1 : 0);
