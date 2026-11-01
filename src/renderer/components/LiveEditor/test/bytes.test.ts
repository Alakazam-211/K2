// The editor's byte index (../bytes.ts) against a plain string: random transactions on documents
// of one CodeMirror block and of many (3000 lines), text with CJK, emoji and ZWJ sequences, every
// conversion checked at character boundaries, and an index never changing for the document it
// came from. Ported from the kit's rope.test.ts when kit v0.10.2 deleted its editor (K2 owns this
// copy; see ../index.ts).

import { ChangeSet, Text } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { byteToIndex, utf8Len, type Hunk } from "@k2/mpc-client";
import { rng } from "../../../../../vendor/k2-mpc-base/client/test/fake-server";
import { ByteIndex } from "../bytes";

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
