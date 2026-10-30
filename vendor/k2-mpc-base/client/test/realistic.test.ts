// "Realistic typing": three people in one 2–20 KB doc, each at their own caret: ~90%
// keystrokes at the caret, ~7% word replacements (select a word, type another), a few
// backspaces and line deletions; a keystroke every 60–260 ms per person; links of 50–300 ms.
// Reports the refusal rate (what a person would see bounce) and checks convergence.

import { describe, expect, it } from "vitest";
import { utf8Len, type Hunk } from "../src/hunk";
import { LiveText } from "../src/live";
import { Bias, transform } from "../src/position";
import { FakeServer, Net, rng } from "./fake-server";
import { runrec } from "./runrec";

const WORDS = "the quick brown fox jumps over a lazy dog while seven bright éclairs wait at 日本 for 🎉 news".split(" ");

function doc(r: ReturnType<typeof rng>, bytes: number): string {
  let s = "";
  while (utf8Len(s) < bytes) s += r.pick(WORDS) + (r.next() < 0.08 ? "\n" : " ");
  return s;
}

/** Byte offset of the char boundary at or before `b` in `t`. */
function floor(t: string, b: number): number {
  let n = 0;
  for (const ch of t) {
    const l = utf8Len(ch);
    if (n + l > b) break;
    n += l;
  }
  return n;
}

/** The word (letters, digits, multi-byte) around byte `b`: [start, end). */
function wordAt(t: string, b: number): [number, number] {
  const chars = [...t];
  let pos = 0;
  let i = 0;
  while (i < chars.length && pos + utf8Len(chars[i]) <= b) pos += utf8Len(chars[i++]);
  const isW = (c: string | undefined) => !!c && !/\s/.test(c);
  let s = i;
  let sb = pos;
  while (s > 0 && isW(chars[s - 1])) sb -= utf8Len(chars[--s]);
  let e = i;
  let eb = pos;
  while (e < chars.length && isW(chars[e])) eb += utf8Len(chars[e++]);
  return [sb, eb];
}

function run(seed: number, keystrokes: number) {
  const r = rng(seed);
  const net = new Net();
  const server = new FakeServer(net, doc(r, 2000 + r.int(18000)));
  const people = [1, 2, 3].map((i) => {
    const live = new LiveText(server.snapshot(), server.connect(i, () => 50 + r.int(251)), { client: i });
    const p = { live, caret: floor(live.text, r.int(utf8Len(live.text) + 1)), typed: 0, rejected: 0, n: 0, group: 0 };
    live.onChange((e) => {
      if (!e.local) p.caret = transform(p.caret, e.hunks, Bias.After);
    });
    live.onRejected((x) => (p.rejected += x.ops.length));
    live.onReload(() => {
      throw new Error("a reload in realistic typing");
    });
    return p;
  });
  const act = (p: (typeof people)[number]) => {
    if (p.n++ >= keystrokes) return;
    const t = p.live.text;
    const len = utf8Len(t);
    p.caret = floor(t, Math.min(p.caret, len));
    const roll = r.next();
    let hs: Hunk[] = [];
    let group = `type-${p.group}`;
    if (roll < 0.9) {
      const ch = r.next() < 0.15 ? " " : r.next() < 0.02 ? "\n" : r.pick(["a", "e", "t", "o", "é", "n"]);
      hs = [{ start: p.caret, end: p.caret, insert: ch }];
      p.caret += utf8Len(ch);
      if (ch === " " || ch === "\n") p.group++;
    } else if (roll < 0.97) {
      const [s, e] = wordAt(t, p.caret);
      const w = r.pick(WORDS);
      if (s < e) hs = [{ start: s, end: e, insert: w }];
      else hs = [{ start: s, end: s, insert: w }];
      p.caret = s + utf8Len(w);
      group = `replace-${p.n}`;
      p.group++;
    } else if (roll < 0.995) {
      if (p.caret > 0) {
        const s = floor(t, p.caret - 1);
        hs = [{ start: s, end: p.caret, insert: "" }];
        p.caret = s;
        group = `delete-${p.group}`;
      }
    } else {
      // delete the caret's line
      const idx = indexOf(t, p.caret);
      const before = t.slice(0, idx);
      const ls = utf8Len(before.slice(0, before.lastIndexOf("\n") + 1));
      const nl = t.slice(idx).indexOf("\n");
      const le = nl < 0 ? len : utf8Len(t.slice(0, idx + nl + 1));
      if (ls < le) hs = [{ start: ls, end: le, insert: "" }];
      p.caret = ls;
      group = `line-${p.n}`;
      p.group++;
    }
    if (hs.length) {
      p.live.applyLocal(hs, { group });
      p.typed++;
    }
    net.at(60 + r.int(201), () => act(p));
  };
  for (const p of people) net.at(r.int(200), () => act(p));
  while (net.step());
  for (const p of people) {
    expect(p.live.pending).toBe(0);
    expect(p.live.text).toBe(server.text);
  }
  return {
    typed: people.reduce((n, p) => n + p.typed, 0),
    rejected: people.reduce((n, p) => n + p.rejected, 0),
    refused: server.refused,
    bytes: utf8Len(server.text),
  };
}

function indexOf(t: string, byte: number): number {
  let n = 0;
  let i = 0;
  for (const ch of t) {
    if (n >= byte) break;
    n += utf8Len(ch);
    i += ch.length;
  }
  return i;
}

describe("realistic typing", () => {
  it("three people at their carets in 2–20 KB docs: converge, and few changes bounce", () => {
    let typed = 0;
    let rejected = 0;
    let refused = 0;
    const runs = Number(process.env.REALISTIC_RUNS ?? 40);
    for (let seed = 1; seed <= runs; seed++) {
      const s = run(seed, 300);
      typed += s.typed;
      rejected += s.rejected;
      refused += s.refused;
    }
    const rate = rejected / typed;
    console.log(
      `realistic typing: ${runs} runs, ${typed} edits, ${rejected} rejected (${(rate * 100).toFixed(3)}%), ${refused} refused by the server`,
    );
    expect(typed).toBeGreaterThan(runs * 800);
    expect(rate).toBeLessThan(0.01);
    runrec("client_realistic", { runs, typed, rejected, refused });
  });
});
