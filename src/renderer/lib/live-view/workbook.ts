// A live workbook as the view reads it: the keyed state (kept as KeyedWatch
// keeps it) plus the pack's derived values (A1 text, merges, style tables),
// and the derivations a grid needs: sheets, and the grid of a sheet, numbered
// and placed by the kit's grid helpers (`gridNumbers`, `cornerNumbers`,
// `colOfCellField`...), with cells in their shown form. No React, no DOM.

import {
  colLetters,
  colOfCellField,
  cornerNumbers,
  cornerProblem,
  gridNumbers,
  mapKey,
  rangeA1 as kitRangeA1,
  type CellRange,
  type ContainerDerived,
  type Corner,
  type FrameDerived,
  type GridNumbers,
  type KeyedState,
  type KeyedValue,
  type OpsChangeFrame,
  type StyleHint,
} from '@k2/mpc-client'
import { A1, cellDerived, NO_CELL_DERIVED, withFrameDerived, withUnitsDerived, type CellDerivedValues } from './cell-derived'
import { foldOps } from './keyed-fold'
import type { SheetState, XlsxViewSchema, XlsxViewSnapshot, XlsxViewUnit } from './not-in-kit'

/** One version of the workbook. Immutable: a frame makes a new one. */
export interface Book {
  rev: number
  state: KeyedState
  schema: XlsxViewSchema
  /** The derived values kept by cell field (`a1`); read with `cellDerived(book, name, unit, field)`. */
  cellDerived: CellDerivedValues
  /** `Container.derived` by sheet key (merges, styles). */
  containers: Readonly<Record<string, ContainerDerived>>
}

/** A book from a whole snapshot: the keyed state, the view's containers, and the cell-derived values (the rows pages', then the units'). */
export function bookFromSnapshot(
  state: KeyedState,
  view: XlsxViewSnapshot,
  units: ReadonlyMap<string, readonly XlsxViewUnit[]>,
  rev: number,
  fromPages: CellDerivedValues = NO_CELL_DERIVED,
): Book {
  // r1.1: the A1 text rides on the cells pages too, so a formula never reads as its number while the units stream.
  const derived = withUnitsDerived(fromPages, units.values(), view.schema.grid)
  const containers: Record<string, ContainerDerived> = {}
  for (const c of view.containers) if (c.derived) containers[c.key] = { merges: c.derived.merges ?? [], styles: c.derived.styles ?? {} }
  return { rev, state, schema: view.schema, cellDerived: derived, containers }
}

/** A frame onto a book: its ops (the kit's KeyedWatch, keyed-fold.ts), then the pack's derived values. Throws `KeyedMisfit` when it doesn't fit. */
export function applyFrameToBook(book: Book, f: OpsChangeFrame): Book {
  const state = foldOps(book.state, book.rev, f)
  const d: FrameDerived = f.derived ?? {}
  const derived = withFrameDerived(book.cellDerived, d, book.schema.grid)
  let containers = book.containers
  if (d.containers) {
    const next: Record<string, ContainerDerived> = { ...book.containers }
    for (const [k, c] of Object.entries(d.containers)) next[k] = { ...(next[k] ?? {}), ...c }
    containers = next
  }
  return { rev: f.rev, state, schema: book.schema, cellDerived: derived, containers }
}

/** A cell's A1 text (`derived.a1`, a formula's text as people read it), `null` when it isn't a formula. */
export const a1Text = (book: Book, row: string, field: string): string | null => cellDerived(book, A1, row, field)

// ---------- reading roots ----------

function mapGet(state: KeyedState, root: string, key: string, field: string | null): KeyedValue | undefined {
  const r = state.roots.get(root)
  return r && r.kind === 'map' ? r.entries.get(mapKey(key, field)) : undefined
}

function listKeys(state: KeyedState, root: string): string[] {
  const r = state.roots.get(root)
  return r && r.kind === 'list' ? r.live.map((e) => e[0]) : []
}

/** Which map holds a list's fields (`composite`: sheets → sheet_props). */
function propsRoot(state: KeyedState, list: string): string | null {
  return state.schema.composite?.find((c) => c.list === list)?.map ?? null
}

// ---------- sheets ----------

export interface Sheet {
  key: string
  /** Tab order, from 1. */
  position: number
  title: string
  hidden: boolean
  state: SheetState
  sid: string | null
}

const SHEETS_CACHE = new WeakMap<Book, Sheet[]>()

