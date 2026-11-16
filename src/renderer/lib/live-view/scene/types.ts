// The scene model's shapes: the kit's wire frames as the BLEND pack sends
// them (kit v0.18.0 wire.ts, view.rs, place.rs), the folded doc, and the
// sample's simulation state. Loose where the mock's sample adds annotations
// (`_`-fields), exact where the tab reads a field.

export interface Actor {
  id: string
  name: string
  kind: string
  colour: string
}

export interface Rot {
  euler?: string
  angles?: number[]
  quat?: number[]
}

export interface Trs {
  loc: number[]
  rot: Rot
  scale: number[]
}

export interface PlacementStep {
  trs?: Trs
  matrix?: number[]
}

export interface Placement {
  relative_to: string | null
  steps: PlacementStep[]
  approx?: string
}

export interface Space {
  length_unit: string
  metres_per_unit: number | null
  up: string
  handed: string
}

export interface SchemaJson {
  keyed: { name: string; roots: { name: string; kind: string }[]; composite?: { map: string; list: string }[] }
  view: {
    containers: string
    units: string
    unit_container_field: string
    container_id_field: string | null
    native_unit: string
    page: unknown
    geometry: unknown
    container_fields: { title: string; hidden: string; layout: string | null }
    unit_fields: { kind: unknown; name: string; hidden: string; text: string | null } & Record<string, unknown>
    relations?: { name: string; from: string; field: string; to: string; _provisional?: string }[]
    field_class: Record<string, string>
    unit_derived?: string[]
    space: { unit_fields: string[]; container_fields: string[]; precision: string }
  }
}

export interface IndexContainer {
  key: string
  position: number
  title: string | null
  hidden: boolean
  layout: string | null
  unit_count: number
  parent?: string
}

export interface IndexUnit {
  key: string
  container: string
  kind: string
  name: string
  hidden: boolean
  placement?: Placement | null
  derived?: Record<string, string>
}

export interface IndexJson {
  tier: string
  rev: number
  containers: IndexContainer[]
  space: Space
  units: IndexUnit[]
}

export interface Op {
  root: string
  kind: string
  op: string
  key: string
  field?: string | null
  before?: string | null
  after?: string | null
  index_after?: number
}

export interface UnitDelta {
  key: string
  container: string
  ops: string[]
  fields: string[]
  placement_after?: Placement | null
  geom_before?: unknown
  geom_after?: unknown
  hidden_after?: unknown
  placeholder_type?: unknown
  _note?: string
}

export interface Frame {
  type: string
  seq: number
  rev: number
  at_ms: number | null
  change_kind: string
  actor: Actor
  client: number
  via: string
  on_behalf_of: unknown
  turn: string | null
  verb: string | null
  claim: string | null
  ops: Op[]
  units: UnitDelta[]
  containers_changed: string[]
  containers: string[]
  derived: { units: Record<string, Record<string, string | null>> } | null
  render_hashes: unknown
  view_space?: Space
  _constructed?: boolean
}

/** The kit's verb_refused frame (c23d2349). */
export interface Refusal {
  type: string
  at_ms: number | null
  actor: Actor
  client: number
  turn: string | null
  verb: string
  unit: string
  code: string
  holder: string | null
  claim: string | null
  _constructed?: boolean
}

/** The kit's outside_save frame (02ddb6aa). */
export interface SettledFrame {
  type?: string
  outcome: 'ingested' | 'kept_both' | 'declined' | 'missing'
  rev?: number
  kept?: number
  copy?: { name: string; path: string; doc?: string }
  code?: string
  message?: string
}

export interface PresenceStart {
  handle: string
  name: string
  kind: string
  colour: string
  away: boolean
  isSelf?: boolean
  focus: { root: string; key: string } | null
  selection: { root: string; keys: string[] } | null
  viewAddress?: string
}

/** A folded doc: the keyed state plus the view snapshot's per-unit derived text and placements. */
export interface Doc {
  rev: number
  lists: Record<string, string[]>
  kinds: Record<string, string>
  props: Record<string, Record<string, string>>
  scene: Record<string, string>
  units: Record<string, { derived: Record<string, string>; placement: Placement | null }>
  space: Space | null
  tier: string
  removed: Record<string, { kind: string; props: Record<string, string>; rev: number; at: number }>
  added: Record<string, number>
  generated?: boolean
  pinned?: boolean
}
