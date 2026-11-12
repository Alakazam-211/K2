// The pack's derived values kept by cell field (the grid's `cell_derived`,
// `["a1"]` for xlsx: a formula's text as people read it), kept the way the
// kit's `KeyedWatch` keeps them (k2-mpc-base client/src/keyed.ts at v0.17.0,
// WIRE.md "Kept by cell field"): a snapshot's rows pages carry them for their
// entries, each change's `derived.units.<row>.<name>` updates them (`null`
// removes one), a fresh snapshot replaces them all. Read with `cellDerived`,
// the same lookup as `KeyedWatch.cellDerived(name, unit, field)`, so the real
// source maps onto it one to one.
//
// Why this is still ours: `KeyedWatch` keeps these in one mutable map behind a
// private field; the view keeps a value per book (immutable), so a change's
// "was" stays readable. Only names the grid lists in `cell_derived` are kept,
// as the kit does.

import type { FrameDerived, GridSlot, KeyedPageFrame } from '@k2/mpc-client'
import type { XlsxViewUnit } from './not-in-kit'

/** The derived value that holds a formula's A1 text. */
export const A1 = 'a1'

/** What is kept: name, then unit (a row's address), then field, to text. */
export type CellDerivedValues = Readonly<Record<string, Readonly<Record<string, Readonly<Record<string, string>>>>>>

export const NO_CELL_DERIVED: CellDerivedValues = {}

/** `KeyedWatch.cellDerived`: the value `name` keeps for field `field` of row `unit`, `null` when it has none. */
export function cellDerived(of: { cellDerived: CellDerivedValues }, name: string, unit: string, field: string): string | null {
  return of.cellDerived[name]?.[unit]?.[field] ?? null
}

/** The names the grid keeps by cell field (none without a grid). */
export function cellDerivedNames(grid: GridSlot | null | undefined): string[] {
  return grid?.cell_derived ?? []
}

type Writes = [name: string, unit: string, field: string, text: string | null][]

/** The values with `writes` applied (a text sets one, `null` removes it); the input is never changed. */
function withWrites(values: CellDerivedValues, writes: Writes): CellDerivedValues {
  if (!writes.length) return values
  const next: Record<string, Record<string, Record<string, string>>> = { ...values }
  const copied = new Set<string>()
  for (const [name, unit, field, text] of writes) {
    const byUnit = (next[name] = copied.has(name) ? next[name] : { ...(next[name] ?? {}) })
    const k = name + '\u001f' + unit
    const row = (byUnit[unit] = copied.has(k) ? byUnit[unit] : { ...(byUnit[unit] ?? {}) })
    copied.add(name).add(k)
    if (text === null) delete row[field]
    else row[field] = text
  }
  return next
}

/** A snapshot's page: its `derived` for the names the grid keeps (keyed.ts `onPage`). */
export function withPageDerived(values: CellDerivedValues, f: KeyedPageFrame, grid: GridSlot | null | undefined): CellDerivedValues {
  if (!f.derived) return values
  const names = new Set(cellDerivedNames(grid))
  const writes: Writes = []
  for (const [name, byUnit] of Object.entries(f.derived)) {
    if (!names.has(name)) continue
    for (const [unit, byField] of Object.entries(byUnit)) {
      for (const [field, text] of Object.entries(byField)) if (typeof text === 'string') writes.push([name, unit, field, text])
    }
  }
  return withWrites(values, writes)
}

/** A change's `derived.units` (keyed.ts `keepCellDerived`). */
export function withFrameDerived(values: CellDerivedValues, d: FrameDerived | null | undefined, grid: GridSlot | null | undefined): CellDerivedValues {
  const units = d?.units
  const names = grid?.cell_derived
  if (!units || !names) return values
  const writes: Writes = []
  for (const [unit, u] of Object.entries(units)) {
    for (const name of names) {
      const byField = u[name]
      if (!byField || typeof byField !== 'object') continue
      for (const [field, text] of Object.entries(byField as Record<string, unknown>)) {
        if (text === null || typeof text === 'string') writes.push([name, unit, field, text])
      }
    }
  }
  return withWrites(values, writes)
}

/**
 * Not the kit's rule: the view index's own units' `derived` (`Unit.derived.a1`,
 * on the units pages), over what the rows pages carried. In the goldens the
 * two agree (the sample builds the pages' values from the units); kept so the
 * view reads no less than it did before the kit had `cell_derived`.
 */
export function withUnitsDerived(values: CellDerivedValues, units: Iterable<readonly XlsxViewUnit[]>, grid: GridSlot | null | undefined): CellDerivedValues {
  const names = cellDerivedNames(grid)
  const writes: Writes = []
  for (const list of units) {
    for (const u of list) {
      for (const name of names) {
        const m = u.derived?.[name]
        if (!m || typeof m !== 'object') continue
        for (const [field, text] of Object.entries(m as Record<string, unknown>)) if (typeof text === 'string') writes.push([name, u.key, field, text])
      }
    }
  }
  return withWrites(values, writes)
}
