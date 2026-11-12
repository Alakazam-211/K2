// The live grid's shared vector (`test/vectors/grid_view.tsv`, written for both the kit's rows and
// the client's), read: the workbook's keyed state at its first rev, each later rev's ops and what
// the agent's cells are after it, the cells claim, and the numbers, blank rows, A1 and refused
// corners the rows check. Tab-separated; `#` lines are comments; a corner is
// `row,row_off,col,col_off` (`-` for none), a range `corner:corner`, ranges joined by `;`. Every
// line kind is read or the reader throws: nothing is skipped by accident.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { CellRange, Cells, ClientFrame, Corner, GridSlot, KeyedSchema, ServerFrame, Transport, ViewSchemaJson, WatchOp } from "../src/wire";
import type { KeyedState } from "../src/keyed";

export const VECTOR_PATH = join(dirname(fileURLToPath(import.meta.url)), "vectors", "grid_view.tsv");

/** One op of a rev, as the vector writes it. */
export type VectorOp =
  | { kind: "map"; root: string; address: string; field: string; op: "set"; value: string }
  | { kind: "map"; root: string; address: string; field: string; op: "delete" }
  | { kind: "list"; root: string; address: string; op: "insert"; after: string | null; value: string };

export interface VectorRev {
  rev: number;
  /** The verb that made it, `null` for a change no verb made (`-`). */
  verb: string | null;
  ops: VectorOp[];
  /** The agent's cells after it, when the vector says. */
  agentCells: Cells | null;
  /** The cells claim's rows when it was released at this rev. */
  claimLines: { first: number; last: number } | null;
}

export interface GridVector {
  rev: number;
  lists: [string, string, string][];
  maps: [string, string, string, string][];
  /** The formula cells' A1 text at the first rev: row address, field, text. */
  a1: [string, string, string][];
  revs: VectorRev[];
  claim: { container: string; range: CellRange; note: string; hold: boolean } | null;
  claimCells: Cells | null;
  numbers: { rev: number; container: string; axis: "rows" | "cols"; lines: [string, string][] }[];
  blankRows: { rev: number; container: string; rows: number[] }[];
  cellsA1: { rev: number; id: string; cells: Cells; active: string | null; ranges: string[] }[];
  rangeA1: { rev: number; id: string; container: string; range: CellRange; a1: string }[];
  cornerA1: { rev: number; id: string; container: string; corner: Corner; a1: string }[];
  cornerRefused: { rev: number; id: string; container: string; corner: Corner; why: string }[];
}

const none = (s: string) => (s === "-" ? null : s);

function int(s: string, what: string): number {
  if (!/^\d+$/.test(s)) throw new Error(`grid_view.tsv: ${what} is not a number: ${s}`);
  return Number(s);
}

function corner4(row: string, rowOff: string, col: string, colOff: string): Corner {
  return { row: none(row), row_off: int(rowOff, "row_off"), col: none(col), col_off: int(colOff, "col_off") };
}

export function parseCorner(s: string): Corner {
  const p = s.split(",");
  if (p.length !== 4) throw new Error(`grid_view.tsv: not a corner: ${s}`);
  return corner4(p[0], p[1], p[2], p[3]);
}

/** A range: seven comma-separated parts, the fourth `<col_off>:<row>` (addresses hold `:`). */
export function parseRange(s: string): CellRange {
  const p = s.split(",");
  const i = p.length === 7 ? p[3].indexOf(":") : -1;
  if (i < 0) throw new Error(`grid_view.tsv: not a range: ${s}`);
  return { start: corner4(p[0], p[1], p[2], p[3].slice(0, i)), end: corner4(p[3].slice(i + 1), p[4], p[5], p[6]) };
}

const parseRanges = (s: string) => (s === "-" ? [] : s.split(";").map(parseRange));

function parseCells(container: string, active: string, ranges: string): Cells {
  return { container, active: active === "-" ? null : parseCorner(active), ranges: parseRanges(ranges) };
}

