// The live grid as data, on the shared vector (`test/vectors/grid_view.tsv`, which the kit's rows
// read too): a `KeyedWatch` takes the workbook's snapshot and each rev's change, and at every rev
// the vector names, the grid helpers give its numbers and letters, its blank rows, its A1 and its
// refused corners from the watch's state and view schema. The watch keeps the schema's `grid` and
// `field_class_prefix`, and the formula text the snapshot's rows page and later changes carry.
//
// Plant caught by name: `GridGapIgnored` (`withGridGapIgnored`).

import { describe, expect, it } from "vitest";
import {
  blankNumbers,
  cellsA1,
  colLabel,
  colLetters,
  colNumber,
  colOfCellField,
  cornerA1,
  cornerAt,
  cornerNumbers,
  cornerProblem,
  fieldClass,
  gridNumbers,
  rangeA1,
  type CornerProblem,
} from "../src/grid";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ServerFrame, ViewSchemaJson, Welcome } from "../src/wire";
import { WORKBOOK_GRID, WORKBOOK_VIEW, changeFrame, readGridVector, snapshotFrames, wire, type GridVector } from "./grid-vector";
import { withGridGapIgnored } from "./plants";

const V = readGridVector();

const welcome: Welcome = { root: "rows", version: 1, client: 7, self: "person:ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

/** Replay the vector through a `KeyedWatch`: `check(rev, watch)` after the snapshot (at the
 * state's rev) and after each rev's change. Throws if the watch asked for a snapshot again. */
function replay(v: GridVector, check: (rev: number, w: KeyedWatch) => void): KeyedWatch {
  const { t, deliver } = wire();
  const w = new KeyedWatch(t, welcome);
  const asked: string[] = [];
  w.onResync((why, detail) => void (why !== "start" && asked.push(`${why}: ${detail}`)));
  const frames = snapshotFrames(v);
  for (const f of frames) deliver(f);
  if (asked.length || !w.ready) throw new Error(`the snapshot didn't take: ${asked[0] ?? "not ready"}`);
  check(v.rev, w);
  let seq = frames.length;
  for (const r of v.revs) {
    deliver(changeFrame(r, ++seq, w.state!));
    if (asked.length) throw new Error(`rev ${r.rev}: the watch asked for a snapshot: ${asked[0]}`);
    check(r.rev, w);
  }
  return w;
}

const numbersOf = (w: KeyedWatch, container: string) => gridNumbers(w.state!, w.view!.schema, container);

/** Every place the numbers, letters and blank rows differ from the vector's (empty: none). */
function numberDrift(v: GridVector): string[] {
  const drift: string[] = [];
  replay(v, (rev, w) => {
    for (const n of v.numbers.filter((x) => x.rev === rev)) {
      const got = numbersOf(w, n.container)[n.axis].map((l): [string, string] => [l.address, n.axis === "cols" ? colLabel(w.grid!, l.number) : String(l.number)]);
      if (JSON.stringify(got) !== JSON.stringify(n.lines)) drift.push(`rev ${rev} ${n.container} ${n.axis}: ${JSON.stringify(got)} for ${JSON.stringify(n.lines)}`);
    }
    for (const b of v.blankRows.filter((x) => x.rev === rev)) {
      const got = blankNumbers(numbersOf(w, b.container).rows, WORKBOOK_GRID.numbering.rows.first);
      if (JSON.stringify(got) !== JSON.stringify(b.rows)) drift.push(`rev ${rev} ${b.container} blank rows: ${JSON.stringify(got)} for ${JSON.stringify(b.rows)}`);
    }
  });
  return drift;
}

/** The vector's words for a refused corner, as the problems they may be. */
const WHY: Record<string, CornerProblem[]> = {
  "not in its form": ["row_form", "col_form"],
  "past the last column": ["cols_max"],
  "past the last row": ["rows_max"],
  "a row of another sheet": ["row"],
  "a column of another sheet": ["col"],
};

describe("the shared vector (test/vectors/grid_view.tsv)", () => {
  it("is there: a state, changes after it, numbers at several revs, corners, A1, an agent's cells and a claim", () => {
    expect(V.rev).toBe(20);
    expect(V.revs.length).toBeGreaterThanOrEqual(9);
    const revs = new Set(V.numbers.map((n) => n.rev));
    expect(revs.has(V.rev)).toBe(true);
    expect(revs.size).toBeGreaterThanOrEqual(3);
    expect(V.blankRows.length).toBeGreaterThan(0);
    expect(V.cellsA1.length + V.rangeA1.length + V.cornerA1.length).toBeGreaterThan(5);
    expect(V.cornerRefused.length).toBeGreaterThan(3);
    expect(V.claim).not.toBeNull();
    expect(V.a1.length).toBeGreaterThan(0);
    for (const r of V.cornerRefused) expect(WHY[r.why], `a refusal this suite doesn't map: ${r.why}`).toBeDefined();
  });

  it("the numbers and letters, and the blank rows, at every rev it names", () => {
    expect(numberDrift(V)).toEqual([]);
  });

  it("every cells_a1, range_a1 and corner_a1 gives its A1 at its rev's state", () => {
    const wrong: string[] = [];
    replay(V, (rev, w) => {
      for (const c of V.cellsA1.filter((x) => x.rev === rev)) {
        const got = cellsA1(numbersOf(w, c.cells.container), c.cells);
        const want = { active: c.active, ranges: c.ranges };
        if (JSON.stringify(got) !== JSON.stringify(want)) wrong.push(`${c.id} at ${rev}: ${JSON.stringify(got)} for ${JSON.stringify(want)}`);
      }
      for (const r of V.rangeA1.filter((x) => x.rev === rev)) {
        const got = rangeA1(numbersOf(w, r.container), r.range);
        if (got !== r.a1) wrong.push(`${r.id} at ${rev}: ${got} for ${r.a1}`);
      }
      for (const c of V.cornerA1.filter((x) => x.rev === rev)) {
        const n = numbersOf(w, c.container);
        const got = cornerA1(n, c.corner);
        if (got !== c.a1) wrong.push(`${c.id} at ${rev}: ${got} for ${c.a1}`);
        // the cell's numbers give the same corner back, in normal form
        const at = cornerNumbers(n, c.corner);
        if (!at || JSON.stringify(cornerAt(n, at.row, at.col)) !== JSON.stringify(c.corner)) wrong.push(`${c.id} at ${rev}: cornerAt doesn't give the corner back`);
      }
    });
    expect(wrong).toEqual([]);
  });

  it("every corner_refused is refused, for its reason, and has no A1", () => {
    const wrong: string[] = [];
    replay(V, (rev, w) => {
      for (const c of V.cornerRefused.filter((x) => x.rev === rev)) {
        const n = numbersOf(w, c.container);
        const why = cornerProblem(n, c.corner);
        if (why === null || !WHY[c.why]?.includes(why)) wrong.push(`${JSON.stringify(c.corner)} at ${rev}: ${why} for "${c.why}"`);
        if (cornerA1(n, c.corner) !== null || cornerNumbers(n, c.corner) !== null) wrong.push(`${JSON.stringify(c.corner)} at ${rev}: has an A1`);
      }
    });
    expect(wrong).toEqual([]);
  });

  it("the watch keeps the schema's grid and field_class_prefix through every change", () => {
    const w = replay(V, (_rev, w) => {
      expect(w.grid).toEqual(WORKBOOK_GRID);
      expect(w.view!.schema.field_class_prefix).toEqual([["c", "cell"]]);
    });
    expect(w.rev).toBe(V.revs.at(-1)!.rev);
  });

  it("plant GridGapIgnored (every gap read as 0) is caught", () => {
    expect(numberDrift(V)).toEqual([]);
    expect(withGridGapIgnored(() => numberDrift(V)).length).toBeGreaterThan(0);
  });
});

describe("the formula text kept by cell field (the grid's cell_derived)", () => {
  it("is there right after the snapshot's rows page, before any change; a change's derived.units sets and removes it", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    const frames = snapshotFrames(V);
    const rowsPage = frames.find((f) => f.type === "keyed_page" && f.root === WORKBOOK_GRID.cells);
    expect(rowsPage && rowsPage.type === "keyed_page" && rowsPage.derived?.a1).toBeTruthy();
    for (const f of frames) deliver(f);
    expect(w.ready).toBe(true);
    for (const [unit, field, text] of V.a1) expect(w.cellDerived("a1", unit, field), `${unit} ${field}`).toBe(text);
    expect(w.cellDerived("a1", "@sheet:1/row:r3", "c3")).toBeNull();
    const [unit, field] = V.a1[0];
    const change = (seq: number, rev: number, derived: unknown): ServerFrame => {
      const f = changeFrame({ rev, verb: "cell set", ops: [], agentCells: null, claimLines: null }, seq, w.state!);
      return { ...f, derived } as ServerFrame;
    };
    deliver(change(frames.length + 1, V.rev + 1, { units: { [unit]: { a1: { [field]: "=C4+D4+1", c9: "=A1" }, other: { [field]: "x" } } } }));
    expect(w.cellDerived("a1", unit, field)).toBe("=C4+D4+1");
    expect(w.cellDerived("a1", unit, "c9")).toBe("=A1");
    // a name the grid doesn't keep by cell field isn't kept
    expect(w.cellDerived("other", unit, field)).toBeNull();
    deliver(change(frames.length + 2, V.rev + 2, { units: { [unit]: { a1: { [field]: null } } } }));
    expect(w.cellDerived("a1", unit, field)).toBeNull();
    expect(w.cellDerived("a1", unit, "c9")).toBe("=A1");
    // a fresh snapshot replaces them all
    for (const f of snapshotFrames(V)) deliver(f);
    expect(w.cellDerived("a1", unit, field)).toBe(V.a1[0][2]);
    expect(w.cellDerived("a1", unit, "c9")).toBeNull();
  });
});