/** The sheets in tab order (the containers list), with their fields by the schema's `container_fields`. */
export function sheetsOf(book: Book): Sheet[] {
  const hit = SHEETS_CACHE.get(book)
  if (hit) return hit
  const sch = book.schema
  const root = sch.containers
  const props = propsRoot(book.state, root)
  const cf = sch.container_fields
  const field = (k: string, src: unknown): string | null => (props && typeof src === 'string' ? (mapGet(book.state, props, k, src) ?? null) : null)
  const out = listKeys(book.state, root).map((key, i): Sheet => {
    const hidden = field(key, cf.hidden) === '1'
    const rawState = cf.state && props ? (mapGet(book.state, props, key, cf.state.field) ?? null) : null
    const mapped = rawState != null ? (cf.state?.values[rawState] ?? rawState) : null
    const state: SheetState = mapped === 'very_hidden' || mapped === 'hidden' || mapped === 'visible' ? mapped : hidden ? 'hidden' : 'visible'
    return { key, position: i + 1, title: field(key, cf.title) ?? '', hidden, state, sid: sch.container_id_field && props ? (mapGet(book.state, props, key, sch.container_id_field) ?? null) : null }
  })
  SHEETS_CACHE.set(book, out)
  return out
}

export function sheetOf(book: Book, key: string): Sheet | undefined {
  return sheetsOf(book).find((s) => s.key === key)
}

export function titleOf(book: Book, key: string): string {
  return sheetOf(book, key)?.title ?? ''
}

// ---------- the grid ----------

export interface GridRow {
  key: string
  /** Its number (VG): never the key. */
  n: number
  /** Its place in the sheet's run of keyed rows. */
  i: number
  hidden: boolean
  /** `@ht`, in points (r1 units). */
  heightPt: number | null
}

export interface GridCol {
  key: string
  n: number
  i: number
  letter: string
  hidden: boolean
  /** `w`, in characters of the default font's maximum digit width (r1 units). */
  widthChars: number | null
}

/** What a cell shows (Item 5): a value, or a formula's A1 text with its cached result or none. */
export type CellShown =
  | { kind: 'value'; text: string }
  | { kind: 'formula'; formula: string; result: string | null; resultState: 'as_last_calculated' | 'not_calculated' }

export interface GridCell {
  row: string
  field: string
  r: number
  c: number
  /** The register's type (`n`, `s`, `b`, `e`, `d`), when present. */
  t: string | null
  v: string | null
  /** The register's style index (0 when absent). */
  s: number
  a1: string | null
  raw: string
}

export interface Grid {
  key: string
  title: string
  rows: GridRow[]
  cols: GridCol[]
  rowByKey: ReadonlyMap<string, GridRow>
  colByKey: ReadonlyMap<string, GridCol>
  rowByNumber: ReadonlyMap<number, GridRow>
  colByNumber: ReadonlyMap<number, GridCol>
  cells: GridCell[]
  cellAt(r: number, c: number): GridCell | undefined
  merges: CellRange[]
  styles: Readonly<Record<string, StyleHint>>
  /** The kit's numbering of this sheet (`gridNumbers`): what its corner helpers read. */
  numbers: GridNumbers
  lastRow: number
  lastCol: number
}

/** The cells of each row, by row key: one pass over the cells map per book. */
const ROW_FIELDS_CACHE = new WeakMap<Book, Map<string, [string, string][]>>()

function rowFields(book: Book): Map<string, [string, string][]> {
  const hit = ROW_FIELDS_CACHE.get(book)
  if (hit) return hit
  const out = new Map<string, [string, string][]>()
  const r = book.state.roots.get(book.schema.grid.cells)
  if (r && r.kind === 'map') {
    for (const [k, v] of r.entries) {
      const at = k.indexOf('\u001f')
      if (at < 0 || v == null) continue
      const rk = k.slice(0, at)
      const fld = k.slice(at + 1)
      const list = out.get(rk)
      if (list) list.push([fld, v])
      else out.set(rk, [[fld, v]])
    }
  }
  ROW_FIELDS_CACHE.set(book, out)
  return out
}

const GRID_CACHE = new WeakMap<Book, Map<string, Grid | null>>()

