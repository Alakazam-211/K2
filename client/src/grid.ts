// The live grid as data (WIRE.md, "The live grid"): a container's rows and columns numbered
// from the keyed state a client keeps (`KeyedWatch.state`) and the view schema's `grid`; corners
// by address and their cell numbers; normal form and bounds; A1. Pure functions, no DOM. Rows
// and columns are named by address everywhere; a number is worked out from the list order and
// the gaps, never read out of an address.

import { mapKey, type KeyedState } from "./keyed";
import type { CellRange, Cells, Corner, FieldClass, GridSlot, ViewSchemaJson, ViewSrc } from "./wire";

/** The name a host lists in `welcome.features` when it takes the `cells` frame. */
export const CELLS_FEATURE = "cells";

/** The name a host lists in `welcome.features` when it takes a claim on cells. */
export const CELL_CLAIMS_FEATURE = "cell_claims";

/** The most ranges in one `Cells`. */
export const CELLS_RANGES_MAX = 16;

/** One keyed row or column and its number. */
export interface GridLine {
  address: string;
  number: number;
}

/** A container's keyed rows and columns, numbered, in list order (so in number order), and the
 * grid's highest row and column numbers. */
export interface GridNumbers {
  container: string;
  rows: GridLine[];
  cols: GridLine[];
  max: { rows: number; cols: number };
}

/** Why a corner isn't one: `bad` (an offset that isn't a whole number at least 0), `row` / `col`
 * (not a live row or column of the container: another container's, deleted, or unknown),
 * `row_form` / `col_form` (not in normal form: a keyed row or column lies between it and the
 * cell, or a `null` row with an offset under 1), `rows_max` / `cols_max` (past the last row or
 * column). */
export type CornerProblem = "bad" | "row" | "col" | "row_form" | "col_form" | "rows_max" | "cols_max";

/** How the numbering reads a gap value: a whole number at least 0, else 0 (missing, or not a
 * number). One object, so a test can plant a fault in it. */
export const gridRead = {
  gap(value: string | null): number {
    if (value === null) return 0;
    const t = value.trim();
    if (!/^\d+$/.test(t)) return 0;
    const n = Number(t);
    return Number.isSafeInteger(n) ? n : 0;
  },
};

function mapGet(state: KeyedState, map: string, key: string): string | null {
  const r = state.roots.get(map);
  if (!r || r.kind !== "map") return null;
  return r.entries.get(key) ?? null;
}

/** The value a container's rows and columns name it by (`container_id_field`, or its address),
 * `null` when it isn't a live container. */
function containerId(state: KeyedState, schema: ViewSchemaJson, container: string): string | null {
  const list = state.roots.get(schema.containers);
  if (!list || list.kind !== "list" || !list.live.some(([k]) => k === container)) return null;
  if (schema.container_id_field === null) return container;
  const map = state.schema.composite?.find((c) => c.list === schema.containers)?.map;
  return map === undefined ? null : mapGet(state, map, mapKey(container, schema.container_id_field));
}

/** The live elements of `list` whose `field` (in `map`) is `id`, numbered. */
function numbered(state: KeyedState, list: string, map: string, field: string, id: string, gapField: string, first: number): GridLine[] {
  const r = state.roots.get(list);
  if (!r || r.kind !== "list") return [];
  const out: GridLine[] = [];
  let prev: number | null = null;
  for (const [address] of r.live) {
    if (mapGet(state, map, mapKey(address, field)) !== id) continue;
    const gap = gridRead.gap(mapGet(state, map, mapKey(address, gapField)));
    const n: number = prev === null ? gap + first : prev + gap + 1;
    out.push({ address, number: n });
    prev = n;
  }
  return out;
}

/**
 * Container `container`'s keyed rows and columns, numbered: within the container, in the list's
 * order, each one's number is the previous one's plus its gap plus 1, the first one's its gap
 * plus the axis's `first`. Deleted elements don't count. Empty for a container that isn't live,
 * and for a schema with no grid.
 */
