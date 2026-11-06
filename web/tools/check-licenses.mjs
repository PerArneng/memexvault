// Fails on any dependency under a denied licence. Usage: check-licenses.mjs [fixture.json]. Without a fixture it asks pnpm.
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

const denied = /\b(A?GPL)\b/i;
const raw = process.argv[2] ? readFileSync(process.argv[2], "utf8") : execSync("pnpm licenses list --json", { encoding: "utf8", maxBuffer: 1 << 28 });
const byLicense = JSON.parse(raw);
const bad = Object.entries(byLicense).filter(([licence]) => denied.test(licence));
const count = Object.values(byLicense).reduce((n, list) => n + list.length, 0);
for (const [licence, list] of bad) console.error(`denied: ${licence}: ${list.map((p) => p.name).join(", ")}`);
console.log(`${count} packages, licences: ${Object.keys(byLicense).sort().join(", ")}`);
process.exit(bad.length ? 1 : 0);
