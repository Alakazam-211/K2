// What the live workbook view reads that the kit's TS client (k2-mpc-base
// v0.21.0, `@k2/mpc-client`) doesn't type. Everything else (frames, roster
// events, corners, ranges, cells, the grid slot, derived values) is imported
// from the kit, never redefined here. Each type below says why it is still
// ours; when the kit (or K2's host) types it, it goes.

import type { ContainerDerived, GridSlot, UnitDerived, ViewContainer, ViewSchemaJson, ViewSnapshotJson, ViewUnit, Welcome, ClaimActor } from '@k2/mpc-client'

/** A sheet's `state` (addendum r1 Item 1). Ours: the kit's `ViewContainer` has `hidden` only. */
export type SheetState = 'visible' | 'hidden' | 'very_hidden'

/**
 * The xlsx view schema: the kit's `ViewSchemaJson` with its `grid` always
 * there, plus what the xlsx addendum (r1) adds and the kit doesn't type yet:
 * `pack`, `root_order`, the containers' `state` slot and `derived.frames_only`.
 */
export interface XlsxViewSchema extends Omit<ViewSchemaJson, 'container_fields' | 'derived' | 'grid'> {
  pack?: string
  root_order?: string[]
  container_fields: ViewSchemaJson['container_fields'] & { state?: { field: string; values: Record<string, string> } }
  derived: ViewSchemaJson['derived'] & { frames_only?: string[] }
  grid: GridSlot
}

/**
 * The index's container with r1's `state` and `derived` (a sheet's merges and
 * style hints). Ours: the kit's `ViewContainer` has no `state`, and since
 * client 0.19.0 its `derived` is a container's own values (`{name: text}`,
 * the view schema's `container_derived`), a different shape from r1's, so
 * this container replaces it (as `XlsxViewUnit` does a unit's).
 */
export interface XlsxViewContainer extends Omit<ViewContainer, 'derived'> {
  state?: SheetState
  derived?: ContainerDerived | null
}

/**
 * The index's unit with r1's `derived` (`a1`: a row's formula text by cell
 * field). Ours: since client 0.18.0 the kit's `ViewUnit.derived` is a unit's
 * own values (`{name: text}`, the view schema's `unit_derived`), a different
 * shape from r1's by-field map, so this unit replaces it.
 */
export interface XlsxViewUnit extends Omit<ViewUnit, 'derived'> {
  derived?: UnitDerived | null
}

/**
 * Units as a units page carries them. On the wire a unit's `derived` is a
 * JSON object either way, and the kit's client keeps a unit as it came (it
 * reads only the names the view schema's `unit_derived` lists, which the
 * xlsx schema lists none of), so r1's units ride in the kit's page type
 * unchanged.
 */
export function asPageUnits(units: XlsxViewUnit[]): ViewUnit[] {
  return units as unknown as ViewUnit[]
}

/** The snapshot's view with the xlsx schema and containers. */
export interface XlsxViewSnapshot extends Omit<ViewSnapshotJson, 'schema' | 'containers'> {
  schema: XlsxViewSchema
  containers: XlsxViewContainer[]
}

/**
 * The welcome as the view reads it: the kit's `Welcome` plus K2's host field
 * `turn_idle_ms` (RC, r1.1). `root` and `version` (a text connection's) are
 * optional here: the view never reads them, and the goldens' welcome has
 * neither. A kit `Welcome` is always one of these.
 */
export type XlsxWelcome = Omit<Welcome, 'root' | 'version'> & Partial<Pick<Welcome, 'root' | 'version'>> & { turn_idle_ms?: number }

/** RC's `turn_end` host event, with the actor (r1). Ours: a K2 host event, not a kit frame. */
export interface TurnEndFrame {
  type: 'turn_end'
  turn: string
  actor: ClaimActor
  at_ms: number
}

/** Not a wire frame: the client's clock, in the host's time. The view ends a turn by idling with it (`turn_idle_ms`, r1.1). */
export interface ClockFrame {
  type: 'clock'
  at_ms: number
}
