// The xlsx-only parts of the live workbook contract that the kit's TS client
// doesn't ship yet (rendering-data contract, xlsx addendum r1). Named exactly
// as the addendum names them. Each one is replaced by the kit's own type when
// the kit ships it (kit items VG, VC, VK, V1, and the pack's derived values);
// until then the view reads them through these declarations only.

import type { ClaimActor, Handle, KeyedPageFrame, LineSpan, OpsChangeFrame, RosterEntry, RosterEvent, ViewContainer, ViewSchemaJson, ViewSnapshotJson, ViewUnit, Welcome } from './kit'

/** provisional: replaced when the kit ships it. A sheet's `state` (addendum r1 Item 1, `Container.state`). */
export type SheetState = 'visible' | 'hidden' | 'very_hidden'

/** provisional: replaced when the kit ships it. The grid slot of the view schema (kit item VG, addendum §1.3). */
export interface GridSlot {
  rows: string
  cols: string
  cells: string
  col_fields: string
  col_container_field: string
  row_fields_start: string
  col_of_cell_field: string
  numbering: {
    rows: { gap_field: string; first: number }
    cols: { gap_field: string; first: number }
  }
  col_label: 'letters'
  max: { rows: number; cols: number }
  cell: { encoding: 'json'; type: string; value: string; formula: string; formula_kind: string; style: string }
  row_fields: { hidden: string; height: string; height_unit: 'pt' }
  col_field_names: { hidden: string; width: string; width_unit: string }
  viewport: 'cells'
  /** r1.1: the per-cell derived values the kit puts on each `cells` page (`["a1"]`, or `{names: ["a1"]}`). */
  cell_derived?: string[] | { names: string[] }
  /** A missing table, or an index the table lacks, means `default` (r1.1). */
  style_hint?: { from: string; table: string; default?: StyleHint }
}

/** provisional: replaced when the kit ships it. The xlsx view schema: the kit's plus `grid`, `root_order` and the `state` slot (r1). */
export interface XlsxViewSchema extends Omit<ViewSchemaJson, 'container_fields' | 'derived'> {
  pack?: string
  root_order?: string[]
  container_fields: ViewSchemaJson['container_fields'] & { state?: { field: string; values: Record<string, string> } }
  derived: { containers: string[]; units: string[]; frames_only?: string[] }
  grid: GridSlot
}

/** provisional: replaced when the kit ships it. A corner (Item 3.4): the last keyed row (column) at or before the cell, plus offsets. */
export interface Corner {
  row: string | null
  row_off: number
  col: string | null
  col_off: number
}

/** provisional: replaced when the kit ships it. A range by two corners; `start` where it began, `end` where it is now. */
export interface CellRange {
  start: Corner
  end: Corner
}

/** provisional: replaced when the kit ships it. The `cells` presence value (kit item VC): a caret plus up to 16 ranges on one sheet. */
export interface CellsValue {
  container: string
  active: Corner | null
  ranges: CellRange[]
}

/** provisional: replaced when the kit ships it. The style hint (r1 ruling 10, Item 5). */
export interface StyleHint {
  bold: boolean
  fill: string | null
  numfmt: string
}

/** provisional: replaced when the kit ships it. A sheet's style table, keyed by style index (`Container.derived.styles`). */
export type StyleTable = Record<string, StyleHint>

/** provisional: replaced when the kit ships it. `Container.derived` for xlsx: `merges` and `styles`. */
export interface ContainerDerived {
  merges?: CellRange[]
  styles?: StyleTable
}

/** provisional: replaced when the kit ships it. `Unit.derived` for xlsx: A1 text per formula field (`null`: no longer a formula). */
export interface UnitDerived {
  a1?: Record<string, string | null> | null
}

/** provisional: replaced when the kit ships it. A frame's `written` (r1 ruling 1): what a verb wrote, by corners at the frame's rev. */
export interface Written {
  container: string
  ranges: CellRange[]
}

/** provisional: replaced when the kit ships it. One of a frame's `bands` (r1 ruling 4): a delete's numbers are before the change, an insert's after. */
export interface Band {
  container: string
  axis: 'rows' | 'cols'
  op: 'insert' | 'delete'
  from: number
  to: number
  numbers: 'before' | 'after'
}

/** provisional: replaced when the kit ships it. An xlsx frame's `derived` (Item 2). */
export interface XlsxFrameDerived {
  units?: Record<string, UnitDerived>
  containers?: Record<string, ContainerDerived>
  written?: Written
  bands?: Band[]
}

/** provisional: replaced when the kit ships it. The index's container with r1's `state` and `derived`. */
export interface XlsxViewContainer extends ViewContainer {
  state?: SheetState
  derived?: ContainerDerived | null
}

/** provisional: replaced when the kit ships it. The index's unit with `derived.a1` (kit V1). */
export interface XlsxViewUnit extends ViewUnit {
  derived?: UnitDerived | null
}

/** provisional: replaced when the kit ships it. The snapshot's view with the xlsx schema and containers. */
export interface XlsxViewSnapshot extends Omit<ViewSnapshotJson, 'schema' | 'containers'> {
  schema: XlsxViewSchema
  containers: XlsxViewContainer[]
}

/** provisional: replaced when the kit ships it. The welcome with RC's `turn_idle_ms` (K2's). */
export interface XlsxWelcome extends Welcome {
  turn_idle_ms?: number
}

/** provisional: replaced when the kit ships it. A roster entry with `cells`. */
export interface XlsxRosterEntry extends RosterEntry {
  cells?: CellsValue | null
}

/** provisional: replaced when the kit ships it. The new roster events: `cells` (VC) and `claim_started_cells` (VK); `claim_ended` with `lines` as row numbers. */
export type XlsxRosterEvent =
  | Exclude<RosterEvent, { kind: 'snapshot' } | { kind: 'claim_ended' }>
  | { kind: 'snapshot'; version: number; members: XlsxRosterEntry[] }
  | { kind: 'cells'; who: Handle; cells: CellsValue | null; client?: number }
  | {
      kind: 'claim_started_cells'
      who: Handle
      claim: string
      container: string
      ranges: CellRange[]
      note: string | null
      hold: boolean
      expires_ms: number
      version: number
      actor: ClaimActor
      started_ms: number
    }
  | {
      kind: 'claim_ended'
      who: Handle
      claim: string
      how: string
      version: number
      actor?: ClaimActor
      at_ms?: number
      at_rev?: number | null
      lines?: LineSpan | null
    }

/** provisional: replaced when the kit ships it. RC's `turn_end` host event, with the actor (r1). */
export interface TurnEndFrame {
  type: 'turn_end'
  turn: string
  actor: ClaimActor
  at_ms: number
}

/** provisional: replaced when the kit ships it. The ops_change frame with xlsx's derived. */
export type XlsxOpsChangeFrame = Omit<OpsChangeFrame, 'derived'> & { derived: XlsxFrameDerived | null }

/** provisional: replaced when the kit ships it. A keyed page with r1.1's per-cell derived values: a `cells` page carries the A1 text of the formula cells in that page (row address, then cell field). */
export interface XlsxKeyedPageFrame extends KeyedPageFrame {
  derived?: { a1?: Record<string, Record<string, string>> } | null
}

/** Not a wire frame: the client's clock, in the host's time. The view ends a turn by idling with it (`turn_idle_ms`, r1.1). */
export interface ClockFrame {
  type: 'clock'
  at_ms: number
}
