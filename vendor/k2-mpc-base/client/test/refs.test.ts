// The kit's reference-state vectors (test/vectors/refs.json, written by the Rust kit's
// crates/base/tests/refs_vectors.rs): src/refs.ts must match every one exactly.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { resolveParents, resolveRef, resolveRefs, type Lists } from "../src/refs";
import { runrec } from "./runrec";

const v = JSON.parse(readFileSync(new URL("./vectors/refs.json", import.meta.url), "utf8"));

describe("refs vectors", () => {
  it(`matches all ${v.cases.length} cases`, () => {
    expect(v.cases.length).toBe(300);
    let refs = 0;
    let parents = 0;
    for (const c of v.cases) {
      const lists: Lists = { items: c.live };
      const kindOk = c.kind === "box" ? (x: string | null) => x === "box" : () => true;
      const got = resolveRefs(lists, c.refs, kindOk);
      expect(got.map(([, s]) => s), JSON.stringify(c)).toEqual(c.states);
      got.forEach(([r, s], i) => {
        expect(r, "unchanged, hint included").toBe(c.refs[i]);
        expect(resolveRef(lists, r, kindOk)).toBe(s);
      });
      refs += got.length;
      const ps = resolveParents(lists, "items", (k) => c.parents[k] ?? null);
      expect(Object.fromEntries([...ps].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))), JSON.stringify(c)).toEqual(c.parent_states);
      parents += ps.size;
    }
    expect(refs).toBeGreaterThan(500);
    runrec("client_refs_vectors", { cases: v.cases.length, refs, parents });
  });
});
