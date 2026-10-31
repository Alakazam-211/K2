// The rope (src/rope.ts) and the editor's byte index (src/editor/bytes.ts) against plain-string
// models: random edits on random texts (ASCII, CJK, emoji, ZWJ sequences, line breaks), tiny
// chunks so edits land on chunk edges and surrogate pairs sit at them, empty and large documents,
// every UTF-8/UTF-16 conversion, validation, slices, inversion, equality, and that an edit never
// changes the version it came from. A planted fault (a chunk's cached UTF-8 length off by one)
// must be caught.

import { ChangeSet, Text } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";
import { ByteIndex } from "../src/editor/bytes";
import { apply, byteToIndex, invert, utf8Len, validate, type Hunk } from "../src/hunk";
import { Rope } from "../src/rope";
import { rng } from "./fake-server";

type R = ReturnType<typeof rng>;
const BITS = ["a", "b", "c", " ", "\n", "é", "日", "本", "🎉", "😀", "👩‍💻", "xy", "word ", "한"];

function text(r: R, max: number): string {
  let s = "";
  for (let i = r.int(max + 1); i > 0; i--) s += r.pick(BITS);
  return s;
}

/** Character boundaries of `t` in UTF-8 bytes. */
function bounds(t: string): number[] {
  const out = [0];
  let b = 0;
  for (const ch of t) {
    b += utf8Len(ch);
    out.push(b);
  }
  return out;
}

/** Up to five valid hunks on `t` (sorted, non-overlapping, no empty insertion). */
function edit(r: R, t: string, bigInsert = 40): Hunk[] {
  const b = bounds(t);
  const n = 1 + r.int(5);
  const cuts = Array.from({ length: 2 * n }, () => r.pick(b)).sort((x, y) => x - y);
  const out: Hunk[] = [];
  for (let i = 0; i < cuts.length; i += 2) {
    let [s, e] = [cuts[i], cuts[i + 1]];
    const kind = r.int(3);
    if (kind === 0) e = s;
    const insert = kind === 1 ? "" : text(r, r.next() < 0.1 ? bigInsert : 3) || "z";
    const last = out[out.length - 1];
    if (last && (s < last.end || (s === last.end && s === e && last.start === last.end))) continue;
    if (s === e && !insert) continue;
    out.push({ start: s, end: e, insert });
  }
  return out;
}

/** Possibly invalid hunks, for `validate`. */
function wild(r: R, len: number): Hunk[] {
  const out: Hunk[] = [];
  for (let k = r.int(4); k > 0; k--) {
    const s = r.int(len + 3) - 1;
    const e = r.next() < 0.3 ? s : s + r.int(6) - 1;
    out.push({ start: s, end: e, insert: r.next() < 0.3 ? "" : "q" });
  }
  return out;
}

type Internals = { blocks: Array<{ chunks: string[]; s16: number[]; s8: number[] }>; b16: number[]; b8: number[] };

