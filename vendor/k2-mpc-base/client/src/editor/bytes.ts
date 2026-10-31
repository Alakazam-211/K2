// UTF-16 <-> UTF-8 offsets on a CodeMirror document without flattening it. The index keeps
// each line's length in UTF-8 bytes and updates only the lines a transaction touches; prefix
// sums are rebuilt lazily from the first changed line, and only as far as a lookup needs. A
// keystroke costs the changed lines plus a walk of integers, never `doc.toString()`.

import type { ChangeSet, Text } from "@codemirror/state";
import { byteToIndex, utf8Len } from "../hunk";

export class ByteIndex {
  /** UTF-8 bytes of each line, without its line break. */
  private readonly lens: number[];
  /** `starts[i]`: the byte offset of line i (0-based), valid for i < `valid`. */
  private readonly starts: number[];
  private valid: number;

  private constructor(lens: number[], starts: number[], valid: number) {
    this.lens = lens;
    this.starts = starts;
    this.valid = valid;
  }

  static of(doc: Text): ByteIndex {
    const lens: number[] = [];
    for (const it = doc.iterLines(); !it.next().done; ) lens.push(utf8Len(it.value));
    return new ByteIndex(lens, [0], 1);
  }

  /** The document's length in UTF-8 bytes. */
  total(): number {
    const n = this.lens.length;
    return this.lineStart(n - 1) + this.lens[n - 1];
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
    if (byte < 0) return -1;
    // the last line starting at or before `byte`
    let lo = 0;
    let hi = this.lens.length - 1;
    if (byte > this.total()) return -1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.lineStart(mid) <= byte) lo = mid;
      else hi = mid - 1;
    }
    const into = byte - this.lineStart(lo);
    if (into > this.lens[lo]) return -1; // inside the line break (a single byte: never)
    const line = doc.line(lo + 1);
    if (into === 0) return line.from;
    const i = byteToIndex(line.text, into);
    return i < 0 ? -1 : line.from + i;
  }

  /** The index of `newDoc`, after `changes` turned `oldDoc` into it. */
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
    const lens = this.lens.slice();
    let shift = 0;
    for (const [a0, a1, b0, b1] of spans) {
      const fresh: number[] = [];
      for (let l = b0; l <= b1; l++) fresh.push(utf8Len(newDoc.line(l + 1).text));
      lens.splice(a0 + shift, a1 - a0 + 1, ...fresh);
      shift += b1 - b0 - (a1 - a0);
    }
    const first = spans[0][0];
    const valid = Math.min(this.valid, first + 1);
    return new ByteIndex(lens, this.starts.slice(0, valid), valid);
  }

  private lineStart(i: number): number {
    while (this.valid <= i) {
      const k = this.valid;
      this.starts[k] = this.starts[k - 1] + this.lens[k - 1] + 1;
      this.valid++;
    }
    return this.starts[i];
  }
}
