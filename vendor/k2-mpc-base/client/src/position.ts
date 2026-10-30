// How positions move through edits: a line-for-line port of the kit's
// `session/position.rs` (`transform`, `Bias`). Positions are UTF-8 byte offsets. The shared
// vectors (`test/vectors/transform.json`, written by the Rust kit) pin it.

import { utf8Len, type Hunk } from "./hunk";

/** Which way a position goes when text is inserted exactly at it. */
export const Bias = {
  /** The position stays where it is; the inserted text lands after it. */
  Before: "before",
  /** The position moves to the end of the inserted text (a caret pushed along by typing). */
  After: "after",
} as const;
export type Bias = (typeof Bias)[keyof typeof Bias];

/**
 * Where byte offset `pos` of the old text lands after `hunks` (sorted, non-overlapping, in
 * old-text coordinates) are applied:
 *
 * - before a hunk: unchanged;
 * - at the start of a replacement or deletion: unchanged (it stays at the start);
 * - strictly inside a replaced or deleted range: snaps to the start of the hunk;
 * - at the end of a replaced range, or after it: shifted by the size change;
 * - exactly at an insertion: per `bias`.
 */
export function transform(pos: number, hunks: readonly Hunk[], bias: Bias): number {
  let delta = 0;
  for (const h of hunks) {
    if (pos < h.start) break;
    const grow = utf8Len(h.insert) - (h.end - h.start);
    if (h.start === h.end) {
      if (pos === h.start && bias === Bias.Before) break;
      delta += grow;
      continue;
    }
    if (pos === h.start) break;
    if (pos < h.end) return h.start + delta;
    delta += grow;
  }
  return pos + delta;
}
