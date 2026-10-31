// Interim code is marked `INTERIM` and removed when what it stands in for lands: none may be
// left in the client's sources.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p) : [p];
  });
}

describe("client/src", () => {
  it("has no INTERIM code left", () => {
    const src = fileURLToPath(new URL("../src", import.meta.url));
    const all = files(src);
    expect(all.length).toBeGreaterThan(5);
    const marked = all.filter((f) => readFileSync(f, "utf8").includes("INTERIM"));
    expect(marked).toEqual([]);
  });
});
