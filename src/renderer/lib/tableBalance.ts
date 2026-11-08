// Lifted unchanged from the K2 Companion repo (K2SO-companion,
// src/lib/tableBalance.ts), commit 71bc924 on branch release-4.2.1.
// Keep the two copies in sync: change it there first, then copy it here
// below this header. The desktop wrapper is components/common/BalancedTable.tsx.

// Column widths that make a markdown table as short as possible (Rosson,
// 2026-11-07): the browser's auto layout hands the room to the column with
// the most text, so short columns ("Status", "on main, pushed") wrap into
// tall rows. Here every split of the width is scored by the table's height
// in lines (each row is as tall as its tallest cell), even if one row ends
// up awkward.
//
// Pure: no DOM, no imports. The caller measures each cell's words (canvas)
// and applies the widths. K2's desktop lifts this file as is
// (src/renderer/lib/), so keep it free of app code.

/** One cell, measured in the font it's drawn in. */
export type BalanceCell = {
  /** Width of each run that can't break (a word, with any punctuation or
   *  inline code glued to it), px. */
  words: number[];
  /** Width of a space, px. */
  space: number;
  /** Horizontal padding and borders, px. */
  pad: number;
};

export type BalanceResult =
  /** Every cell fits on one line: leave the browser's layout alone. */
  | { kind: "natural" }
  /** Even the narrowest widths don't fit: let the table scroll sideways. */
  | { kind: "scroll" }
  /** Too big to balance, or nothing to balance: keep the browser's layout. */
  | { kind: "skip" }
  /** Column widths (px, summing to the available width) and the table's
   *  height in lines. */
  | { kind: "fixed"; widths: number[]; lines: number };

export type BalanceOptions = {
  /** Narrowest column, px (the CSS's `min-width: 4ch`). */
  minColumn?: number;
  /** Width step when looking for where a column's lines change, px. */
  step?: number;
  maxColumns?: number;
  maxRows?: number;
  /** Upper bound on cell evaluations; past it the search keeps the best
   *  split found so far. */
  maxWork?: number;
};

export const BALANCE_MAX_COLUMNS = 12;
export const BALANCE_MAX_ROWS = 200;

const EMPTY: BalanceCell = { words: [], space: 0, pad: 0 };
/** Canvas and layout round differently: a line that fits within this much
 *  still fits. */
const FIT_SLACK = 0.5;

/** Lines a cell takes at `width` (content box), breaking only between words. */
export function wrapLines(words: number[], space: number, width: number): number {
  let lines = 1;
  let x = -1;
  for (const w of words) {
    if (x < 0) x = w;
    else if (x + space + w <= width + FIT_SLACK) x += space + w;
    else {
      lines++;
      x = w;
    }
  }
  return lines;
}

/** A column width at which the column's line counts change, with them. */
type Stop = { w: number; lines: number[] };

type Score = { height: number; total: number; width: number };

function better(a: Score, b: Score | null): boolean {
  if (!b) return true;
  if (a.height !== b.height) return a.height < b.height;
  if (a.total !== b.total) return a.total < b.total;
  return a.width < b.width;
}