export function readGridVector(text = readFileSync(VECTOR_PATH, "utf8")): GridVector {
  const v: GridVector = { rev: -1, lists: [], maps: [], a1: [], revs: [], claim: null, claimCells: null, numbers: [], blankRows: [], cellsA1: [], rangeA1: [], cornerA1: [], cornerRefused: [] };
  let cur: VectorRev | null = null;
  for (const line of text.split("\n")) {
    if (line === "" || line.startsWith("#")) continue;
    const f = line.split("\t");
    const need = (n: number) => {
      if (f.length !== n) throw new Error(`grid_view.tsv: ${n} fields wanted: ${line}`);
    };
    switch (f[0]) {
      case "state":
        need(2);
        v.rev = int(f[1], "state");
        break;
      case "list":
        need(4);
        v.lists.push([f[1], f[2], f[3]]);
        break;
      case "map":
        need(5);
        v.maps.push([f[1], f[2], f[3], f[4]]);
        break;
      case "rev":
        need(3);
        cur = { rev: int(f[1], "rev"), verb: none(f[2]), ops: [], agentCells: null, claimLines: null };
        v.revs.push(cur);
        break;
      case "op": {
        if (!cur) throw new Error(`grid_view.tsv: an op before any rev: ${line}`);
        if (f[1] === "map" && f[5] === "set") {
          need(7);
          cur.ops.push({ kind: "map", root: f[2], address: f[3], field: f[4], op: "set", value: f[6] });
        } else if (f[1] === "map" && f[5] === "delete") {
          need(6);
          cur.ops.push({ kind: "map", root: f[2], address: f[3], field: f[4], op: "delete" });
        } else if (f[1] === "list" && f[4] === "insert") {
          need(7);
          cur.ops.push({ kind: "list", root: f[2], address: f[3], op: "insert", after: none(f[5]), value: f[6] });
        } else throw new Error(`grid_view.tsv: an op this reader doesn't know: ${line}`);
        break;
      }
      case "report":
        // what the verb reports: the kit's rows check it
        break;
      case "agent_cells":
        need(4);
        if (!cur) throw new Error(`grid_view.tsv: agent_cells before any rev: ${line}`);
        cur.agentCells = parseCells(f[1], f[2], f[3]);
        break;
      case "claim_lines":
        need(3);
        if (!cur) throw new Error(`grid_view.tsv: claim_lines before any rev: ${line}`);
        cur.claimLines = { first: int(f[1], "first"), last: int(f[2], "last") };
        break;
      case "a1":
        need(4);
        v.a1.push([f[1], f[2], f[3]]);
        break;
      case "claim":
        need(5);
        v.claim = { container: f[1], range: parseRange(f[2]), note: f[3], hold: f[4] === "hold" };
        break;
      case "claim_cells":
        need(4);
        v.claimCells = parseCells(f[1], f[2], f[3]);
        break;
      case "numbers": {
        need(5);
        const lines = f[4] === "-" ? [] : f[4].split(",").map((p): [string, string] => [p.slice(0, p.lastIndexOf("=")), p.slice(p.lastIndexOf("=") + 1)]);
        v.numbers.push({ rev: int(f[1], "rev"), container: f[2], axis: f[3] === "cols" ? "cols" : "rows", lines });
        break;
      }
      case "blank_rows":
        need(4);
        v.blankRows.push({ rev: int(f[1], "rev"), container: f[2], rows: f[3] === "-" ? [] : f[3].split(",").map((n) => int(n, "blank row")) });
        break;
      case "cells_a1":
        need(8);
        v.cellsA1.push({ rev: int(f[1], "rev"), id: f[2], cells: parseCells(f[3], f[4], f[5]), active: none(f[6]), ranges: f[7] === "-" ? [] : f[7].split(",") });
        break;
      case "range_a1":
        need(6);
        v.rangeA1.push({ rev: int(f[1], "rev"), id: f[2], container: f[3], range: parseRange(f[4]), a1: f[5] });
        break;
      case "corner_a1":
        need(6);
        v.cornerA1.push({ rev: int(f[1], "rev"), id: f[2], container: f[3], corner: parseCorner(f[4]), a1: f[5] });
        break;
      case "corner_refused":
        need(6);
        v.cornerRefused.push({ rev: int(f[1], "rev"), id: f[2], container: f[3], corner: parseCorner(f[4]), why: f[5] });
        break;
      default:
        throw new Error(`grid_view.tsv: a line this reader doesn't know: ${line}`);
    }
  }
  if (v.rev < 0) throw new Error("grid_view.tsv: no state line");
  return v;
}

/** The workbook's model, as its pack declares it: the sheets, the columns and the rows, each
 * list with its composite map, and the book's own map. Test data, the pack's names. */
export const WORKBOOK_SCHEMA: KeyedSchema = {
  name: "workbook",
  roots: [
    { name: "sheets", kind: "list" },
    { name: "sheet_props", kind: "map" },
    { name: "cols", kind: "list" },
    { name: "col_props", kind: "map" },
    { name: "rows", kind: "list" },
    { name: "cells", kind: "map" },
    { name: "book", kind: "map" },
  ],
  composite: [
    { map: "sheet_props", list: "sheets" },
    { map: "col_props", list: "cols" },
    { map: "cells", list: "rows" },
  ],
};

/** The view schema's grid, as SPEC §1a gives it, with the derived values kept by cell field. */
export const WORKBOOK_GRID: GridSlot = {
  rows: "rows",
  cols: "cols",
  cells: "cells",
  col_fields: "col_props",
  col_container_field: "sheet",
  row_fields_start: "@",
  col_of_cell_field: "{container}/col:{field}",
  numbering: { rows: { gap_field: "@gap", first: 1 }, cols: { gap_field: "gap", first: 1 } },
  col_label: "letters",
  max: { rows: 1048576, cols: 16384 },
  cell: { encoding: "json", type: "t", value: "v", formula: "f", formula_kind: "fk", style: "s", types: { n: "number", s: "text", b: "bool", e: "error", d: "date" } },
  row_fields: { hidden: "@hidden", height: "@ht", height_unit: "pt" },
  col_field_names: { hidden: "hidden", width: "w", width_unit: "max digit widths" },
  viewport: "cells",
  style_hint: { field: "s", default: "0", table: "styles", parts: ["bold", "fill", "numfmt"] },
  cell_derived: ["a1"],
};

