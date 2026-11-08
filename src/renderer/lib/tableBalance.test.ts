// Lifted unchanged from the K2 Companion repo (K2SO-companion,
// src/lib/tableBalance.test.ts), commit 71bc924 on branch release-4.2.1.
// Keep the two copies in sync: change it there first, then copy it here
// below this header. The desktop wrapper is components/common/BalancedTable.tsx.

import { describe, expect, it } from "vitest";
import { balanceColumns, wrapLines, type BalanceCell } from "./tableBalance";

// A fixed-width "font": every character is 7px, a space 4px, cells pad 16px.
const CH = 7;
const PAD = 16;
const cell = (text: string): BalanceCell => ({
  words: text.split(/\s+/).filter(Boolean).map((w) => w.length * CH),
  space: 4,
  pad: PAD,
});
const table = (rows: string[][]) => rows.map((r) => r.map(cell));

/** Lines each cell takes at the given widths: the layout the widths make. */
function heights(rows: BalanceCell[][], widths: number[]): number {
  return rows.reduce(
    (h, r) => h + Math.max(...r.map((x, c) => wrapLines(x.words, x.space, widths[c] - x.pad))),
    0
  );
}

// K2's 0.46.10 sitrep (Rosson's screenshot): a long middle column between
// two short ones.
const SITREP = table([
  ["", "Item", "Status"],
  ["✅", "Add-workspace fix, passports, DB fixes, App doors, split view default, pg_restore, GitHub setup card, signer hardening", "on main, pushed"],
  ["✅", "Client CI pieces, off by default (ciPreview)", "on main, pushed"],
  ["✅", "Voice tunnel move clears direct mode's leftovers", "on main, pushed"],
  ["⏳", "Sew exit 78 is config refused", "building"],
]);

describe("wrapLines", () => {
  it("breaks only between words", () => {
    expect(wrapLines([], 4, 10)).toBe(1);
    expect(wrapLines([30, 30], 4, 64)).toBe(1);
    expect(wrapLines([30, 30], 4, 63)).toBe(2);
    // A word wider than the line overflows on its own line; it never splits.
    expect(wrapLines([100, 10], 4, 50)).toBe(2);
  });
});

describe("balanceColumns", () => {
  it("keeps the short Status column on as few lines as the table allows (sitrep)", () => {
    const r = balanceColumns(SITREP, 330);
    expect(r.kind).toBe("fixed");
    if (r.kind !== "fixed") return;
    expect(r.widths.reduce((a, b) => a + b, 0)).toBeCloseTo(330, 5);
    // "on main, pushed" never stacks letter by letter: Status is at least
    // as wide as "pushed" plus padding.
    expect(r.widths[2]).toBeGreaterThanOrEqual(6 * CH + PAD);
    // Shorter than the browser's habit of giving Item everything it can
    // (Status and the tick at their narrowest).
    const greedy = [Math.max(32, 2 * CH + PAD), 0, 7 * CH + PAD];
    greedy[1] = 330 - greedy[0] - greedy[2];
    expect(r.lines).toBeLessThanOrEqual(heights(SITREP, greedy));
    expect(heights(SITREP, r.widths)).toBe(r.lines);
  });

  it("finds the shortest table among every split", () => {
    const rows = SITREP;
    const r = balanceColumns(rows, 300, { step: 1 });
    if (r.kind !== "fixed") throw new Error(r.kind);
    // Brute force over whole-pixel splits.
    let best = Infinity;
    for (let a = 32; a <= 300; a++) {
      for (let b = 32; a + b <= 300 - 32; b++) {
        const c = 300 - a - b;
        const w = [a, b, c];
        if (w.some((x, i) => x < Math.max(32, ...rows.map((row) => Math.max(0, ...row[i].words) + PAD)))) continue;
        best = Math.min(best, heights(rows, w));
      }
    }
    expect(r.lines).toBe(best);
  });

  it("accepts one awkward row when that makes the whole table shorter", () => {
    // Nine rows want a wide second column; one row's long first cell would
    // rather have the room.
    const rows = table([
      ["a", "status words here"],
      ...Array.from({ length: 9 }, () => ["x", "on main, pushed to origin"]),
      ["a long first cell that would like to be wide please", "ok"],
    ]);
    const r = balanceColumns(rows, 260);
    if (r.kind !== "fixed") throw new Error(r.kind);
    // The nine rows sit on one line each; the long row takes the wrapping.
    const second = rows[1][1];
    expect(wrapLines(second.words, second.space, r.widths[1] - PAD)).toBe(1);
    const long = rows[10][0];
    expect(wrapLines(long.words, long.space, r.widths[0] - PAD)).toBeGreaterThan(2);
  });

  it("leaves a table that fits on one line per cell to the browser", () => {
    expect(balanceColumns(table([["a", "b"], ["c", "d"]]), 300)).toEqual({ kind: "natural" });
  });

  it("scrolls when even the narrowest columns don't fit", () => {
    const rows = table([["supercalifragilistic", "antidisestablishment", "floccinaucinihilipilification"]]);
    expect(balanceColumns(rows, 200)).toEqual({ kind: "scroll" });
  });

  it("skips tables past the size guard and empty input", () => {
    const wide = table([Array.from({ length: 13 }, () => "word word")]);
    expect(balanceColumns(wide, 300).kind).toBe("skip");
    const tall = table(Array.from({ length: 201 }, () => ["word word word", "x"]));
    expect(balanceColumns(tall, 100).kind).toBe("skip");
    expect(balanceColumns([], 300).kind).toBe("skip");
    expect(balanceColumns(SITREP, 0).kind).toBe("skip");
  });

  it("is deterministic and doesn't jump between layouts on a 1px resize", () => {
    const a = balanceColumns(SITREP, 330);
    expect(balanceColumns(SITREP, 330)).toEqual(a);
    const b = balanceColumns(SITREP, 331);
    if (a.kind !== "fixed" || b.kind !== "fixed") throw new Error("not fixed");
    expect(b.lines).toBe(a.lines);
    b.widths.forEach((w, i) => expect(Math.abs(w - a.widths[i])).toBeLessThanOrEqual(1));
  });

  it("falls back to the greedy search past the work cap and still beats auto layout", () => {
    const r = balanceColumns(SITREP, 330, { maxWork: 1 });
    if (r.kind !== "fixed") throw new Error(r.kind);
    expect(r.widths.reduce((s, w) => s + w, 0)).toBeCloseTo(330, 5);
    const full = balanceColumns(SITREP, 330);
    if (full.kind !== "fixed") throw new Error(full.kind);
    expect(r.lines).toBeGreaterThanOrEqual(full.lines);
  });

  it("handles ragged rows (missing cells count as empty)", () => {
    const rows = [[cell("one two three four five six seven"), cell("x")], [cell("y")]];
    const r = balanceColumns(rows, 120);
    expect(r.kind).toBe("fixed");
  });
});
