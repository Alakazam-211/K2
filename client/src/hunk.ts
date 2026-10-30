// Hunks over text, in UTF-8 byte offsets (as the kit's `text::diff::Hunk`), and the UTF-8
// helpers they need. JS strings are UTF-16; offsets are converted only at the edges (the
// CodeMirror binding), never on the wire.

/** Replace `old[start..end]` (UTF-8 byte offsets, on character boundaries) with `insert`. */
export interface Hunk {
  start: number;
  end: number;
  insert: string;
}

const encoder = new TextEncoder();

/** The length of `s` in UTF-8 bytes. */
export function utf8Len(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c < 0xdc00 && i + 1 < s.length) {
      const d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d < 0xe000) {
        n += 4;
        i++;
      } else n += 3;
    } else n += 3;
  }
  return n;
}

/**
 * The UTF-16 index in `s` of UTF-8 byte offset `byte`, or `-1` when `byte` is past the end
 * or inside a character.
 */
export function byteToIndex(s: string, byte: number): number {
  if (byte < 0) return -1;
  let b = 0;
  let i = 0;
  while (i < s.length && b < byte) {
    const c = s.charCodeAt(i);
    if (c < 0x80) {
      b += 1;
      i += 1;
    } else if (c < 0x800) {
      b += 2;
      i += 1;
    } else if (c >= 0xd800 && c < 0xdc00 && i + 1 < s.length) {
      b += 4;
      i += 2;
    } else {
      b += 3;
      i += 1;
    }
  }
  return b === byte ? i : -1;
}

/** The UTF-8 byte offset of UTF-16 index `index` in `s` (clamped to the end). */
export function indexToByte(s: string, index: number): number {
  return utf8Len(s.slice(0, Math.max(0, Math.min(index, s.length))));
}

/** Whether `byte` is a character boundary of `s` (0 and the end included). */
export function isBoundary(s: string, byte: number): boolean {
  return byteToIndex(s, byte) >= 0;
}

/** `old` with `hunks` applied (as the kit's `diff::apply`); `hunks` must be valid for it. */
export function apply(old: string, hunks: readonly Hunk[]): string {
  let out = "";
  let at = 0; // UTF-16
  let atByte = 0;
  for (const h of hunks) {
    const s = at + byteToIndexFrom(old, at, h.start - atByte);
    const e = s + byteToIndexFrom(old, s, h.end - h.start);
    out += old.slice(at, s) + h.insert;
    at = e;
    atByte = h.end;
  }
  return out + old.slice(at);
}

/** UTF-16 length of the `bytes` UTF-8 bytes of `s` from UTF-16 index `from`. */
function byteToIndexFrom(s: string, from: number, bytes: number): number {
  let b = 0;
  let i = from;
  while (i < s.length && b < bytes) {
    const c = s.charCodeAt(i);
    if (c < 0x80) b += 1;
    else if (c < 0x800) b += 2;
    else if (c >= 0xd800 && c < 0xdc00 && i + 1 < s.length) {
      b += 4;
      i++;
    } else b += 3;
    i++;
  }
  if (b !== bytes) throw new Error(`hunk offset off a character boundary or past the end`);
  return i - from;
}

/** Why hunks don't fit a text (as the kit's `HunkError`, `MPC-E151`). */
export type HunkError =
  | { kind: "range"; index: number }
  | { kind: "order"; index: number }
  | { kind: "empty"; index: number };

/**
 * Check `hunks` against `old` (as the kit's `diff::validate`): in order, non-overlapping, in
 * range, on character boundaries, no empty insertion. `null` when valid.
 */
export function validate(old: string, hunks: readonly Hunk[]): HunkError | null {
  const len = utf8Len(old);
  let prevEnd = 0;
  let prevInsertAt: number | null = null;
  for (let index = 0; index < hunks.length; index++) {
    const h = hunks[index];
    if (
      !Number.isInteger(h.start) ||
      !Number.isInteger(h.end) ||
      h.start < 0 ||
      h.start > h.end ||
      h.end > len ||
      !isBoundary(old, h.start) ||
      !isBoundary(old, h.end)
    ) {
      return { kind: "range", index };
    }
    const insertion = h.start === h.end;
    if (index > 0 && (h.start < prevEnd || (insertion && prevInsertAt === h.start))) {
      return { kind: "order", index };
    }
    if (insertion && h.insert.length === 0) return { kind: "empty", index };
    prevInsertAt = insertion ? h.start : null;
    prevEnd = h.end;
  }
  return null;
}

