// The replica's text store: a persistent two-level rope. The text is a list of chunks (strings of
// about `CHUNK` UTF-16 units, never splitting a surrogate pair) in blocks of about `BLOCK` chunks.
// Each block keeps its chunks' starts (UTF-16 units and UTF-8 bytes, from the block's start);
// the rope keeps each block's start. An edit rebuilds only the chunks its hunks touch, in the
// blocks that hold them (and one block either side, for merging), shares every other block, and
// copies the list of blocks with its starts:
//
// - a byte offset finds its block, then its chunk, by binary search, and its UTF-16 index by a
//   scan of one chunk;
// - `apply` costs the touched chunks, a few blocks of pointers, and O(blocks) integer work
//   (a 4 MB document is about 4,000 chunks in about 130 blocks), never a scan of the text;
// - two ropes compare block by block, and a shared block (or chunk) compares by identity;
// - the whole string is built only when asked for (`toString`), then cached.
//
// The semantics are `hunk.ts`'s on a plain string, exactly: the differential property test
// (`test/rope.test.ts`) holds the two to each other.

import { byteToIndex as byteToIndexIn, utf8Len, type Hunk, type HunkError } from "./hunk";

/** About this many UTF-16 units per chunk; chunks stay between a quarter and twice this. */
const CHUNK = 1024;
/** About this many chunks per block; blocks stay between a quarter and twice this. */
const BLOCK = 32;

interface Block {
  /** The chunks, none empty. */
  readonly chunks: readonly string[];
  /** `s16[i]`: chunk i's start in UTF-16 units from the block's start; the last entry is the
   * block's length. */
  readonly s16: readonly number[];
  /** As `s16`, in UTF-8 bytes. */
  readonly s8: readonly number[];
}

export class Rope {
  private flat: string | null = null;

  private constructor(
    /** The blocks, none empty (an empty text has none). */
    private readonly blocks: readonly Block[],
    /** `b16[b]`: block b's start in UTF-16 units; the last entry is the total. */
    private readonly b16: readonly number[],
    /** As `b16`, in UTF-8 bytes. */
    private readonly b8: readonly number[],
    /** The target chunk size (units) and block size (chunks); tests make them small to put
     * edits at chunk and block edges. */
    private readonly size: number,
    private readonly blockSize: number,
  ) {}

  /** A chunk's length in UTF-8 bytes (a seam the planted-fault test overrides). */
  static measure8(s: string): number {
    return utf8Len(s);
  }

  /** A block's length in UTF-8 bytes, for the rope's block starts (a seam the planted-fault
   * test overrides). */
  static blockTotal8(b: Block): number {
    return b.s8[b.chunks.length];
  }

  /** The rope holding `s`. `chunk` (UTF-16 units) and `block` (chunks): target sizes, for tests. */
  static of(s: string, opts: { chunk?: number; block?: number } = {}): Rope {
    const size = Math.max(2, opts.chunk ?? CHUNK);
    const blockSize = Math.max(2, opts.block ?? BLOCK);
    const chunks = split(s, size);
    const r = Rope.top(makeBlocks(chunks, chunks.map((c) => Rope.measure8(c)), blockSize), size, blockSize);
    r.flat = s;
    return r;
  }

  /** A rope of `blocks`, its block starts computed. */
  private static top(blocks: readonly Block[], size: number, blockSize: number): Rope {
    const b16: number[] = [0];
    const b8: number[] = [0];
    for (const b of blocks) {
      b16.push(b16[b16.length - 1] + b.s16[b.chunks.length]);
      b8.push(b8[b8.length - 1] + Rope.blockTotal8(b));
    }
    return new Rope(blocks, b16, b8, size, blockSize);
  }

  /** The length in UTF-16 units. */
  get length16(): number {
    return this.b16[this.blocks.length];
  }

  /** The length in UTF-8 bytes. */
  get length8(): number {
    return this.b8[this.blocks.length];
  }

  /** The whole text (built once, then cached). */
  toString(): string {
    if (this.flat === null) {
      const parts: string[] = [];
      for (const b of this.blocks) for (const c of b.chunks) parts.push(c);
      this.flat = parts.join("");
    }
    return this.flat;
  }

  /** The UTF-16 index of UTF-8 offset `byte`, or -1 when it is past the end or inside a
   * character (as `hunk.ts`'s `byteToIndex` on the whole text). */
  byteToIndex(byte: number): number {
    if (!Number.isInteger(byte) || byte < 0) return -1;
    const nb = this.blocks.length;
    if (byte >= this.b8[nb]) return byte === this.b8[nb] ? this.b16[nb] : -1;
    const bi = last(this.b8, nb, byte);
    const blk = this.blocks[bi];
    const rel = byte - this.b8[bi];
    const ci = last(blk.s8, blk.chunks.length, rel);
    const into = rel - blk.s8[ci];
    const at = this.b16[bi] + blk.s16[ci];
    if (into === 0) return at;
    const j = byteToIndexIn(blk.chunks[ci], into);
    return j < 0 ? -1 : at + j;
  }

