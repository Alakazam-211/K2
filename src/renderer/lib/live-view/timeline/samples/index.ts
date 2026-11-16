// The live timeline's sample data, lifted from the council-passed mock
// (mockup-live-timeline-v2.html) block for block:
//
//   g-schema    the pack's keyed and view schema (c9739a65), with LiveLimits;
//   g-state     the sample's keyed state at rev 30 (snapshot pages);
//   g-index     its view index (units with derived tc, containers with derived kind_label and kind);
//   g-frames    the turn: welcome, snapshot, roster flushes, ops_change rev 31–35, a refusal, an outside save;
//   g-presence  three pinned presence points of the turn;
//   g-saved     what an outside app saved (the kept-both case);
//   g-cut       the pack test's Cut fixture with its expectations;
//   g-media     what K2 Render will hold per source (hashes made up; `_footage` is the mock's drawing);
//   g-golden    the pack's view golden snapshots (cut, cut-edited, cut-trim-in with its change), as pinned.
//
// Sample data, never a file: fields starting with `_` are annotations.
// Loaded only by the sample (a lazy chunk), never by a real file's tab.

import type { PagedRoot, PinnedSnapshot } from '../adapter'
import type { SourceFacts } from '../picture'
import type { Actor, SampleTimePosition, TimelineFrame, TrackDerived, Viewport } from '../schema'
import schemaJson from './g-schema.json'
import stateJson from './g-state.json'
import indexJson from './g-index.json'
import framesJson from './g-frames.json'
import presenceJson from './g-presence.json'
import savedJson from './g-saved.json'
import cutJson from './g-cut.json'
import mediaJson from './g-media.json'
import goldenJson from './g-golden.json'

export interface GoldSchema {
  keyed: { name: string; roots: { name: string; kind: string }[]; composite: { map: string; list: string }[] }
  view: {
    containers: string
    units: string
    unit_container_field: string
    container_id_field: string | null
    native_unit: string
    page: unknown
    container_fields: { title: string; hidden: unknown; layout: unknown }
    unit_fields: { kind: unknown; name: string; text: unknown; hidden: string; editable_text: unknown; [k: string]: unknown }
    geometry: { field: string; parts: string[]; rot_unit: string }
    derived: unknown
    [k: string]: unknown
  }
  limits: { max_items: number; max_markers: number; max_tracks: number; keyed_max_entries: number; [k: string]: unknown }
}

export interface IndexUnit {
  key: string
  container: string
  kind: string
  name: string
  bbox: { x: number; y: number; w: number; h: number } | null
  hidden: boolean
  editable_text: string | null
  text: unknown
  derived: { tc: string }
}

export interface IndexContainerFull {
  key: string
  position: number
  title: string
  derived?: TrackDerived
}

export interface RosterEvent {
  kind: string
  who?: string
  claim?: string
  actor?: Actor
  root?: string
  keys?: string[]
  note?: string
  hold?: boolean
  started_ms?: number
  expires_ms?: number
  how?: string
  at_ms?: number
  at_rev?: number | null
  timePosition?: SampleTimePosition | null
  viewAddress?: string | null
  [k: string]: unknown
}

export interface TurnEvent {
  event: string
  at_ms: number
  rev?: number
  seq?: number
  frame?: Record<string, unknown>
  roster_events?: RosterEvent[]
  host?: { type: string; rev?: number }
}

export interface PresencePeer {
  handle: string
  name: string
  kind: string
  colour: string
  isSelf: boolean
  away: boolean
  focus: unknown
  selection: unknown
  viewport: Viewport | null
  mode_derived: string
  timePosition: SampleTimePosition | null
}

export interface PresencePoint {
  id: string
  rev: number
  at_ms: number
  _at: string
  peers: PresencePeer[]
  claims_active: { id: string }[]
}

export type GoldenSnapshot = PinnedSnapshot & { marker_list: string; state: { timeline: Record<string, string> } & Record<string, unknown> }

export interface Gold {
  schema: GoldSchema
  state: { upto: number; roots: PagedRoot[]; _sample?: unknown }
  index: { containers: IndexContainerFull[]; units: IndexUnit[] }
  turn: { actors: Record<string, Actor>; timeline: TurnEvent[]; frames: TimelineFrame[] }
  pc: { points: PresencePoint[] }
  saved: { roots: PagedRoot[] }
  cut: { roots: PagedRoot[]; _pinned: { expect: { lanes: string[]; units: number; item1: { key: string; container: string; x: number; w: number; name: string }; tc_item1: string; F: number }; view_rs_unit_tests: Record<string, string | null> } }
  media: { renderer: { filmstrip: { every_s: number } }; media: Record<string, SourceFacts> }
  golden: { cut: GoldenSnapshot; 'cut-edited': GoldenSnapshot; 'cut-trim-in': GoldenSnapshot & { change: TimelineFrame } }
}

export const GOLD: Gold = {
  schema: schemaJson as unknown as GoldSchema,
  state: stateJson as unknown as Gold['state'],
  index: indexJson as unknown as Gold['index'],
  turn: framesJson as unknown as Gold['turn'],
  pc: presenceJson as unknown as Gold['pc'],
  saved: savedJson as unknown as Gold['saved'],
  cut: cutJson as unknown as Gold['cut'],
  media: mediaJson as unknown as Gold['media'],
  golden: goldenJson as unknown as Gold['golden'],
}

