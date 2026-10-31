// Copied from vendor/k2-mpc-base/client/src/editor/bytes.ts (K2 owns this copy; see ./index.ts).
// UTF-16 <-> UTF-8 offsets on a CodeMirror document without flattening it. The index keeps each
// line's length in UTF-8 bytes in blocks of lines; an update rebuilds only the blocks of the lines
// a transaction touches and copies the list of blocks (one pointer per block), so a keystroke
// costs the changed lines plus O(lines / BLOCK), never `doc.toString()` and never a copy of every
// line's length. Indexes are persistent: an update leaves the index it came from valid (the
// binding reads the transaction's start index after the new one exists). The block sums are
// built lazily, once per index, on its first lookup.

import type { ChangeSet, Text } from "@codemirror/state";
import { byteToIndex, utf8Len } from "@k2/mpc-client";

/** Lines per block (a block holds between a quarter and twice this). */
const BLOCK = 256;

export class ByteIndex {
  /** Each block: the UTF-8 length of each of its lines, without the line break. */
  private readonly blocks: readonly (readonly number[])[];
  /** Built on the first lookup: `lines[b]` the first line of block b, `bytes[b]` its first
   * byte; one entry more than the blocks (the totals). */
  private pre: { lines: number[]; bytes: number[] } | null = null;

  private constructor(blocks: readonly (readonly number[])[]) {
    this.blocks = blocks;
  }

  static of(doc: Text): ByteIndex {
    const lens: number[] = [];
    for (const it = doc.iterLines(); !it.next().done; ) lens.push(utf8Len(it.value));
    return new ByteIndex(chop(lens));
  }

  /** The document's length in UTF-8 bytes. */
  total(): number {
    const p = this.sums();
    return p.bytes[p.bytes.length - 1] - 1; // no line break after the last line
  }

  /** The UTF-8 byte offset of UTF-16 position `pos` of `doc` (clamped to the document). */
  byteAt(doc: Text, pos: number): number {
    const p = Math.max(0, Math.min(pos, doc.length));
    const line = doc.lineAt(p);
    const into = p - line.from;
    return this.lineStart(line.number - 1) + (into === 0 ? 0 : utf8Len(line.text.slice(0, into)));
  }

  /** The UTF-16 position in `doc` of UTF-8 byte offset `byte`, or -1 when it isn't a boundary. */
  posAt(doc: Text, byte: number): number {
    if (byte < 0 || byte > this.total()) return -1;
    const p = this.sums();
    // the block, then the line, starting at or before `byte`
    let lo = 0;
    let hi = this.blocks.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (p.bytes[mid] <= byte) lo = mid;
      else hi = mid - 1;
    }
    const block = this.blocks[lo];
    let start = p.bytes[lo];
    let k = 0;
    while (k + 1 < block.length && start + block[k] + 1 <= byte) {
      start += block[k] + 1;
      k++;
    }
    const into = byte - start;
    if (into > block[k]) return -1; // inside the line break (a single byte: never)
    const line = doc.line(p.lines[lo] + k + 1);
    if (into === 0) return line.from;
    const i = byteToIndex(line.text, into);
    return i < 0 ? -1 : line.from + i;
  }

  /** The index of `newDoc`, after `changes` turned `oldDoc` into it. This index stays valid. */
  update(oldDoc: Text, newDoc: Text, changes: ChangeSet): ByteIndex {
    if (changes.empty) return this;
    // the changed lines, merged where they share a line: [old first, old last, new first, new last]
    const spans: Array<[number, number, number, number]> = [];
    changes.iterChangedRanges((fromA, toA, fromB, toB) => {
      const a0 = oldDoc.lineAt(fromA).number - 1;
      const a1 = oldDoc.lineAt(toA).number - 1;
      const b0 = newDoc.lineAt(fromB).number - 1;
      const b1 = newDoc.lineAt(toB).number - 1;
      const last = spans[spans.length - 1];
      if (last && a0 <= last[1]) {
        last[1] = Math.max(last[1], a1);
        last[3] = Math.max(last[3], b1);
      } else spans.push([a0, a1, b0, b1]);
    });
    let blocks = this.blocks;
    let shift = 0;
    for (const [a0, a1, b0, b1] of spans) {
      const fresh: number[] = [];
      for (let l = b0; l <= b1; l++) fresh.push(utf8Len(newDoc.line(l + 1).text));
      blocks = splice(blocks, a0 + shift, a1 + shift, fresh);
      shift += b1 - b0 - (a1 - a0);
    }
    return new ByteIndex(blocks);
  }

  /** The byte offset of line i (0-based). */
  private lineStart(i: number): number {
    const p = this.sums();
    let lo = 0;
    let hi = this.blocks.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (p.lines[mid] <= i) lo = mid;
      else hi = mid - 1;
    }
    const block = this.blocks[lo];
    let start = p.bytes[lo];
    for (let k = 0, n = i - p.lines[lo]; k < n; k++) start += block[k] + 1;
    return start;
  }

  private sums(): { lines: number[]; bytes: number[] } {
    if (this.pre) return this.pre;
    const n = this.blocks.length;
    const lines = new Array<number>(n + 1);
    const bytes = new Array<number>(n + 1);
    let l = 0;
    let b = 0;
    for (let k = 0; k < n; k++) {
      lines[k] = l;
      bytes[k] = b;
      const block = this.blocks[k];
      l += block.length;
      for (const x of block) b += x + 1;
    }
    lines[n] = l;
    bytes[n] = b;
    return (this.pre = { lines, bytes });
  }
}

/** Lines `a0..a1` (inclusive, 0-based) of `blocks` replaced by `fresh`: new blocks for the ones
 * touched, the others shared. */
function splice(blocks: readonly (readonly number[])[], a0: number, a1: number, fresh: number[]): (readonly number[])[] {
  // the blocks holding a0 and a1
  let first = 0;
  let line = 0;
  while (first < blocks.length - 1 && line + blocks[first].length <= a0) line += blocks[first++].length;
  const firstLine = line;
  let lastB = first;
  while (lastB < blocks.length - 1 && line + blocks[lastB].length <= a1) line += blocks[lastB++].length;
  const lastLine = line; // first line of block lastB
  const head = blocks[first].slice(0, a0 - firstLine);
  const tail = blocks[lastB].slice(a1 - lastLine + 1);
  let mid = [...head, ...fresh, ...tail];
  let from = first;
  let to = lastB;
  // a small result joins a neighbour, so blocks stay at least a quarter full
  if (mid.length < BLOCK / 4 && blocks.length > 1) {
    if (to + 1 < blocks.length) mid = [...mid, ...blocks[++to]];
    else if (from > 0) mid = [...blocks[--from], ...mid];
  }
  const out = blocks.slice(0, from);
  for (const c of chop(mid)) out.push(c);
  for (let k = to + 1; k < blocks.length; k++) out.push(blocks[k]);
  return out;
}

/** Lines in blocks of BLOCK (one block when they fit in twice that); always at least one block. */
function chop(lens: number[]): number[][] {
  if (lens.length <= 2 * BLOCK) return [lens];
  const out: number[][] = [];
  for (let at = 0; at < lens.length; at += BLOCK) {
    const rest = lens.length - at - BLOCK;
    if (rest > 0 && rest < BLOCK / 2) {
      out.push(lens.slice(at));
      break;
    }
    out.push(lens.slice(at, at + BLOCK));
  }
  return out;
}
