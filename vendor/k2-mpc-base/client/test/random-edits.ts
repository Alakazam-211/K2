// Random text and edits for the convergence harnesses (UTF-8 byte hunks on character
// boundaries, with multi-byte characters and line breaks).

import { utf8Len, type Hunk } from "../src/hunk";
import type { rng } from "./fake-server";

type R = ReturnType<typeof rng>;

const BITS = ["a", "b", "c", " ", "\n", "é", "日", "🎉", "xy", "word "];

export function boundaries(t: string): number[] {
  const out = [0];
  let b = 0;
  for (const ch of t) {
    b += utf8Len(ch);
    out.push(b);
  }
  return out;
}

export function randomText(r: R, max: number) {
  let s = "";
  for (let i = r.int(max + 1); i > 0; i--) s += r.pick(BITS);
  return s;
}

export type Kind = "any" | "insert" | "delete";

/** One random edit on `t`: an insertion, a deletion or a replacement (sometimes two). */
export function randomEdit(r: R, t: string, only: Kind = "any"): Hunk[] {
  const b = boundaries(t);
  const n = 1 + (r.next() < 0.2 ? 1 : 0);
  const cuts = Array.from({ length: 2 * n }, () => r.pick(b)).sort((x, y) => x - y);
  const out: Hunk[] = [];
  for (let i = 0; i < cuts.length; i += 2) {
    let [s, e] = [cuts[i], cuts[i + 1]];
    const kind = only === "insert" ? 0 : only === "delete" ? 1 : r.int(3); // 0 insert, 1 delete, 2 replace
    if (kind === 0) e = s;
    const insert = kind === 1 ? "" : randomText(r, 2) || "z";
    const last = out[out.length - 1];
    if (last && (s < last.end || (s === last.end && s === e && last.start === last.end))) continue;
    if (s === e && !insert) continue;
    if (kind === 1 && s === e) continue;
    out.push({ start: s, end: e, insert });
  }
  return out;
}