export function gridNumbers(state: KeyedState, schema: ViewSchemaJson, container: string): GridNumbers {
  const g = schema.grid;
  if (!g) return { container, rows: [], cols: [], max: { rows: 0, cols: 0 } };
  const id = containerId(state, schema, container);
  const n = g.numbering;
  return {
    container,
    rows: id === null ? [] : numbered(state, g.rows, g.cells, schema.unit_container_field, id, n.rows.gap_field, n.rows.first),
    cols: id === null ? [] : numbered(state, g.cols, g.col_fields, g.col_container_field, id, n.cols.gap_field, n.cols.first),
    max: { rows: g.max.rows, cols: g.max.cols },
  };
}

/** The numbers from `first` to the last line's that no keyed line has (a container's blank rows
 * between its keyed ones). */
export function blankNumbers(lines: readonly GridLine[], first = 1): number[] {
  const taken = new Set(lines.map((l) => l.number));
  const last = lines.length === 0 ? first - 1 : lines[lines.length - 1].number;
  const out: number[] = [];
  for (let n = first; n <= last; n++) if (!taken.has(n)) out.push(n);
  return out;
}

/** A column number as letters: 1 is `A`, 26 `Z`, 27 `AA`, 16384 `XFD`; `""` for a number under
 * 1 or not whole. */
export function colLetters(n: number): string {
  if (!Number.isSafeInteger(n) || n < 1) return "";
  let s = "";
  for (let x = n; x > 0; x = Math.floor((x - 1) / 26)) s = String.fromCharCode(65 + ((x - 1) % 26)) + s;
  return s;
}

/** Letters as a column number (`A` is 1, `xfd` 16384), `null` when they aren't letters. */
export function colNumber(letters: string): number | null {
  if (!/^[A-Za-z]{1,10}$/.test(letters)) return null;
  let n = 0;
  for (const ch of letters.toUpperCase()) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n;
}

/** A column's label as the grid names columns (`col_label`): letters, or the number itself. */
export function colLabel(grid: GridSlot, n: number): string {
  return grid.col_label === "numbers" ? String(n) : colLetters(n);
}

/** One axis of a corner: the cell's number, or why not. */
function axis(lines: readonly GridLine[], at: string | null, off: number, max: number): number | "bad" | "live" | "form" | "max" {
  if (!Number.isSafeInteger(off) || off < 0 || (at !== null && typeof at !== "string")) return "bad";
  let n: number;
  let next: GridLine | undefined;
  if (at === null) {
    if (off < 1) return "form";
    n = off;
    next = lines[0];
  } else {
    const i = lines.findIndex((l) => l.address === at);
    if (i < 0) return "live";
    n = lines[i].number + off;
    next = lines[i + 1];
  }
  if (next !== undefined && n >= next.number) return "form";
  if (n > max) return "max";
  return n;
}

/** Why `corner` isn't a corner of `numbers`' container in normal form and in bounds (the row
 * checked first), or `null` when it is. */
export function cornerProblem(numbers: GridNumbers, corner: Corner): CornerProblem | null {
  const r = axis(numbers.rows, corner.row, corner.row_off, numbers.max.rows);
  if (typeof r !== "number") return r === "bad" ? "bad" : r === "live" ? "row" : r === "form" ? "row_form" : "rows_max";
  const c = axis(numbers.cols, corner.col, corner.col_off, numbers.max.cols);
  if (typeof c !== "number") return c === "bad" ? "bad" : c === "live" ? "col" : c === "form" ? "col_form" : "cols_max";
  return null;
}

/** A corner's row and column numbers, `null` when it isn't one (`cornerProblem`). */
export function cornerNumbers(numbers: GridNumbers, corner: Corner): { row: number; col: number } | null {
  const row = axis(numbers.rows, corner.row, corner.row_off, numbers.max.rows);
  const col = axis(numbers.cols, corner.col, corner.col_off, numbers.max.cols);
  return typeof row === "number" && typeof col === "number" ? { row, col } : null;
}