function check(rope: Rope, model: string, r: R, where: string) {
  expect(rope.toString(), where).toBe(model);
  const len8 = utf8Len(model);
  expect(rope.length16, where).toBe(model.length);
  expect(rope.length8, where).toBe(len8);
  // the blocks and chunks: none empty, no pair split, every start right
  const x = rope as unknown as Internals;
  let a = 0;
  let b = 0;
  const all = x.blocks.flatMap((blk) => blk.chunks);
  let seen = 0;
  x.blocks.forEach((blk, bi) => {
    expect(blk.chunks.length, `${where}: empty block`).toBeGreaterThan(0);
    expect([x.b16[bi], x.b8[bi]], `${where}: block ${bi}'s start`).toEqual([a, b]);
    let ra = 0;
    let rb = 0;
    blk.chunks.forEach((c, i) => {
      expect(c.length, `${where}: empty chunk`).toBeGreaterThan(0);
      const end = c.charCodeAt(c.length - 1);
      if (++seen < all.length) expect(end >= 0xd800 && end < 0xdc00, `${where}: a pair split at a chunk edge`).toBe(false);
      expect([blk.s16[i], blk.s8[i]], `${where}: block ${bi} chunk ${i}'s start`).toEqual([ra, rb]);
      ra += c.length;
      rb += utf8Len(c);
    });
    expect([blk.s16[blk.chunks.length], blk.s8[blk.chunks.length]], `${where}: block ${bi}'s length`).toEqual([ra, rb]);
    a += ra;
    b += rb;
  });
  expect([x.b16[x.blocks.length], x.b8[x.blocks.length]], `${where}: the totals`).toEqual([a, b]);
  // every byte offset (small texts), else a sample, and one past either end
  const offs = len8 <= 300 ? Array.from({ length: len8 + 3 }, (_, i) => i - 1) : Array.from({ length: 300 }, () => r.int(len8 + 3) - 1);
  for (const o of offs) expect(rope.byteToIndex(o), `${where}: byteToIndex(${o})`).toBe(o < 0 ? -1 : byteToIndex(model, o));
  // slices between boundaries
  const bs = bounds(model);
  for (let k = 0; k < 10; k++) {
    const [p, q] = [r.pick(bs), r.pick(bs)].sort((u, v) => u - v);
    const i = byteToIndex(model, p);
    const j = byteToIndex(model, q);
    expect(rope.slice8(p, q), `${where}: slice8(${p}, ${q})`).toBe(model.slice(i, j));
    expect(rope.slice16(i, j), `${where}: slice16(${i}, ${j})`).toBe(model.slice(i, j));
  }
  // validation, of valid and invalid hunks
  for (let k = 0; k < 5; k++) {
    const hs = r.next() < 0.5 ? edit(r, model) : wild(r, len8);
    expect(rope.validate(hs), `${where}: validate ${JSON.stringify(hs)}`).toEqual(validate(model, hs));
  }
}

/** Random edits from `start`, rope against model; `chunk`, `block`: the rope's sizes. */
function property(seed: number, start: string, chunk: number | undefined, steps: number, bigInsert = 40, block?: number) {
  const r = rng(seed);
  let model = start;
  let rope = Rope.of(start, { ...(chunk === undefined ? {} : { chunk }), ...(block === undefined ? {} : { block }) });
  check(rope, model, r, `seed ${seed} start`);
  for (let s = 0; s < steps; s++) {
    const hs = edit(r, model, bigInsert);
    const where = `seed ${seed} chunk ${chunk} step ${s} ${JSON.stringify(hs).slice(0, 200)}`;
    const before = rope;
    const beforeModel = model;
    expect(rope.invert(hs), `${where}: invert`).toEqual(invert(model, hs));
    rope = rope.apply(hs);
    model = apply(model, hs);
    // the version it came from is untouched (chunks are shared, never changed)
    expect(before.toString(), `${where}: the old version changed`).toBe(beforeModel);
    expect(before.length8, `${where}: the old version's length changed`).toBe(utf8Len(beforeModel));
    // equality: with a fresh rope of the same text (other chunking), and with the old version
    expect(rope.equals(Rope.of(model, { chunk: 1 + r.int(9) })), `${where}: equals a fresh rope`).toBe(true);
    expect(rope.equals(before), `${where}: equals the old version`).toBe(model === beforeModel);
    if (s % 5 === 0 || model.length < 400) check(rope, model, r, where);
  }
  check(rope, model, r, `seed ${seed} end`);
}

