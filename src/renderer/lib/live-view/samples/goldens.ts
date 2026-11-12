// K2 Cores' xlsx goldens (render-contract/goldens-xlsx, addendum r1), copied
// as they are: hand-made from the contract, not kit or pack output. Fields
// starting with `_` are annotations, never on the wire; the sample source
// strips them, and only the checks read them.

import type { ClaimActor, KeyedValue, Rect } from '../kit'
import type { CellRange, CellsValue, XlsxOpsChangeFrame, XlsxRosterEvent, XlsxViewContainer, XlsxViewSchema, XlsxViewUnit, XlsxWelcome } from '../xlsx-provisional'
import schemaJson from './view-schema.json'
import stateJson from './keyed-state-rev20.json'
import indexJson from './index-rev20.json'
import turnJson from './turn-frames.json'
import presenceJson from './presence-and-claims.json'
import casesJson from './cases-presence.json'
import gridJson from './expected-grid.json'

export interface GoldenKeyedRoot {
  name: string
  kind: 'list' | 'map'
  composite_of?: string | null
  live?: [string, KeyedValue][]
  hidden?: [string, KeyedValue][]
  entries?: [string, string | null, KeyedValue][]
}

export interface GoldenTimelineEvent {
  at_ms: number
  event: 'welcome' | 'keyed_snapshot' | 'roster_flush' | 'ops_change' | 'turn_end'
  frame?: (XlsxWelcome & { type: 'welcome' }) | { type: 'turn_end'; turn: string; actor: ClaimActor; at_ms: number }
  roster_events?: XlsxRosterEvent[]
  rev?: number
  after_rev?: number
}

export interface GoldenPeer {
  handle: string
  name: string
  kind: string
  colour: string
  away: boolean
  focus: { root: string; key: string } | null
  selection: { root: string; keys: string[] } | null
  cells: (CellsValue & { _active_a1?: string; ranges: (CellRange & { _a1?: string })[] }) | null
  pointer: { x: number; y: number } | null
  viewport: Rect | null
  _viewport_a1?: string
  mode_derived: 'editing' | 'selecting' | 'viewing'
  isSelf: boolean
  sameUser: boolean
}

export interface GoldenPoint {
  id: string
  rev: number
  peers: GoldenPeer[]
  claims_active: { id: string; actor: ClaimActor; scope: string; container: string; keys: null; ranges: (CellRange & { _a1?: string })[]; label: string; hold: boolean; started_ms: number; expires_ms: number }[]
  claims_ended: { id: string; actor: ClaimActor; how: string; at_ms: number; at_rev: number | null }[]
}

export interface GoldenGridSheet {
  sheet: string
  name: string
  state: string
  columns: Record<string, string>
  merges: string[]
  blank_rows: number[]
  rows: { number: number; address: string; hidden: boolean; cells: Record<string, Record<string, unknown>> }[]
}

/** Any case: its fields vary by form, so the checks read them loosely. */
export type GoldenCase = { id: string } & Record<string, any> // eslint-disable-line @typescript-eslint/no-explicit-any

export const GOLDEN = {
  schema: schemaJson as unknown as XlsxViewSchema,
  state: stateJson as unknown as { upto: number; roots: GoldenKeyedRoot[] },
  index: indexJson as unknown as { tier: string; rev: number; version: number | null; page: null; fonts: unknown[]; layouts: unknown[]; containers: XlsxViewContainer[]; units: (XlsxViewUnit & { _row_number?: number })[] },
  turn: turnJson as unknown as { actors: Record<string, ClaimActor & { colour: string | null }>; timeline: GoldenTimelineEvent[]; frames: (XlsxOpsChangeFrame & { _view: string })[] },
  presence: presenceJson as unknown as { points: GoldenPoint[] },
  cases: (casesJson as unknown as { cases: GoldenCase[] }).cases,
  grid: gridJson as unknown as Record<'rev20' | 'rev28' | 'rev29', GoldenGridSheet[]>,
}

export const goldenCase = (id: string): GoldenCase => {
  const c = GOLDEN.cases.find((x) => x.id === id)
  if (!c) throw new Error(`no golden case ${id}`)
  return c
}

/** A copy with every `_` annotation removed (they are never on the wire). */
export function stripAnnotations<T>(o: T): T {
  if (Array.isArray(o)) return o.map(stripAnnotations) as unknown as T
  if (o && typeof o === 'object') {
    const r: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(o as Record<string, unknown>)) if (k[0] !== '_') r[k] = stripAnnotations(v)
    return r as T
  }
  return o
}
