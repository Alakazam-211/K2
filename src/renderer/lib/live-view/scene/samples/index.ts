// The scene's sample data: the mock's seven g-* blocks and K2 Cores' two
// pinned BLEND snapshots (k2-mpc-blend 686cd0da, crates/blend/tests/
// kit_view_golden/, byte for byte), lifted from mockup-live-scene-v2.html.
// K2 Cores' pinned expected-view snapshot replaces the first six blocks and
// K2 Render's view list and preview records replace `previews`. Fields
// starting with `_` are annotations: the tab never reads them.

import type { Actor, Frame, IndexJson, PresenceStart, SchemaJson, SettledFrame } from '../types'
import schemaJson from './schema.json'
import stateJson from './state.json'
import indexJson from './index.json'
import framesJson from './frames.json'
import presenceJson from './presence.json'
import savesJson from './saves.json'
import previewsJson from './previews.json'
import pinnedOutlinerJson from './pinned-outliner.json'
import pinnedRigJson from './pinned-rig.json'

export interface StateRoot {
  name: string
  kind: string
  composite_of?: string
  live?: [string, string][]
  entries?: [string, string | null, string][]
}

export interface TimelineEvent {
  event: string
  at_ms: number
  seq?: number
  rev?: number
  after_rev?: number
  frame?: Record<string, unknown>
  roster_events?: Record<string, unknown>[]
  _note?: string
}

export interface PresencePoint {
  id: string
  after: string
  rev: number
  on: Record<string, string[]>
  focus: Record<string, string>
  views?: Record<string, string>
  claims: { claim: string; by: string; keys: string[]; note: string }[]
  modes: Record<string, string>
}

export interface SaveCase {
  id: string
  title: string
  base_rev: number
  synced_rev?: number
  frame_rev?: number
  frame?: Frame
  settled: SettledFrame
  declined?: { code: string; version: string }
  means: string
}

export interface ViewEntry {
  view: string
  name: string
  kind: string
  object?: string
  active?: boolean
  _name?: string
}

export interface Golden {
  schema: SchemaJson & { _golden?: { sample?: boolean } }
  state: { _golden?: { sample?: boolean }; upto: number; roots: StateRoot[] }
  index: IndexJson
  turn: { _golden?: { sample?: boolean }; actors: Record<string, Actor>; timeline: TimelineEvent[]; frames: Frame[] }
  pc: { _golden?: { sample?: boolean }; start: PresenceStart[]; points: PresencePoint[] }
  saves: { _golden?: { sample?: boolean }; cases: SaveCase[] }
  previews: {
    _golden?: { sample?: boolean }
    views: ViewEntry[]
    background: { alpha: boolean } & Record<string, unknown>
    bounds: Record<string, { approx?: string; why?: string } & Record<string, unknown>>
    records: { rev: number; view: string; state: string; width: number; ms: number; without_modifiers: boolean }[]
  }
  pinned: { outliner: IndexJson; rig: { index: IndexJson; crowd_pages: { budget: number; pages: { container: string; from: number; next: number | null; total: number; units: string[] }[] } } }
}

export const GOLD: Golden = {
  schema: schemaJson as unknown as Golden['schema'],
  state: stateJson as unknown as Golden['state'],
  index: indexJson as unknown as IndexJson,
  turn: framesJson as unknown as Golden['turn'],
  pc: presenceJson as unknown as Golden['pc'],
  saves: savesJson as unknown as Golden['saves'],
  previews: previewsJson as unknown as Golden['previews'],
  pinned: { outliner: pinnedOutlinerJson as unknown as IndexJson, rig: pinnedRigJson as unknown as Golden['pinned']['rig'] },
}

/** Every block says it is sample data. */
export const SAMPLE = [GOLD.schema, GOLD.state, GOLD.turn, GOLD.pc, GOLD.saves, GOLD.previews].some((g) => !!g._golden?.sample)
