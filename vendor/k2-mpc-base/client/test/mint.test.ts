// The kit's tagged-key minting vectors (test/vectors/mint.json, written by the Rust kit's
// crates/base/tests/mint_vectors.rs): src/mint.ts must match every one exactly.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { formatKey, nextFor, parseKey, parseTag, retiredMark, writerTag } from "../src/mint";
import { runrec } from "./runrec";

const v = JSON.parse(readFileSync(new URL("./vectors/mint.json", import.meta.url), "utf8"));

describe("mint vectors", () => {
  it("writer tags", () => {
    expect(v.tags.length).toBeGreaterThanOrEqual(60);
    for (const c of v.tags) {
      expect(writerTag(c.client), `client ${c.client}`).toBe(c.tag);
      if (c.tag !== null) expect(parseTag(c.tag), c.tag).toBe(c.client);
    }
  });
  it("parse", () => {
    expect(v.parse.length).toBeGreaterThanOrEqual(300);
    for (const c of v.parse) {
      const got = parseKey(c.key);
      const want = c.tagged === null ? null : { prefix: c.tagged.prefix, n: BigInt(c.tagged.n), tag: c.tagged.tag };
      expect(got, c.key).toEqual(want);
    }
  });
  it("format", () => {
    for (const c of v.format) {
      const got = formatKey(c.prefix, BigInt(c.n), c.tag);
      if ("key" in c) expect(got, JSON.stringify(c)).toEqual({ key: c.key });
      else expect("error" in got && got.error.kind, JSON.stringify(c)).toBe(c.error);
    }
  });
  it("next_for", () => {
    expect(v.next.length).toBe(200);
    for (const c of v.next) {
      const got = nextFor(c.places, BigInt(c.retired), c.prefix, c.tag);
      if ("n" in c) expect(got, JSON.stringify(c)).toEqual({ n: BigInt(c.n) });
      else expect("error" in got && got.error.kind, JSON.stringify(c)).toBe(c.error);
    }
  });
  it("retired marks", () => {
    expect(v.marks.length).toBe(120);
    for (const c of v.marks) {
      expect(retiredMark(c.marks, c.root, c.prefix, c.tag), JSON.stringify(c)).toBe(BigInt(c.mark));
    }
    runrec("client_mint_vectors", { tags: v.tags.length, parse: v.parse.length, next: v.next.length, marks: v.marks.length });
  });
});
