// Spec 036: every declared text and surface pair must meet WCAG AA. Usage: check-contrast.mjs [tokens.css].
import { readFileSync } from "node:fs";

const css = readFileSync(process.argv[2] ?? "packages/ui/src/tokens.css", "utf8");
const tokens = Object.fromEntries([...css.matchAll(/--color-([a-z-]+):\s*(#[0-9a-f]{6});/g)].map((m) => [m[1], m[2]]));
const lum = (hex) => {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
};
const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};
const surfaces = ["surface", "surface-raised", "surface-overlay", "surface-selected"];
const pairs = [
  ...["text", "text-muted", "text-faint", "accent", "accent-hover", "success", "warning", "danger", "info"].flatMap((t) => surfaces.map((s) => [t, s, 4.5])),
  ["on-accent", "accent", 4.5], ["on-accent", "accent-hover", 4.5], ["on-danger", "danger-solid", 4.5],
  ["success", "success-surface", 4.5], ["warning", "warning-surface", 4.5], ["danger", "danger-surface", 4.5], ["info", "info-surface", 4.5], ["text", "neutral-surface", 4.5], ["text-muted", "neutral-surface", 4.5],
  // Large text, icons and control borders need 3:1.
  ...surfaces.flatMap((s) => [["border-strong", s, 3], ["focus", s, 3]]),
];
let failed = 0;
for (const [fg, bg, need] of pairs) {
  if (!tokens[fg] || !tokens[bg]) { console.error(`missing token ${fg} or ${bg}`); failed++; continue; }
  const r = ratio(tokens[fg], tokens[bg]);
  if (r < need) { console.error(`FAIL ${fg} on ${bg}: ${r.toFixed(2)} < ${need}`); failed++; }
}
const lowest = Math.min(...pairs.filter(([f, b]) => tokens[f] && tokens[b]).map(([f, b, n]) => ratio(tokens[f], tokens[b]) / n));
console.log(`${pairs.length} pairs, ${failed} failed, tightest margin ${lowest.toFixed(2)}x of its requirement`);
process.exit(failed ? 1 : 0);