/** The last keyed line at or before number `n`, and how far past it `n` is (none: `null`, `n`). */
function anchor(lines: readonly GridLine[], n: number): [string | null, number] {
  let last: GridLine | null = null;
  for (const l of lines) {
    if (l.number > n) break;
    last = l;
  }
  return last ? [last.address, n - last.number] : [null, n];
}

/** The corner, in normal form, of the cell at row `row` and column `col` of `numbers`'
 * container; `null` when either is out of bounds (under 1, past the most, not whole). */
export function cornerAt(numbers: GridNumbers, row: number, col: number): Corner | null {
  const ok = (n: number, max: number) => Number.isSafeInteger(n) && n >= 1 && n <= max;
  if (!ok(row, numbers.max.rows) || !ok(col, numbers.max.cols)) return null;
  const [r, ro] = anchor(numbers.rows, row);
  const [c, co] = anchor(numbers.cols, col);
  return { row: r, row_off: ro, col: c, col_off: co };
}

/** A corner in A1 (`E18`; always letters, whatever the grid's `col_label`), `null` when it isn't
 * one (`cornerProblem`). */
export function cornerA1(numbers: GridNumbers, corner: Corner): string | null {
  if (cornerProblem(numbers, corner) !== null) return null;
  const at = cornerNumbers(numbers, corner)!;
  return colLetters(at.col) + at.row;
}

/** A range in A1, its start's then its end's as given (`A1:B4`, `D1:D1048576`), never swapped;
 * `null` when a corner isn't one. */
export function rangeA1(numbers: GridNumbers, range: CellRange): string | null {
  const a = cornerA1(numbers, range.start);
  const b = cornerA1(numbers, range.end);
  return a === null || b === null ? null : `${a}:${b}`;
}

/** Cells in A1: the active cell (`null` when none) and each range; `null` when they are another
 * container's or a corner isn't one. */
export function cellsA1(numbers: GridNumbers, cells: Cells): { active: string | null; ranges: string[] } | null {
  if (cells.container !== numbers.container) return null;
  const active = cells.active === null ? null : cornerA1(numbers, cells.active);
  if (cells.active !== null && active === null) return null;
  const ranges: string[] = [];
  for (const r of cells.ranges) {
    const a1 = rangeA1(numbers, r);
    if (a1 === null) return null;
    ranges.push(a1);
  }
  return { active, ranges };
}

/** The column address a field of a row's map names in container `container` (the grid's
 * `col_of_cell_field`), `null` for a row field (one starting with `row_fields_start`). */
export function colOfCellField(grid: GridSlot, container: string, field: string): string | null {
  if (field.startsWith(grid.row_fields_start)) return null;
  return grid.col_of_cell_field.replace(/\{(container|field)\}/g, (_, k: string) => (k === "container" ? container : field));
}

/** The field a source reads, if any. */
function srcField(src: ViewSrc): string | null {
  if (typeof src === "string") return src;
  if (src && "field" in src) return src.field;
  return null;
}

/**
 * A field's class by the view schema, as the kit works it out: the geometry field is
 * `geometry`; else its `field_class` entry; else the class of the first `field_class_prefix`
 * rule it starts with; else `visibility` for the hidden field (the units', or with `of:
 * "container"` the containers'), `meta` otherwise.
 */
export function fieldClass(schema: ViewSchemaJson, field: string, of: "unit" | "container" = "unit"): FieldClass {
  if (schema.geometry?.field === field) return "geometry";
  if (Object.prototype.hasOwnProperty.call(schema.field_class, field)) return schema.field_class[field];
  for (const [prefix, c] of schema.field_class_prefix ?? []) if (field.startsWith(prefix)) return c;
  const hidden = of === "unit" ? schema.unit_fields.hidden : schema.container_fields.hidden;
  return srcField(hidden) === field ? "visibility" : "meta";
}