  /** Whether `byte` is a character boundary (0 and the end included). */
  isBoundary(byte: number): boolean {
    return this.byteToIndex(byte) >= 0;
  }

  /** The text between UTF-16 indexes `a` and `b`. */
  slice16(a: number, b: number): string {
    if (a >= b) return "";
    if (this.flat !== null) return this.flat.slice(a, b);
    const nb = this.blocks.length;
    let out = "";
    for (let bi = last(this.b16, nb, a); bi < nb && this.b16[bi] < b; bi++) {
      const blk = this.blocks[bi];
      const base = this.b16[bi];
      for (let ci = 0; ci < blk.chunks.length; ci++) {
        const cs = base + blk.s16[ci];
        const ce = base + blk.s16[ci + 1];
        if (ce <= a) continue;
        if (cs >= b) break;
        out += blk.chunks[ci].slice(Math.max(0, a - cs), Math.min(ce, b) - cs);
      }
    }
    return out;
  }

  /** The text between UTF-8 offsets `a` and `b` (character boundaries). */
  slice8(a: number, b: number): string {
    const i = this.byteToIndex(a);
    const j = this.byteToIndex(b);
    if (i < 0 || j < 0) throw new Error("slice off a character boundary or past the end");
    return this.slice16(i, j);
  }

  /** As `hunk.ts`'s `validate` on the whole text: in order, non-overlapping, in range, on
   * character boundaries, no empty insertion. `null` when valid. */
  validate(hunks: readonly Hunk[]): HunkError | null {
    const len = this.length8;
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
        !this.isBoundary(h.start) ||
        !this.isBoundary(h.end)
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

  /** This text with `hunks` applied (as `hunk.ts`'s `apply`; they must be valid for it). Blocks
   * no hunk touches are shared with this rope. */
  apply(hunks: readonly Hunk[]): Rope {
    if (hunks.length === 0) return this;
    const nb = this.blocks.length;
    const size = this.size;
    // the hunks in UTF-16 units of this text
    const rs: Array<[number, number, string]> = [];
    let prev = 0;
    for (const h of hunks) {
      const a = this.byteToIndex(h.start);
      const b = h.end === h.start ? a : this.byteToIndex(h.end);
      if (a < 0 || b < 0 || a < prev) throw new Error("hunk offset off a character boundary or past the end");
      rs.push([a, b, h.insert]);
      prev = b;
    }
    // the blocks rebuilt: from the one before the first hunk's to the one after the last's
    const blockAt = (x: number) => (x >= this.b16[nb] ? nb - 1 : last(this.b16, nb, x));
    const bA = nb === 0 ? 0 : Math.max(0, blockAt(rs[0][0]) - 1);
    const bB = nb === 0 ? -1 : Math.min(nb - 1, blockAt(rs[rs.length - 1][1]) + 1);
    // their chunks, with their starts in the whole text
    const cs: string[] = [];
    const st: number[] = [];
    const l8: number[] = [];
    for (let bi = bA; bi <= bB; bi++) {
      const blk = this.blocks[bi];
      for (let ci = 0; ci < blk.chunks.length; ci++) {
        cs.push(blk.chunks[ci]);
        st.push(this.b16[bi] + blk.s16[ci]);
        l8.push(blk.s8[ci + 1] - blk.s8[ci]);
      }
    }
    const lo16 = nb === 0 ? 0 : this.b16[bA];
    const hi16 = nb === 0 ? 0 : this.b16[bB + 1];
    st.push(hi16);
    // rebuild them: untouched chunks kept (the same strings), touched ones re-cut
    const out: string[] = [];
    const lens: number[] = [];
    let buf = "";
    const flush = () => {
      if (!buf) return;
      for (const c of buf.length <= 2 * size ? [buf] : split(buf, size)) {
        out.push(c);
        lens.push(Rope.measure8(c));
      }
      buf = "";
    };
    const whole = (i: number) => {
      if (buf) {
        if (buf.length < size / 2) {
          buf += cs[i]; // a small remainder joins its neighbour
          return;
        }
        flush();
      }
      out.push(cs[i]);
      lens.push(l8[i]);
    };
    let i = 0; // the chunk `pos` is in (or starts)
    let pos = lo16; // UTF-16 units of the old text consumed
    const take = (to: number) => {
      while (pos < to) {
        const s = st[i];
        const e = st[i + 1];
        if (pos === s && e <= to) whole(i);
        else buf += cs[i].slice(pos - s, Math.min(e, to) - s);
        pos = Math.min(e, to);
        if (pos === e) i++;
      }
    };
    const skip = (to: number) => {
      while (pos < to) {
        const e = st[i + 1];
        pos = Math.min(e, to);
        if (pos === e) i++;
      }
    };
    for (const [a, b, insert] of rs) {
      take(a);
      buf += insert;
      skip(b);
    }
    take(hi16);
    if (buf && buf.length < size / 2 && out.length > 0) {
      buf = out.pop()! + buf;
      lens.pop();
    }
    flush();
    // splice the blocks: the shared ones before, the rebuilt ones, the shared ones after
    const fresh = makeBlocks(out, lens, this.blockSize);
    const blocks = this.blocks.slice(0, bA);
    const b16 = this.b16.slice(0, bA + 1);
    const b8 = this.b8.slice(0, bA + 1);
    for (const b of fresh) {
      blocks.push(b);
      b16.push(b16[b16.length - 1] + b.s16[b.chunks.length]);
      b8.push(b8[b8.length - 1] + Rope.blockTotal8(b));
    }
    const d16 = b16[b16.length - 1] - (nb === 0 ? 0 : this.b16[bB + 1]);
    const d8 = b8[b8.length - 1] - (nb === 0 ? 0 : this.b8[bB + 1]);
    for (let bi = bB + 1; bi < nb; bi++) {
      blocks.push(this.blocks[bi]);
      b16.push(this.b16[bi + 1] + d16);
      b8.push(this.b8[bi + 1] + d8);
    }
    return new Rope(blocks, b16, b8, size, this.blockSize);
  }

  /** Whether the two hold the same text (shared blocks and chunks compare by identity). */
  equals(o: Rope): boolean {
    if (this === o) return true;
    if (this.length16 !== o.length16 || this.length8 !== o.length8) return false;
    if (this.flat !== null && o.flat !== null) return this.flat === o.flat;
    const A = this.blocks;
    const B = o.blocks;
    // a cursor in each: block, chunk, offset into the chunk
    let ab = 0;
    let ac = 0;
    let ao = 0;
    let bb = 0;
    let bc = 0;
    let bo = 0;
    while (ab < A.length && bb < B.length) {
      if (ac === 0 && ao === 0 && bc === 0 && bo === 0 && A[ab] === B[bb]) {
        ab++;
        bb++;
        continue;
      }
      const x = A[ab].chunks[ac];
      const y = B[bb].chunks[bc];
      if (ao === 0 && bo === 0 && x.length === y.length) {
        if (x !== y) return false;
        ao = x.length;
        bo = y.length;
      } else {
        const k = Math.min(x.length - ao, y.length - bo);
        if (x.slice(ao, ao + k) !== y.slice(bo, bo + k)) return false;
        ao += k;
        bo += k;
      }
      if (ao === x.length) {
        ao = 0;
        if (++ac === A[ab].chunks.length) {
          ac = 0;
          ab++;
        }
      }
      if (bo === y.length) {
        bo = 0;
        if (++bc === B[bb].chunks.length) {
          bc = 0;
          bb++;
        }
      }
    }
    return true;
  }

  /** The hunks undoing `hunks` (applied to this text): as `hunk.ts`'s `invert`. */
  invert(hunks: readonly Hunk[]): Hunk[] {
    let delta = 0;
    const out: Hunk[] = [];
    for (const h of hunks) {
      const start = h.start + delta;
      const ins = utf8Len(h.insert);
      out.push({ start, end: start + ins, insert: h.end > h.start ? this.slice8(h.start, h.end) : "" });
      delta += ins - (h.end - h.start);
    }
    return out.filter((h) => h.start !== h.end || h.insert.length > 0);
  }
}

/** A block of `chunks` (their UTF-8 lengths `lens8`), its starts computed. */
function makeBlock(chunks: string[], lens8: number[]): Block {
  const s16: number[] = [0];
  const s8: number[] = [0];
  for (let i = 0; i < chunks.length; i++) {
    s16.push(s16[i] + chunks[i].length);
    s8.push(s8[i] + lens8[i]);
  }
  return { chunks, s16, s8 };
}

/** `chunks` in blocks of about `size` (one block when they fit in twice that; no small last
 * block); none when there are no chunks. */
function makeBlocks(chunks: string[], lens8: number[], size: number): Block[] {
  if (chunks.length === 0) return [];
  if (chunks.length <= 2 * size) return [makeBlock(chunks, lens8)];
  const out: Block[] = [];
  for (let at = 0; at < chunks.length; ) {
    let end = Math.min(chunks.length, at + size);
    if (chunks.length - end < size / 2) end = chunks.length;
    out.push(makeBlock(chunks.slice(at, end), lens8.slice(at, end)));
    at = end;
  }
  return out;
}

/** The last index i < n with `starts[i] <= x` (starts ascending, starts[0] = 0). */
function last(starts: readonly number[], n: number, x: number): number {
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= x) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** `s` in chunks of about `size` units, none empty, never splitting a surrogate pair. */
function split(s: string, size: number): string[] {
  const out: string[] = [];
  let at = 0;
  while (at < s.length) {
    let end = Math.min(s.length, at + size);
    if (end < s.length && s.length - end < size / 2) end = s.length; // no small last chunk
    const c = s.charCodeAt(end - 1);
    if (end < s.length && c >= 0xd800 && c < 0xdc00) end--; // keep the pair together
    if (end === at) end = at + 2;
    out.push(s.slice(at, end));
    at = end;
  }
  return out;
}