export function balanceColumns(
  rows: BalanceCell[][],
  available: number,
  opts: BalanceOptions = {}
): BalanceResult {
  const minColumn = opts.minColumn ?? 32;
  const step = Math.max(1, opts.step ?? 2);
  const maxWork = opts.maxWork ?? 2_000_000;
  const cols = rows.reduce((n, r) => Math.max(n, r.length), 0);
  if (!rows.length || !cols || !(available > 0)) return { kind: "skip" };
  if (cols > (opts.maxColumns ?? BALANCE_MAX_COLUMNS) || rows.length > (opts.maxRows ?? BALANCE_MAX_ROWS)) {
    return { kind: "skip" };
  }
  const cell = (r: number, c: number) => rows[r][c] ?? EMPTY;

  const minW: number[] = [];
  const natW: number[] = [];
  for (let c = 0; c < cols; c++) {
    let lo = minColumn;
    let hi = minColumn;
    for (let r = 0; r < rows.length; r++) {
      const x = cell(r, c);
      let longest = 0;
      let line = 0;
      x.words.forEach((w, i) => {
        longest = Math.max(longest, w);
        line += w + (i ? x.space : 0);
      });
      lo = Math.max(lo, Math.ceil(longest + x.pad));
      hi = Math.max(hi, Math.ceil(line + x.pad));
    }
    minW.push(lo);
    natW.push(Math.max(lo, hi));
  }
  const sum = (a: number[]) => a.reduce((s, v) => s + v, 0);
  if (sum(natW) <= available) return { kind: "natural" };
  const sumMin = sum(minW);
  if (sumMin > available) return { kind: "scroll" };

  // Each column's stops: the widths (from its narrowest up to what the
  // others leave it) where one of its cells loses a line.
  const stops: Stop[][] = [];
  for (let c = 0; c < cols; c++) {
    const cap = Math.min(natW[c], available - (sumMin - minW[c]));
    const list: Stop[] = [];
    let prev: number[] | null = null;
    for (let w = minW[c]; ; w = Math.min(w + step, cap)) {
      const lines = rows.map((_, r) => {
        const x = cell(r, c);
        return wrapLines(x.words, x.space, w - x.pad);
      });
      if (!prev || lines.some((n, r) => n !== prev![r])) list.push({ w, lines });
      prev = lines;
      if (w >= cap) break;
    }
    stops.push(list);
  }

  let work = 0;
  const score = (pick: number[]): Score => {
    work += rows.length * cols;
    let height = 0;
    let total = 0;
    for (let r = 0; r < rows.length; r++) {
      let tallest = 0;
      for (let c = 0; c < cols; c++) {
        const n = stops[c][pick[c]].lines[r];
        total += n;
        if (n > tallest) tallest = n;
      }
      height += tallest;
    }
    let width = 0;
    for (let c = 0; c < cols; c++) width += stops[c][pick[c]].w;
    return { height, total, width };
  };

  let best: Score | null = null;
  let bestPick: number[] = stops.map(() => 0);
  const combos = stops.reduce((n, s) => n * s.length, 1);
  if (combos * rows.length * cols <= maxWork) {
    // Small enough to try every split. Ties keep the first one found
    // (narrower early columns), so the same table always lands the same way.
    const pick = stops.map(() => 0);
    const restMin: number[] = [];
    for (let c = cols - 1, acc = 0; c >= 0; c--) {
      restMin[c] = acc;
      acc += minW[c];
    }
    const walk = (c: number, used: number) => {
      if (c === cols) {
        const s = score(pick);
        if (better(s, best)) {
          best = s;
          bestPick = pick.slice();
        }
        return;
      }
      for (let k = 0; k < stops[c].length; k++) {
        const w = stops[c][k].w;
        if (used + w + restMin[c] > available) break;
        pick[c] = k;
        walk(c + 1, used + w);
      }
      pick[c] = 0;
    };
    walk(0, 0);
  } else {
    // Big tables: start narrow and keep making the move (one column, or two
    // together when a row needs both) that saves the most lines per pixel.
    const pick = stops.map(() => 0);
    best = score(pick);
    const LOOKAHEAD = 6;
    for (;;) {
      if (work > maxWork) break;
      const cur: Score = best!;
      let move: { pick: number[]; s: Score; rate: number } | null = null;
      const consider = (next: number[]) => {
        const s = score(next);
        if (s.width > available) return;
        const gain = cur.height - s.height || (cur.total - s.total) / 1000;
        if (gain <= 0) return;
        const rate = gain / Math.max(1, s.width - cur.width);
        if (!move || rate > move.rate || (rate === move.rate && better(s, move.s))) {
          move = { pick: next, s, rate };
        }
      };
      for (let c = 0; c < cols; c++) {
        for (let k = pick[c] + 1; k < Math.min(stops[c].length, pick[c] + 1 + LOOKAHEAD); k++) {
          const next = pick.slice();
          next[c] = k;
          consider(next);
        }
      }
      if (!move) {
        for (let a = 0; a < cols && !move; a++) {
          for (let b = a + 1; b < cols; b++) {
            if (pick[a] + 1 >= stops[a].length || pick[b] + 1 >= stops[b].length) continue;
            const next = pick.slice();
            next[a]++;
            next[b]++;
            consider(next);
          }
        }
      }
      if (!move) break;
      const chosen: { pick: number[]; s: Score } = move;
      pick.splice(0, cols, ...chosen.pick);
      best = chosen.s;
      bestPick = pick.slice();
    }
  }

  // The width left over goes to the columns still short of one line per
  // cell, in proportion to what they lack; it changes no line counts, so a
  // small resize only moves these shares.
  const widths = bestPick.map((k, c) => stops[c][k].w);
  const extra = available - sum(widths);
  const lack = widths.map((w, c) => Math.max(0, natW[c] - w));
  const lackSum = sum(lack);
  const out = widths.map((w, c) => w + (lackSum > 0 ? (extra * lack[c]) / lackSum : extra / cols));
  return { kind: "fixed", widths: out, lines: best ? (best as Score).height : 0 };
}