describe("the grid helpers", () => {
  it("column letters both ways", () => {
    const cases: [number, string][] = [
      [1, "A"],
      [2, "B"],
      [26, "Z"],
      [27, "AA"],
      [52, "AZ"],
      [53, "BA"],
      [702, "ZZ"],
      [703, "AAA"],
      [16384, "XFD"],
    ];
    for (const [n, s] of cases) {
      expect(colLetters(n)).toBe(s);
      expect(colNumber(s)).toBe(n);
    }
    expect(colNumber("xfd")).toBe(16384);
    for (const bad of [0, -1, 1.5, Number.NaN]) expect(colLetters(bad)).toBe("");
    for (const bad of ["", "A1", "Ä", " A"]) expect(colNumber(bad)).toBeNull();
    expect(colLabel({ ...WORKBOOK_GRID, col_label: "numbers" }, 27)).toBe("27");
    expect(colLabel(WORKBOOK_GRID, 27)).toBe("AA");
  });

  it("cornerAt gives the corner in normal form, and nothing out of bounds", () => {
    replay(V, (rev, w) => {
      if (rev !== V.rev) return;
      const n = numbersOf(w, "@sheet:1");
      // a blank row (2) is past the row before it; a keyed row is its own, offset 0
      expect(cornerAt(n, 2, 1)).toEqual({ row: "@sheet:1/row:r1", row_off: 1, col: "@sheet:1/col:c1", col_off: 0 });
      expect(cornerAt(n, 3, 3)).toEqual({ row: "@sheet:1/row:r2", row_off: 0, col: "@sheet:1/col:c3", col_off: 0 });
      expect(cornerAt(n, 1048576, 16384)).toEqual({ row: "@sheet:1/row:r18", row_off: 1048556, col: "@sheet:1/col:c6", col_off: 16378 });
      for (const [r, c] of [
        [0, 1],
        [1, 0],
        [1048577, 1],
        [1, 16385],
        [1.5, 1],
      ])
        expect(cornerAt(n, r, c)).toBeNull();
      // every cell of the first 25 rows and 8 columns: back to its numbers, in normal form
      for (let r = 1; r <= 25; r++) {
        for (let c = 1; c <= 8; c++) {
          const k = cornerAt(n, r, c)!;
          expect(cornerProblem(n, k)).toBeNull();
          expect(cornerNumbers(n, k)).toEqual({ row: r, col: c });
        }
      }
      // past a sheet's last row and column: offsets from them
      expect(cornerAt(numbersOf(w, "@sheet:4"), 3, 5)).toEqual({ row: "@sheet:4/row:r1", row_off: 2, col: "@sheet:4/col:c2", col_off: 3 });
      expect(cornerProblem(n, { row: null, row_off: 0, col: null, col_off: 1 })).toBe("row_form");
      expect(cornerProblem(n, { row: "@sheet:1/row:r1", row_off: -1, col: null, col_off: 1 })).toBe("bad");
    });
  });

  it("a cell field's column, and a row field's none", () => {
    expect(colOfCellField(WORKBOOK_GRID, "@sheet:1", "c3")).toBe("@sheet:1/col:c3");
    expect(colOfCellField(WORKBOOK_GRID, "@sheet:2", "c12")).toBe("@sheet:2/col:c12");
    expect(colOfCellField(WORKBOOK_GRID, "@sheet:1", "@ht")).toBeNull();
    // the template's names are replaced once each, whatever the values hold
    expect(colOfCellField({ ...WORKBOOK_GRID, col_of_cell_field: "{field}@{container}" }, "{field}", "$&")).toBe("$&@{field}");
  });

  it("a field's class: exact first, then by prefix, then the hidden field, else meta", () => {
    const vs: ViewSchemaJson = { ...WORKBOOK_VIEW, field_class: { "@ht": "style", c0: "meta" }, geometry: { field: "cgeo", parts: ["x"], rot_unit: "deg" } };
    expect(fieldClass(vs, "c3")).toBe("cell");
    expect(fieldClass(vs, "c0")).toBe("meta");
    expect(fieldClass(vs, "@ht")).toBe("style");
    expect(fieldClass(vs, "@hidden")).toBe("visibility");
    expect(fieldClass(vs, "hidden", "container")).toBe("visibility");
    expect(fieldClass(vs, "hidden")).toBe("meta");
    expect(fieldClass(vs, "cgeo")).toBe("geometry");
    expect(fieldClass(vs, "@sheet")).toBe("meta");
    expect(fieldClass(vs, "constructor")).toBe("cell");
    // the first prefix that matches wins; an older kit's schema has none
    expect(fieldClass({ ...vs, field_class_prefix: [["cx", "text"], ["c", "cell"]] }, "cx1")).toBe("text");
    const { field_class_prefix: _, ...older } = vs;
    expect(fieldClass(older, "c3")).toBe("meta");
  });
});