/** The workbook's view schema: sheets by their `sid`, rows naming theirs in `@sheet`, cell fields
 * of class `cell` by prefix. */
export const WORKBOOK_VIEW: ViewSchemaJson = {
  containers: "sheets",
  units: "rows",
  unit_container_field: "@sheet",
  container_id_field: "sid",
  native_unit: "pt",
  page: null,
  container_fields: { title: "name", hidden: "hidden", layout: null },
  unit_fields: { kind: null, name: null, alt: null, text: null, hidden: "@hidden", editable_text: null, placeholder: null },
  geometry: null,
  field_class: { "@ht": "style" },
  field_class_prefix: [["c", "cell"]],
  derived: { containers: [], units: [] },
  grid: WORKBOOK_GRID,
};

/** A transport the test drives: what the client sent, and a way to deliver frames to it. */
export function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: ClientFrame[] = [];
  const t: Transport = {
    send: (f) => void sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  return { t, sent, deliver: (f: ServerFrame) => [...ls].forEach((l) => l(f)) };
}

const isComposite = (root: string) => WORKBOOK_SCHEMA.composite!.some((c) => c.map === root);

/** The keyed watch's snapshot of the vector's first state: a head with the workbook's view, then
 * one page a root, in schema order; the rows' map page carries the formula cells' A1 text
 * (`derived.a1`), as the kit's does. */
export function snapshotFrames(v: GridVector, view: ViewSchemaJson = WORKBOOK_VIEW): ServerFrame[] {
  const roots = WORKBOOK_SCHEMA.roots;
  const frames: ServerFrame[] = [
    {
      type: "keyed_snapshot",
      seq: 1,
      upto: v.rev,
      schema: WORKBOOK_SCHEMA,
      view: { schema: view, tier: "live", rev: v.rev, version: v.rev, page: null, fonts: [], layouts: [], containers: [] },
      pages: roots.length,
    },
  ];
  roots.forEach((r, i) => {
    const head = { type: "keyed_page" as const, seq: i + 2, upto: v.rev, page: i + 1, pages: roots.length, root: r.name };
    if (r.kind === "list") {
      frames.push({ ...head, live: v.lists.filter(([root]) => root === r.name).map(([, a, val]): [string, string] => [a, val]) });
      return;
    }
    const entries = v.maps.filter(([root]) => root === r.name).map(([, a, field, val]): [string, string | null, string] => (isComposite(r.name) ? [a, field, val] : [field === "" ? a : `${a}\u001f${field}`, null, val]));
    if (r.name !== WORKBOOK_GRID.cells || v.a1.length === 0) {
      frames.push({ ...head, entries });
      return;
    }
    const a1: Record<string, Record<string, string>> = {};
    for (const [unit, field, text] of v.a1) (a1[unit] ??= {})[field] = text;
    frames.push({ ...head, entries, derived: { a1 } });
  });
  return frames;
}

/** A rev's change frame, numbered `seq`, its list ops placed by `state` (the state before it). */
export function changeFrame(r: VectorRev, seq: number, state: KeyedState): ServerFrame {
  const ops: WatchOp[] = r.ops.map((o): WatchOp => {
    if (o.kind === "list") {
      const root = state.roots.get(o.root);
      const live = root && root.kind === "list" ? root.live : [];
      const i = o.after === null ? 0 : live.findIndex(([k]) => k === o.after) + 1;
      return { root: o.root, kind: "list", op: "insert", key: o.address, index_before: null, index_after: i, after: o.value, effect: "visible" };
    }
    const field = isComposite(o.root) ? o.field : null;
    const key = isComposite(o.root) || o.field === "" ? o.address : `${o.address}\u001f${o.field}`;
    return o.op === "set" ? { root: o.root, kind: "map", op: "set", key, field, after: o.value } : { root: o.root, kind: "map", op: "delete", key, field };
  });
  return {
    type: "ops_change",
    seq,
    rev: r.rev,
    at_ms: 0,
    change_kind: "edit",
    actor: { id: "agent:atlas", name: "Atlas", kind: "agent", colour: "#6d5bd0" },
    client: 1,
    via: "cli",
    on_behalf_of: null,
    turn: null,
    verb: r.verb,
    claim: null,
    ops,
    derived: null,
    render_hashes: null,
  };
}

/** Cells in one key order, for comparing what the roster carried with the vector's. */
export function canonCells(c: Cells | null | undefined): string {
  if (!c) return "null";
  const k = (x: Corner | null) => (x === null ? null : { row: x.row, row_off: x.row_off, col: x.col, col_off: x.col_off });
  return JSON.stringify({ container: c.container, active: k(c.active), ranges: c.ranges.map((r) => ({ start: k(r.start), end: k(r.end) })) });
}
