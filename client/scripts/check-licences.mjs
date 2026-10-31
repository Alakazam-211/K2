// Every package the lockfile installs (dev included) must carry a permissive licence.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The kit's licences: MIT, Apache-2.0, BSD (2- and 3-clause), ISC, Zlib, and Unicode-3.0.
const ALLOWED = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0"]);
// Named exceptions: each a dev-only package, with the licence it may have and why.
const EXCEPTIONS = new Map([
  // MIT-0 (MIT without the notice clause), reached only through jsdom's CSS parser in tests
  ["@csstools/color-helpers", "MIT-0"],
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
  const name = path.replace(/^.*node_modules\//, "");
  const exc = EXCEPTIONS.get(name);
  if (exc !== undefined && lic === exc) {
    if (!p.dev) bad.push(`${name}@${p.version}: ${lic} is allowed for a dev-only package, and this one ships`);
    continue;
  }
  if (!lic || !ok(lic)) bad.push(`${path.replace(/^.*node_modules\//, "")}@${p.version}: ${lic || "(none)"}`);
}
console.log(`${n} packages checked`);
if (bad.length) {
  console.error(`not permissive:\n  ${bad.join("\n  ")}`);
  process.exit(1);
}