describe("the rope against a plain string", () => {
  it("random edits, tiny chunks (edges everywhere), from empty and from random text", () => {
    let n = 0;
    for (const chunk of [2, 3, 4, 7, 16]) {
      for (let seed = 1; seed <= 40; seed++) {
        property(seed * 31 + chunk, seed % 4 === 0 ? "" : text(rng(seed), 60), chunk, 60);
        n++;
      }
    }
    expect(n).toBe(200);
  });

  it("many blocks: tiny chunks and tiny blocks, edits across block edges", () => {
    for (const block of [2, 3, 5]) {
      for (let seed = 1; seed <= 12; seed++) property(7000 + seed * 7 + block, text(rng(seed), 400), 3, 80, 120, block);
    }
  });

  it("surrogate pairs at every chunk edge: a text of emoji only", () => {
    for (const chunk of [2, 3, 5]) {
      const start = "😀🎉👩‍💻".repeat(40);
      property(9000 + chunk, start, chunk, 80);
    }
  });

  it("default chunks, with large insertions and deletions across many chunks", () => {
    for (let seed = 1; seed <= 20; seed++) property(500 + seed, text(rng(seed), 3000), undefined, 40, 3000);
  });

  it("an empty document: every query, then growth from nothing", () => {
    const e = Rope.of("");
    expect([e.length16, e.length8, e.toString(), e.byteToIndex(0), e.byteToIndex(1), e.validate([]), e.equals(Rope.of(""))]).toEqual([0, 0, "", 0, -1, null, true]);
    expect(e.apply([])).toBe(e);
    property(77, "", 4, 100);
  });

  it("a large document (1 MB of CJK and emoji): edits and lookups stay exact", () => {
    const r = rng(4242);
    let big = "";
    while (utf8Len(big) < 1_000_000) big += text(r, 400);
    property(4243, big, undefined, 60, 5000);
  });

  it("planted fault (a block's cached UTF-8 total off by one when it has an odd number of chunks): caught", () => {
    const orig = Rope.blockTotal8;
    const spy = vi.spyOn(Rope, "blockTotal8").mockImplementation((b) => orig(b) + (b.chunks.length % 2));
    try {
      let caught = 0;
      for (let seed = 1; seed <= 10; seed++) {
        try {
          property(seed * 13 + 5, text(rng(seed), 300), 3, 40, 40, 3);
        } catch {
          caught++;
        }
      }
      expect(caught, "the fault went unnoticed").toBe(10);
    } finally {
      spy.mockRestore();
    }
  });

  it("planted fault (a chunk's cached UTF-8 length off by one when it ends in a 4-byte character): caught", () => {
    const orig = Rope.measure8;
    const spy = vi.spyOn(Rope, "measure8").mockImplementation((s: string) => {
      const c = s.charCodeAt(s.length - 1);
      return orig(s) + (c >= 0xdc00 && c < 0xe000 ? 1 : 0);
    });
    try {
      let caught = 0;
      for (let seed = 1; seed <= 10; seed++) {
        try {
          property(seed * 31 + 3, text(rng(seed), 60), 3, 60);
        } catch {
          caught++;
        }
      }
      expect(caught, "the fault went unnoticed").toBe(10);
    } finally {
      spy.mockRestore();
    }
  });
});

describe("the editor's byte index against a plain string", () => {
  /** A random document of many short lines (several blocks), random transactions. */
  function bytesProperty(seed: number, lines: number, steps: number) {
    const r = rng(seed);
    const ls = Array.from({ length: lines }, () => text(r, 6).replace(/\n/g, ""));
    let doc = Text.of(ls);
    let idx = ByteIndex.of(doc);
    for (let s = 0; s < steps; s++) {
      const str = doc.toString();
      const hs = edit(r, str, 2000);
      const specs = hs.map((h) => ({ from: byteToIndex(str, h.start), to: byteToIndex(str, h.end), insert: h.insert }));
      const cs = ChangeSet.of(specs, doc.length);
      const next = cs.apply(doc);
      const old = idx;
      const oldDoc = doc;
      idx = idx.update(doc, next, cs);
      doc = next;
      const now = doc.toString();
      const where = `seed ${seed} step ${s}`;
      expect(idx.total(), where).toBe(utf8Len(now));
      for (let k = 0; k < 40; k++) {
        const pos = r.int(doc.length + 1);
        const want = utf8Len(now.slice(0, pos));
        // a position inside a pair is clamped by CodeMirror's own lineAt: compare at boundaries
        if (byteToIndex(now, want) !== pos) continue;
        expect(idx.byteAt(doc, pos), `${where}: byteAt(${pos})`).toBe(want);
        expect(idx.posAt(doc, want), `${where}: posAt(${want})`).toBe(pos);
      }
      for (let k = 0; k < 20; k++) {
        const b = r.int(utf8Len(now) + 2);
        expect(idx.posAt(doc, b), `${where}: posAt(${b})`).toBe(byteToIndex(now, b));
      }
      // the index it came from still answers for its own document
      const was = oldDoc.toString();
      expect(old.total(), `${where}: the old index changed`).toBe(utf8Len(was));
      const p = r.int(oldDoc.length + 1);
      const w = utf8Len(was.slice(0, p));
      if (byteToIndex(was, w) === p) expect(old.byteAt(oldDoc, p), `${where}: the old index changed`).toBe(w);
    }
  }

  it("random transactions on documents of one block and of many", () => {
    for (let seed = 1; seed <= 6; seed++) bytesProperty(seed, 40, 60);
    for (let seed = 7; seed <= 12; seed++) bytesProperty(seed, 3000, 60);
  });
});
