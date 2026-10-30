// Every package the lockfile installs (dev included) must carry a permissive licence.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const ALLOWED = new Set([
  "MIT", "MIT-0", "ISC", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "0BSD", "Zlib",
  "BlueOak-1.0.0", "CC0-1.0", "Unlicense", "Python-2.0", "CC-BY-4.0",
]);
// an SPDX expression is fine when every alternative of an OR, or every term of an AND, is
const ok = (expr) => {
  const e = expr.replace(/[()]/g, "").trim();
  if (e.includes(" OR ")) return e.split(" OR ").some((x) => ok(x));
  if (e.includes(" AND ")) return e.split(" AND ").every((x) => ok(x));
  return ALLOWED.has(e);
};
const lock = JSON.parse(readFileSync(fileURLToPath(new URL("../package-lock.json", import.meta.url)), "utf8"));
const bad = [];
let n = 0;
for (const [path, p] of Object.entries(lock.packages ?? {})) {
  if (path === "") continue;
  n++;
  const lic = typeof p.license === "string" ? p.license : "";
  if (!lic || !ok(lic)) bad.push(`${path.replace(/^.*node_modules\//, "")}@${p.version}: ${lic || "(none)"}`);
}
console.log(`${n} packages checked`);
if (bad.length) {
  console.error(`not permissive:\n  ${bad.join("\n  ")}`);
  process.exit(1);
}
