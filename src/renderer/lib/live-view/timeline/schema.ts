// The timeline pack's shapes, as the live timeline reads them: its keyed
// roots (the pack's live/mod.rs SCHEMA and COMPOSITE), its view schema
// (live/view.rs VIEW), and the frames it sends (the kit's ops_change,
// verb_refused, outside_save and claim_ended). The book is the view's own
// form of the keyed state; adapter.ts makes one from the sample's pages, the
// pinned goldens or the kit's KeyedWatch, so the model never sees where the
// state came from.

import type { KeyedValue, TimePositionJson } from '@k2/mpc-client'

/** The keyed roots, in the schema's order (the first page is `timeline`). */
export const KEYED_ROOTS = [
  { name: 'timeline', kind: 'map' },
  { name: 'tracks', kind: 'list' },
  { name: 'items', kind: 'list' },
  { name: 'markers', kind: 'list' },
  { name: 'media', kind: 'list' },
  { name: 'track_props', kind: 'map' },
  { name: 'item_props', kind: 'map' },
  { name: 'marker_props', kind: 'map' },
  { name: 'media_props', kind: 'map' },
] as const

/** Each list root's fields map (a composite root). */
export const COMPOSITE = [
  { map: 'track_props', list: 'tracks' },
  { map: 'item_props', list: 'items' },
  { map: 'marker_props', list: 'markers' },
  { map: 'media_props', list: 'media' },
] as const

/** The view schema's slots the lanes read. */
export const VIEW = {
  containers: 'tracks',
  units: 'items',
  unit_container_field: 'track',
  container_title: 'name',
  hidden: 'off',
  geometry: 'geom',
  editable_text: { field: 'edit', values: { '0': 'never', '1': 'plain' } as Record<string, string> },
} as const

export const CONT = 'tracks'
export const UNIT = 'items'
export const MKR = 'markers'
export const MED = 'media'
export const TLR = 'timeline'
export const PROPS: Record<string, string> = Object.fromEntries(COMPOSITE.map((c) => [c.list, c.map]))
export const LIST_OF: Record<string, string> = Object.fromEntries(COMPOSITE.map((c) => [c.map, c.list]))

export type KV = KeyedValue

/** A container's derived values (pack 82dfb6c0): the lane's label ("V1") and kind. */
export interface TrackDerived {
  kind_label?: string
  kind?: string
}

/** The keyed state as the view holds it. A map root that isn't composite keeps its value under field ''. */
export interface Book {
  rev: number
  lists: Record<string, [string, KV][]>
  hidden: Record<string, [string, KV][]>
  maps: Record<string, Record<string, Record<string, KV>>>
  derived: { containers: Record<string, TrackDerived> }
}

export interface Actor {
  id: string
  name: string
  kind: string
  colour: string | null
}

export type TimelineOp =
  | { root: string; kind: 'map'; op: 'set' | 'delete'; key: string; field: string | null; before?: KV; after?: KV }
  | { root: string; kind: 'list'; op: 'insert' | 'delete' | 'move' | 'set'; key: string; index_before: number | null; index_after: number | null; before?: KV; after?: KV; effect?: string }

export interface TimelineUnitDelta {
  key: string
  container: string | null
  ops: string[]
  geom_before: string | null
  geom_after: string | null
  fields: string[]
  hidden_after: boolean | null
  placeholder_type: string | null
}

/** The per-clip timecode a change carries (pack 09c7d6d3): the start, as given. */
export interface TimelineFrameDerived {
  units?: Record<string, { tc?: string | { geom?: string } | null }>
  containers?: Record<string, TrackDerived>
}

/** The kit's ops_change for a timeline doc. `_` fields are the sample's annotations. */
export interface TimelineFrame {
  type: 'ops_change'
  seq: number
  rev: number
  at_ms: number
  change_kind: string
  actor: Actor
  client: number | string
  via: string
  on_behalf_of: string | null
  turn: string | null
  verb: string | null
  claim: string | null
  ops: TimelineOp[]
  units?: TimelineUnitDelta[]
  containers_changed?: unknown[]
  containers?: string[]
  derived: TimelineFrameDerived | null
  render_hashes: Record<string, string> | null
  /** The sample's expected wording (a check reads it). */
  _view?: string
  _constructed?: boolean
}

/** The kit's verb_refused (c23d2349). */
export interface Refusal {
  at_ms: number
  actor: Actor
  client: number | string
  turn: string | null
  verb: string
  unit: string
  code: string
  holder: Actor | null
  claim: string | null
  _view?: string
}

export interface Claim {
  id: string
  actor: Actor
  root: string
  keys: string[]
  note: string
  hold: boolean
  started_ms: number
  expires_ms: number
  note_track?: string
}

/** The kit's outside_save without its envelope; `_at_ms` and `_summary` are the sample's. */
export type OutsideSaveEvent =
  | { outcome: 'ingested'; rev: number; _at_ms: number; _summary?: string }
  | { outcome: 'kept_both'; rev: number; kept: string; copy: { name: string; path: string; doc?: string }; _at_ms: number; who?: string }
  | { outcome: 'declined'; code: string; message: string; detail?: unknown; _at_ms: number }
  | { outcome: 'missing'; _at_ms: number }

/** Rational time at the timeline's rate (presence, ruled 2026-11-13). */
export interface RationalTime {
  value: number
  rate: string
}

export interface Viewport {
  from: RationalTime
  to: RationalTime
  tracks: [string, string]
}

/** A time position as the sample carries it: the kit's, plus the mock's lane note. */
export type SampleTimePosition = TimePositionJson & { _track?: string }

/** The view's own pointer, derived from the kit's time position. */
export interface Pointer {
  at: RationalTime
  track: string | null
}

export interface Selection {
  root: string
  keys: string[]
}

export interface Focus {
  root: string
  key: string
}