/** The grid of one sheet at this book's rev; `null` when the sheet isn't there. Cached per book. */
export function gridOf(book: Book, sheetKey: string): Grid | null {
  let byC = GRID_CACHE.get(book)
  if (!byC) GRID_CACHE.set(book, (byC = new Map()))
  if (byC.has(sheetKey)) return byC.get(sheetKey) ?? null
  const sh = sheetOf(book, sheetKey)
  if (!sh || sh.sid == null) {
    byC.set(sheetKey, null)
    return null
  }
  const gs = book.schema.grid
  const numbers = gridNumbers(book.state, book.schema, sheetKey)
  const rows: GridRow[] = numbers.rows.map((r, i) => {
    const ht = mapGet(book.state, gs.cells, r.address, gs.row_fields.height)
    return { key: r.address, n: r.number, i, hidden: mapGet(book.state, gs.cells, r.address, gs.row_fields.hidden) === '1', heightPt: ht != null ? Number(ht) : null }
  })
  const cols: GridCol[] = numbers.cols.map((c, i) => {
    const w = mapGet(book.state, gs.col_fields, c.address, gs.col_field_names.width)
    return { key: c.address, n: c.number, i, letter: colLetters(c.number), hidden: mapGet(book.state, gs.col_fields, c.address, gs.col_field_names.hidden) === '1', widthChars: w != null ? Number(w) : null }
  })
  const rowByKey = new Map(rows.map((r) => [r.key, r]))
  const colByKey = new Map(cols.map((c) => [c.key, c]))
  const cells: GridCell[] = []
  const byPos = new Map<string, GridCell>()
  const fieldsByRow = rowFields(book)
  for (const r of rows) {
    for (const [fld, raw] of fieldsByRow.get(r.key) ?? []) {
      const address = colOfCellField(gs, sheetKey, fld)
      const col = address === null ? undefined : colByKey.get(address)
      if (!col) continue
      let reg: Record<string, unknown>
      try {
        reg = JSON.parse(raw) as Record<string, unknown>
      } catch {
        reg = { v: raw }
      }
      const t = reg[gs.cell.type]
      const v = reg[gs.cell.value]
      const s = reg[gs.cell.style]
      const cell: GridCell = {
        row: r.key,
        field: fld,
        r: r.n,
        c: col.n,
        t: typeof t === 'string' ? t : null,
        v: v == null ? null : String(v),
        s: typeof s === 'number' ? s : Number(s ?? 0) || 0,
        a1: a1Text(book, r.key, fld),
        raw,
      }
      cells.push(cell)
      byPos.set(`${cell.r}:${cell.c}`, cell)
    }
  }
  const derived = book.containers[sheetKey] ?? {}
  const g: Grid = {
    key: sheetKey,
    title: sh.title,
    rows,
    cols,
    rowByKey,
    colByKey,
    rowByNumber: new Map(rows.map((r) => [r.n, r])),
    colByNumber: new Map(cols.map((c) => [c.n, c])),
    cells,
    cellAt: (r, c) => byPos.get(`${r}:${c}`),
    merges: derived.merges ?? [],
    styles: derived.styles ?? {},
    lastRow: rows.length ? rows[rows.length - 1].n : 0,
    lastCol: cols.length ? cols[cols.length - 1].n : 0,
    numbers,
  }
  byC.set(sheetKey, g)
  return g
}

/** The sheet a row belongs to (its `@sheet` matched to a sheet's `sid`). */
export function sheetOfRow(book: Book, rowKey: string): string | null {
  const sid = mapGet(book.state, book.schema.grid.cells, rowKey, book.schema.unit_container_field)
  return sheetsOf(book).find((s) => s.sid === sid)?.key ?? null
}

// ---------- what a cell shows ----------

/** Item 5: a formula is known by `derived.a1` having its field (the view never reads `f`). */
export function cellShown(cell: GridCell): CellShown {
  if (cell.a1 != null) return { kind: 'formula', formula: cell.a1, result: cell.v, resultState: cell.v == null ? 'not_calculated' : 'as_last_calculated' }
  return { kind: 'value', text: cell.v ?? '' }
}

export const DEFAULT_STYLE: StyleHint = { bold: false, fill: null, numfmt: 'General' }

/** A cell's style hint: its register's `s` in its sheet's table (r1 ruling 10). A missing table or index is the default (r1.1), never another sheet's. */
export function styleOf(g: Grid, cell: GridCell): StyleHint {
  return g.styles[String(cell.s)] ?? DEFAULT_STYLE
}

export function isDefaultStyle(h: StyleHint): boolean {
  return !h.bold && h.fill == null && h.numfmt === 'General'
}

/**
 * The common number formats only (Q-X16): General, 0, 0.00, #,##0, #,##0.00,
 * 0%, 0.00% and dates as yyyy-mm-dd. Anything else shows the raw value
 * (`applied: false`, so the view puts the code on hover).
 */