/** Merge hunks that touch, so the result is canonical (as the kit's `diff::coalesce`). */
export function coalesce(v: readonly Hunk[]): Hunk[] {
  const out: Hunk[] = [];
  for (const h of v) {
    const last = out[out.length - 1];
    if (last && last.end === h.start) {
      last.end = h.end;
      last.insert += h.insert;
      continue;
    }
    out.push({ start: h.start, end: h.end, insert: h.insert });
  }
  return out;
}

/**
 * The hunks that undo `hunks` on the text they produced from `old`: each one's inserted text
 * replaced by what it deleted. Canonical when `hunks` is.
 */
export function invert(old: string, hunks: readonly Hunk[]): Hunk[] {
  const bytes = encoder.encode(old);
  const dec = new TextDecoder();
  let delta = 0;
  const out: Hunk[] = [];
  for (const h of hunks) {
    const start = h.start + delta;
    const ins = utf8Len(h.insert);
    out.push({ start, end: start + ins, insert: dec.decode(bytes.subarray(h.start, h.end)) });
    delta += ins - (h.end - h.start);
  }
  return out.filter((h) => h.start !== h.end || h.insert.length > 0);
}

/**
 * The one change that `a` (on `t0`) then `b` (on `a`'s result) make, as canonical hunks on
 * `t0`: the text `t0` keeps, in order, with what replaces each gap between kept parts.
 */
export function compose(t0: string, a: readonly Hunk[], b: readonly Hunk[]): Hunk[] {
  // the text after `a`, as pieces: kept ranges of t0 (bytes) and inserted strings
  type Piece = { keep: [number, number] } | { ins: string };
  const ps: Piece[] = [];
  let at = 0;
  for (const h of a) {
    if (h.start > at) ps.push({ keep: [at, h.start] });
    if (h.insert) ps.push({ ins: h.insert });
    at = h.end;
  }
  const len0 = utf8Len(t0);
  if (at < len0) ps.push({ keep: [at, len0] });
  // apply `b` to the pieces: walk them with t1 offsets
  const out: Piece[] = [];
  let pi = 0;
  let pOff = 0; // bytes consumed in ps[pi]
  let t1 = 0; // t1 offset of the walk
  const plen = (p: Piece) => ("keep" in p ? p.keep[1] - p.keep[0] : utf8Len(p.ins));
  const take = (n: number, keep: boolean) => {
    // move n t1 bytes forward, copying them to out when `keep`
    while (n > 0) {
      const p = ps[pi];
      const rest = plen(p) - pOff;
      const k = Math.min(rest, n);
      if (keep) {
        if ("keep" in p) out.push({ keep: [p.keep[0] + pOff, p.keep[0] + pOff + k] });
        else out.push({ ins: sliceBytes(p.ins, pOff, pOff + k) });
      }
      pOff += k;
      n -= k;
      t1 += k;
      if (pOff === plen(p)) {
        pi++;
        pOff = 0;
      }
    }
  };
  for (const h of b) {
    take(h.start - t1, true);
    if (h.insert) out.push({ ins: h.insert });
    take(h.end - h.start, false);
  }
  while (pi < ps.length) take(plen(ps[pi]) - pOff, true);
  // back to hunks on t0: the gaps between kept ranges, with the strings between them
  const hs: Hunk[] = [];
  let k0 = 0;
  let ins = "";
  for (const p of out) {
    if ("ins" in p) {
      ins += p.ins;
      continue;
    }
    if (p.keep[0] > k0 || ins) hs.push({ start: k0, end: p.keep[0], insert: ins });
    ins = "";
    k0 = p.keep[1];
  }
  if (k0 < len0 || ins) hs.push({ start: k0, end: len0, insert: ins });
  return coalesce(hs.filter((h) => h.start !== h.end || h.insert.length > 0));
}

function sliceBytes(s: string, from: number, to: number): string {
  const i = byteToIndex(s, from);
  const j = byteToIndex(s, to);
  return s.slice(i, j);
}
