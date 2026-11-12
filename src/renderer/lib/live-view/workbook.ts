// A live workbook as the view reads it: the keyed state (kept as KeyedWatch
// keeps it) plus the pack's derived values (A1 text, merges, style tables),
// and the pure derivations a grid needs: sheets, row numbers and column
// letters by the grid slot's rule (kit item VG), cells with their shown form,
// corners and A1. No React, no DOM.

import { applyKeyedOps, mapKey, type KeyedState, type KeyedValue } from './kit'
import type {
  CellRange,
  ContainerDerived,
  Corner,
  SheetState,
  StyleHint,
  StyleTable,
  XlsxFrameDerived,
  XlsxOpsChangeFrame,
  XlsxViewSchema,
  XlsxViewSnapshot,
  XlsxViewUnit,
} from './xlsx-provisional'

/** One version of the workbook. Immutable: a frame makes a new one. */
export interface Book {
  rev: number
  state: KeyedState
  schema: XlsxViewSchema
  /** `derived.a1` by row key, then cell field. */
  a1: Readonly<Record<string, Readonly<Record<string, string>>>>
  /** `Container.derived` by sheet key (merges, styles). */
  containers: Readonly<Record<string, ContainerDerived>>
}

/** A book from a whole snapshot: the keyed state, the view's containers and its units' `derived.a1`. */
export function bookFromSnapshot(state: KeyedState, view: XlsxViewSnapshot, units: ReadonlyMap<string, readonly XlsxViewUnit[]>, rev: number): Book {
  const a1: Record<string, Record<string, string>> = {}
  for (const list of units.values()) {
    for (const u of list) {
      const m = u.derived?.a1
      if (!m) continue
      const row: Record<string, string> = {}
      for (const [f, t] of Object.entries(m)) if (t != null) row[f] = t
      a1[u.key] = row
    }
  }
  const containers: Record<string, ContainerDerived> = {}
  for (const c of view.containers) if (c.derived) containers[c.key] = { merges: c.derived.merges ?? [], styles: c.derived.styles ?? {} }
  return { rev, state, schema: view.schema, a1, containers }
}

/** A frame onto a book: its ops (KeyedWatch's rules), then the pack's derived values. Throws `KeyedMisfit` when it doesn't fit. */
export function applyFrameToBook(book: Book, f: XlsxOpsChangeFrame): Book {
  const state = applyKeyedOps(book.state, f.ops)
  const d: XlsxFrameDerived = f.derived ?? {}
  let a1 = book.a1
  if (d.units) {
    const next: Record<string, Record<string, string>> = { ...book.a1 }
    for (const [k, u] of Object.entries(d.units)) {
      if (!u?.a1) continue
      const row: Record<string, string> = { ...(next[k] ?? {}) }
      for (const [fld, t] of Object.entries(u.a1)) {
        if (t === null) delete row[fld]
        else row[fld] = t
      }
      next[k] = row
    }
    a1 = next
  }
  let containers = book.containers
  if (d.containers) {
    const next: Record<string, ContainerDerived> = { ...book.containers }
    for (const [k, c] of Object.entries(d.containers)) next[k] = { ...(next[k] ?? {}), ...c }
    containers = next
  }
  return { rev: f.rev, state, schema: book.schema, a1, containers }
}

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

// ---------- letters ----------

/** 1 → A, 26 → Z, 27 → AA, 16,384 → XFD (`col_label: letters`). */
export function letters(n: number): string {
  let s = ''
  let x = n
  while (x > 0) {
    const m = (x - 1) % 26
    s = String.fromCharCode(65 + m) + s
    x = Math.floor((x - 1) / 26)
  }
  return s
}