export function formatNumber(v: string, numfmt: string): { text: string; applied: boolean } {
  const n = Number(v)
  if (numfmt === 'General' || v.trim() === '' || !Number.isFinite(n)) return { text: v, applied: numfmt === 'General' }
  const fixed = (d: number, group: boolean): string => n.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d, useGrouping: group })
  switch (numfmt) {
    case '0':
      return { text: fixed(0, false), applied: true }
    case '0.00':
      return { text: fixed(2, false), applied: true }
    case '#,##0':
      return { text: fixed(0, true), applied: true }
    case '#,##0.00':
      return { text: fixed(2, true), applied: true }
    case '0%':
      return { text: `${(n * 100).toFixed(0)}%`, applied: true }
    case '0.00%':
      return { text: `${(n * 100).toFixed(2)}%`, applied: true }
    case 'yyyy-mm-dd': {
      // A serial day number (the 1900 system: day 25,569 is 1970-01-01).
      const d = new Date(Math.round((n - 25569) * 86400000))
      return Number.isNaN(d.getTime()) ? { text: v, applied: false } : { text: d.toISOString().slice(0, 10), applied: true }
    }
    default:
      return { text: v, applied: false }
  }
}

// ---------- corners (the kit's grid helpers) ----------

export interface RC {
  r: number
  c: number
}

export interface Rect4 {
  r0: number
  r1: number
  c0: number
  c1: number
}

/** A corner's row and column numbers on this grid (the kit's `cornerNumbers`), `null` when it isn't one of this sheet's in normal form and in bounds. */
export function cornerPlace(g: Grid, c: Corner): RC | null {
  const at = cornerNumbers(g.numbers, c)
  return at ? { r: at.row, c: at.col } : null
}

/** The corner is live, on this sheet, in normal form and within the sheet's bounds (the kit's `cornerProblem` finds nothing). */
export function cornerOk(g: Grid, c: Corner): boolean {
  return cornerProblem(g.numbers, c) === null
}

export const a1Of = (rc: RC): string => colLetters(rc.c) + rc.r

/** A range to its rectangle (normalised), or null when a corner doesn't place. */
export function rangeRect(g: Grid, r: CellRange): Rect4 | null {
  const s = cornerPlace(g, r.start)
  const e = cornerPlace(g, r.end)
  if (!s || !e) return null
  return { r0: Math.min(s.r, e.r), r1: Math.max(s.r, e.r), c0: Math.min(s.c, e.c), c1: Math.max(s.c, e.c) }
}

/** A range as A1 text, start then end, as the person made it (`C4:D15`; a one-cell range is still `D5:D5`): the kit's `rangeA1` on this sheet's numbers. */
export const rangeA1 = (g: Grid, r: CellRange): string | null => kitRangeA1(g.numbers, r)

/** A rectangle as A1 for wording: one cell as `F6`, else `D5:D7`. */
export function rectA1(R: Rect4): string {
  return R.r0 === R.r1 && R.c0 === R.c1 ? colLetters(R.c0) + R.r0 : `${colLetters(R.c0)}${R.r0}:${colLetters(R.c1)}${R.r1}`
}

export const inRect = (rc: RC, R: Rect4): boolean => rc.r >= R.r0 && rc.r <= R.r1 && rc.c >= R.c0 && rc.c <= R.c1
export const overlaps = (A: Rect4, R: Rect4): boolean => A.r0 <= R.r1 && R.r0 <= A.r1 && A.c0 <= R.c1 && R.c0 <= A.c1

/** A peer's viewport read as cells (`grid.viewport: "cells"`): rows floor(y)…ceil(y+h)−1, columns the same. */
export function viewportRect(v: { x: number; y: number; w: number; h: number }): Rect4 {
  return { r0: Math.floor(v.y), r1: Math.ceil(v.y + v.h) - 1, c0: Math.floor(v.x), c1: Math.ceil(v.x + v.w) - 1 }
}

export function viewportA1(v: { x: number; y: number; w: number; h: number }): string {
  const R = viewportRect(v)
  return `${colLetters(R.c0)}${R.r0}:${colLetters(R.c1)}${R.r1}`
}

// ---------- units (r1 ruling 8) ----------

/** The fixed digit width K2 uses until the pack names the default font (gap X-18): px per character, plus padding. */
export const DIGIT_PX = 7
export const COL_PAD_PX = 12

/** A column width in characters of the default font's maximum digit width → pixels. */
export const colWidthPx = (chars: number): number => Math.round(chars * DIGIT_PX + COL_PAD_PX)

/** A row height in points → pixels at 96 dpi. */
export const rowHeightPx = (pt: number): number => Math.round((pt * 96) / 72)