export function lettersToN(s: string): number {
  let n = 0
  for (const ch of s) n = n * 26 + (ch.charCodeAt(0) - 64)
  return n
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
  styles: StyleTable
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

/** VG's rule: within one container, in list order, number = previous + gap + 1 (a missing gap is 0). */
function numbered(book: Book, list: string, propsMap: string, contField: string, gapField: string, first: number, sid: string): { key: string; n: number }[] {
  const out: { key: string; n: number }[] = []
  let n = first - 1
  for (const k of listKeys(book.state, list)) {
    if (mapGet(book.state, propsMap, k, contField) !== sid) continue
    n = n + Number(mapGet(book.state, propsMap, k, gapField) ?? 0) + 1
    out.push({ key: k, n })
  }
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
  const ucf = book.schema.unit_container_field
  const rows: GridRow[] = numbered(book, gs.rows, gs.cells, ucf, gs.numbering.rows.gap_field, gs.numbering.rows.first, sh.sid).map((r, i) => {
    const ht = mapGet(book.state, gs.cells, r.key, gs.row_fields.height)
    return { key: r.key, n: r.n, i, hidden: mapGet(book.state, gs.cells, r.key, gs.row_fields.hidden) === '1', heightPt: ht != null ? Number(ht) : null }
  })
  const cols: GridCol[] = numbered(book, gs.cols, gs.col_fields, gs.col_container_field, gs.numbering.cols.gap_field, gs.numbering.cols.first, sh.sid).map((c, i) => {
    const w = mapGet(book.state, gs.col_fields, c.key, gs.col_field_names.width)
    return { key: c.key, n: c.n, i, letter: letters(c.n), hidden: mapGet(book.state, gs.col_fields, c.key, gs.col_field_names.hidden) === '1', widthChars: w != null ? Number(w) : null }
  })
  const rowByKey = new Map(rows.map((r) => [r.key, r]))
  const colByKey = new Map(cols.map((c) => [c.key, c]))
  const cells: GridCell[] = []
  const byPos = new Map<string, GridCell>()
  const fieldsByRow = rowFields(book)
  for (const r of rows) {
    for (const [fld, raw] of fieldsByRow.get(r.key) ?? []) {
      if (fld.startsWith(gs.row_fields_start)) continue
      const col = colByKey.get(colAddress(gs.col_of_cell_field, sheetKey, fld))
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
        a1: book.a1[r.key]?.[fld] ?? null,
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
  }
  byC.set(sheetKey, g)
  return g
}

/** `col_of_cell_field`: a cell field (`c4`) to its column's address (`@sheet:1/col:c4`). */
export function colAddress(template: string, container: string, field: string): string {
  return template.replace('{container}', container).replace('{field}', field)
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

/** A cell's style hint: its register's `s` in its sheet's table (r1 ruling 10). */
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

// ---------- corners (kit item VC) ----------

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

function axisNum<T extends { n: number }>(byKey: ReadonlyMap<string, T>, key: string | null, off: number): number | null {
  if (key == null) return off
  const x = byKey.get(key)
  return x ? x.n + off : null
}

/** A corner to its row and column numbers (`null` parts when its row or column isn't on this grid). */
export function cornerRC(g: Grid, c: Corner): { r: number | null; c: number | null } {
  return { r: axisNum(g.rowByKey, c.row, c.row_off), c: axisNum(g.colByKey, c.col, c.col_off) }
}

/** A corner that places: both parts on this grid. */
export function cornerPlace(g: Grid, c: Corner): RC | null {
  const p = cornerRC(g, c)
  return p.r == null || p.c == null ? null : { r: p.r, c: p.c }
}

function axisOk<T extends { n: number; i: number }>(list: readonly T[], byKey: ReadonlyMap<string, T>, key: string | null, off: number, max: number): boolean {
  if (key != null && !byKey.has(key)) return false
  const base = key == null ? 0 : (byKey.get(key) as T).n
  const at = base + off
  if (off < 0 || at < 1 || at > max) return false
  // Normal form: no keyed row (column) strictly between the key and the cell.
  const next = key == null ? list[0] : list[(byKey.get(key) as T).i + 1]
  return !next || next.n > at
}

/** The corner is live, on this sheet, in normal form and within the sheet's bounds. */
export function cornerOk(g: Grid, c: Corner, max: { rows: number; cols: number }): boolean {
  return axisOk(g.rows, g.rowByKey, c.row, c.row_off, max.rows) && axisOk(g.cols, g.colByKey, c.col, c.col_off, max.cols)
}

/** A cell's corner in normal form: the last keyed row (column) at or before it, plus an offset. */
export function cornerAt(g: Grid, r: number, c: number): Corner {
  let lr: GridRow | null = null
  for (const x of g.rows) if (x.n <= r) lr = x
  let lc: GridCol | null = null
  for (const x of g.cols) if (x.n <= c) lc = x
  return { row: lr ? lr.key : null, row_off: lr ? r - lr.n : r, col: lc ? lc.key : null, col_off: lc ? c - lc.n : c }
}

export const a1Of = (rc: RC): string => letters(rc.c) + rc.r

/** A range to its rectangle (normalised), or null when a corner doesn't place. */
export function rangeRect(g: Grid, r: CellRange): Rect4 | null {
  const s = cornerPlace(g, r.start)
  const e = cornerPlace(g, r.end)
  if (!s || !e) return null
  return { r0: Math.min(s.r, e.r), r1: Math.max(s.r, e.r), c0: Math.min(s.c, e.c), c1: Math.max(s.c, e.c) }
}

/** A range as A1 text, start then end, as the person made it (`C4:D15`); a one-cell range is still `D5:D5`. */
export function rangeA1(g: Grid, r: CellRange): string | null {
  const s = cornerPlace(g, r.start)
  const e = cornerPlace(g, r.end)
  return s && e ? `${a1Of(s)}:${a1Of(e)}` : null
}

/** A rectangle as A1 for wording: one cell as `F6`, else `D5:D7`. */
export function rectA1(R: Rect4): string {
  return R.r0 === R.r1 && R.c0 === R.c1 ? letters(R.c0) + R.r0 : `${letters(R.c0)}${R.r0}:${letters(R.c1)}${R.r1}`
}

export const inRect = (rc: RC, R: Rect4): boolean => rc.r >= R.r0 && rc.r <= R.r1 && rc.c >= R.c0 && rc.c <= R.c1
export const overlaps = (A: Rect4, R: Rect4): boolean => A.r0 <= R.r1 && R.r0 <= A.r1 && A.c0 <= R.c1 && R.c0 <= A.c1

/** A peer's viewport read as cells (`grid.viewport: "cells"`): rows floor(y)…ceil(y+h)−1, columns the same. */
export function viewportRect(v: { x: number; y: number; w: number; h: number }): Rect4 {
  return { r0: Math.floor(v.y), r1: Math.ceil(v.y + v.h) - 1, c0: Math.floor(v.x), c1: Math.ceil(v.x + v.w) - 1 }
}

export function viewportA1(v: { x: number; y: number; w: number; h: number }): string {
  const R = viewportRect(v)
  return `${letters(R.c0)}${R.r0}:${letters(R.c1)}${R.r1}`
}

// ---------- units (r1 ruling 8) ----------

/** The fixed digit width K2 uses until the pack names the default font (gap X-18): px per character, plus padding. */
export const DIGIT_PX = 7
export const COL_PAD_PX = 12

/** A column width in characters of the default font's maximum digit width → pixels. */
export const colWidthPx = (chars: number): number => Math.round(chars * DIGIT_PX + COL_PAD_PX)

/** A row height in points → pixels at 96 dpi. */
export const rowHeightPx = (pt: number): number => Math.round((pt * 96) / 72)
