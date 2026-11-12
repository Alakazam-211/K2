// The kit adapter for the live workbook view: the ONLY file in lib/live-view
// (and components/LiveView) that names the kit's client types.
//
// TODAY: a minimal, exact mirror of the types this view needs from the kit's
// no-DOM TS client at k2-mpc-base `origin/main` ebcbe9e2:
//   - client/src/wire.ts   (the keyed watch's frames, the presence and claim
//                           roster events, the welcome);
//   - client/src/keyed.ts  (KeyedState, KeyedRootState, mapKey, and the
//                           watcher's op and page application, mirrored below
//                           as `applyKeyedOp` / `applyKeyedPage`);
//   - client/src/view.ts   (VIEW_TEXT_MAX).
// K2 vendors an older kit (`vendor/k2-mpc-base`, imported as `@k2/mpc-client`)
// that has no keyed.ts or view.ts yet.
//
// WHEN K2 VENDORS KIT v0.16: replace every declaration in this file with
//   `export type { … } from '@k2/mpc-client'` and
//   `export { mapKey, VIEW_TEXT_MAX } from '@k2/mpc-client'`,
// and let `KeyedWatch` itself keep the state (see `source.ts`, the
// KeyedWatchWorkbookSource seam). Nothing else in the view changes: every
// other module imports from here. Keep this file a mirror, never a fork:
// a field the kit doesn't have belongs in `xlsx-provisional.ts`.

// ---------- wire.ts ----------

export type Handle = string

/** wire.ts `ElemFocus`: the container in view. */
export interface ElemFocus {
  root: string
  key: string
}

/** wire.ts `ElemSelection`: whole elements of one list root. */
export interface ElemSelection {
  root: string
  keys: string[]
}

/** wire.ts `Point`. */
export interface Point {
  x: number
  y: number
}

/** wire.ts `Rect`: a viewport. */
export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

/** wire.ts `ClaimActor`: who holds a claim, as its events carry it. */
export interface ClaimActor {
  id: Handle
  name: string
  kind: string
  colour: string
}

/** wire.ts `LineSpan`. */
export interface LineSpan {
  root?: string
  first: number
  last: number
}

/** wire.ts `ErrorReport` (the fields this view reads). */
export interface ErrorReport {
  id: string
  message?: string
}

/** wire.ts `Welcome` (the fields this view reads). */
export interface Welcome {
  client: number
  self: Handle
  rights: 'edit' | 'read' | 'none'
  features?: string[]
  wire?: number
}

/** wire.ts `RosterEntry` (the fields this view reads). */
export interface RosterEntry {
  handle: Handle
  name: string
  colour: string
  kind: string
  role: string
  away: boolean
  selection?: ElemSelection | null
  pointer?: Point | null
  viewport?: Rect | null
  focus?: ElemFocus | null
  self?: boolean
}

/** wire.ts `RosterEvent`, the kinds this view folds. */
export type RosterEvent =
  | { kind: 'snapshot'; version: number; members: RosterEntry[]; [k: string]: unknown }
  | { kind: 'joined'; who: Handle; participant: { name: string; colour: string; kind: string }; rights?: string }
  | { kind: 'left'; who: Handle; reason?: string }
  | { kind: 'away'; who: Handle; away: boolean }
  | { kind: 'selection'; who: Handle; selection: ElemSelection | null; client?: number }
  | { kind: 'pointer'; who: Handle; pointer: Point | null; client?: number }
  | { kind: 'viewport'; who: Handle; viewport: Rect | null; client?: number }
  | { kind: 'focus'; who: Handle; focus: ElemFocus | null; client?: number }
  | {
      kind: 'claim_started_elems'
      who: Handle
      claim: string
      root: string
      keys: string[]
      note: string | null
      hold: boolean
      expires_ms: number
      version: number
      actor?: ClaimActor
      started_ms?: number
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
      [k: string]: unknown
    }

/** wire.ts `KeyedValue`. */
export type KeyedValue = string | null

/** wire.ts `KeyedSchema`. */
export interface KeyedSchema {
  name: string
  roots: { name: string; kind: string }[]
  composite?: { map: string; list: string }[]
}

/** wire.ts `ViewSrc`. */
export type ViewSrc = null | string | { list_value: true } | { derived: string } | { field: string; values: Record<string, string> }

/** wire.ts `ViewSchemaJson`. */
export interface ViewSchemaJson {
  containers: string
  units: string
  unit_container_field: string
  container_id_field: string | null
  native_unit: string
  page: { root: string; w: string; h: string } | null
  container_fields: { title: ViewSrc; hidden: ViewSrc; layout: ViewSrc }
  unit_fields: { kind: ViewSrc; name: ViewSrc; alt: ViewSrc; text: ViewSrc; hidden: ViewSrc; editable_text: ViewSrc; placeholder: ViewSrc }
  geometry: { field: string; parts: string[]; rot_unit: string } | null
  field_class: Record<string, string>
  derived: { containers: string[]; units: string[] }
}

/** wire.ts `ViewContainer`. */
export interface ViewContainer {
  key: string
  position: number
  title: string | null
  hidden: boolean
  layout: string | null
  render_hash: string | null
  fonts: number[]
  has_fields: string[]
  unit_count: number
}

/** wire.ts `ViewUnit` (the fields this view reads; the rest pass through). */
export interface ViewUnit {
  key: string
  container: string
  live: boolean
  kind: string
  z: number
  hidden: boolean
  [k: string]: unknown
}

/** wire.ts `ViewSnapshotJson`. */
export interface ViewSnapshotJson {
  schema: ViewSchemaJson
  tier: string
  rev: number | null
  version: number | null
  page: { w: number; h: number; unit: string } | null
  fonts: unknown[]
  layouts: unknown[]
  containers: ViewContainer[]
}

/** wire.ts `KeyedSnapshotFrame`. */
export interface KeyedSnapshotFrame {
  type: 'keyed_snapshot'
  seq: number
  upto: number
  schema: KeyedSchema
  view?: ViewSnapshotJson | null
  pages: number
}

/** wire.ts `KeyedUnitsPage`. */
export interface KeyedUnitsPage {
  container: string
  from: number
  units: ViewUnit[]
  next: number | null
  total: number
}

/** wire.ts `KeyedPageFrame`. */
export interface KeyedPageFrame {
  type: 'keyed_page'
  seq: number
  upto: number
  page: number
  pages: number
  root: string
  live?: [string, KeyedValue][]
  hidden?: [string, KeyedValue][]
  entries?: [string, string | null, KeyedValue][]
  text?: string
  units?: KeyedUnitsPage
}

/** wire.ts `WatchActor`: `colour` is null for a file (the host's to choose). */
export interface WatchActor {
  id: Handle
  name: string
  kind: string
  colour: string | null
}

/** wire.ts `WatchOp` (no text roots in a workbook, so no `hunks` form here). */
export type WatchOp =
  | {
      root: string
      kind: 'list'
      op: 'insert' | 'move' | 'delete' | 'restore' | 'set' | 'drop'
      key: string
      index_before: number | null
      index_after: number | null
      before?: KeyedValue
      after?: KeyedValue
      effect: 'visible' | 'hidden'
    }
  | { root: string; kind: 'map'; op: 'set' | 'delete'; key: string; field: string | null; before?: KeyedValue; after?: KeyedValue }

/** wire.ts `OpsChangeFrame` (the view deltas `units` / `containers_changed` are not read). */
export interface OpsChangeFrame {
  type: 'ops_change'
  seq: number
  rev: number
  at_ms: number
  change_kind: string
  actor: WatchActor
  client: number
  via: string
  on_behalf_of: string | null
  turn: string | null
  verb: string | null
  claim: string | null
  ops: WatchOp[]
  /** The pack's derived values; opaque in the kit (`unknown`). The xlsx shape is `XlsxFrameDerived`. */
  derived: unknown
  render_hashes: Record<string, string> | null
}

// ---------- view.ts ----------

/** view.ts `VIEW_TEXT_MAX`: A1 text per cell is cut at this many bytes. */
export const VIEW_TEXT_MAX = 1024

// ---------- keyed.ts ----------

/** keyed.ts `KeyedRootState` (no text roots in a workbook). */
export type KeyedRootState =
  | { kind: 'list'; live: [string, KeyedValue][]; hidden: Map<string, KeyedValue> }
  | { kind: 'map'; entries: Map<string, KeyedValue> }

/** keyed.ts `KeyedState`. */
export interface KeyedState {
  schema: KeyedSchema
  roots: Map<string, KeyedRootState>
}

const SEP = '\u001f'

/** keyed.ts `mapKey`: `key`, or `<key>\x1f<field>`. */
export function mapKey(key: string, field: string | null): string {
  return field === null ? key : key + SEP + field
}

/** keyed.ts `emptyState`. */
export function emptyKeyedState(schema: KeyedSchema): KeyedState {
  const roots = new Map<string, KeyedRootState>()
  for (const r of schema.roots) {
    if (r.kind === 'list') roots.set(r.name, { kind: 'list', live: [], hidden: new Map() })
    else if (r.kind === 'map') roots.set(r.name, { kind: 'map', entries: new Map() })
  }
  return { schema, roots }
}

/** keyed.ts `Misfit`: a frame that doesn't fit the state. The watch asks for a fresh snapshot. */
export class KeyedMisfit extends Error {}

function fail(why: string): never {
  throw new KeyedMisfit(why)
}

function has(o: object, k: string): boolean {
  return Object.prototype.hasOwnProperty.call(o, k)
}

/** keyed.ts `applyOp`, exactly: one op on the state (mutates it); anything that doesn't fit throws `KeyedMisfit`. */
export function applyKeyedOp(state: KeyedState, o: WatchOp): void {
  const r = state.roots.get(o.root) ?? fail(`no root ${o.root}`)
  if (o.kind === 'map') {
    if (r.kind !== 'map') fail(`${o.root} isn't a map`)
    const k = mapKey(o.key, o.field)
    if (o.op === 'set') {
      if (!has(o, 'after')) fail(`set of ${k} without a value`)
      r.entries.set(k, o.after ?? null)
    } else if (!r.entries.delete(k)) fail(`delete of ${k}, absent`)
    return
  }
  if (r.kind !== 'list') fail(`${o.root} isn't a list`)
  const { live, hidden } = r
  const at = (i: number | null): number => (i !== null && i < live.length && live[i][0] === o.key ? i : fail(`${o.op} ${o.key}: not at ${i}`))
  const after = (): KeyedValue => (has(o, 'after') ? (o.after ?? null) : fail(`${o.op} ${o.key} without a value`))
  if (o.effect === 'hidden') {
    if (o.op === 'set') {
      if (!hidden.has(o.key)) fail(`hidden set of ${o.key}, not deleted`)
      hidden.set(o.key, after())
    } else if (o.op === 'drop') {
      if (!hidden.delete(o.key)) fail(`drop of ${o.key}, not deleted`)
    } else if (o.op !== 'move') fail(`hidden ${o.op}`)
    return
  }
  switch (o.op) {
    case 'insert': {
      const i = o.index_after
      if (i === null || i > live.length) fail(`insert of ${o.key} at ${i}`)
      if (hidden.has(o.key) || live.some(([k]) => k === o.key)) fail(`insert of ${o.key}, already there`)
      live.splice(i, 0, [o.key, after()])
      return
    }
    case 'move': {
      const b = at(o.index_before)
      const a = o.index_after
      if (a === null || a >= live.length) fail(`move of ${o.key} to ${a}`)
      const [e] = live.splice(b, 1)
      live.splice(a, 0, e)
      return
    }
    case 'delete': {
      const [[k, v]] = live.splice(at(o.index_before), 1)
      hidden.set(k, v)
      return
    }
    case 'restore': {
      const a = o.index_after
      if (a === null || a > live.length) fail(`restore of ${o.key} at ${a}`)
      if (!hidden.has(o.key)) fail(`restore of ${o.key}, not deleted`)
      const v = hidden.get(o.key) as KeyedValue
      hidden.delete(o.key)
      live.splice(a, 0, [o.key, v])
      return
    }
    case 'set':
      live[at(o.index_after)][1] = after()
      return
    case 'drop':
      if (!hidden.delete(o.key)) fail(`drop of ${o.key}, not deleted`)
      return
  }
}

/** keyed.ts `applyPage`: one page's run onto the state being built (units onto `units`). Exact but for
 *  text pages, which a workbook never has (a text page fails as "doesn't fit"). */
export function applyKeyedPage(state: KeyedState, units: Map<string, ViewUnit[]>, f: KeyedPageFrame): void {
  const parts = ['live', 'hidden', 'entries', 'text', 'units'].filter((k) => has(f, k))
  if (parts.length !== 1) fail(`page ${f.page} holds ${parts.join(', ') || 'nothing'}`)
  if (f.units) {
    const have = units.get(f.units.container) ?? []
    if (f.units.from !== have.length) fail(`units of ${f.units.container} from ${f.units.from}, after ${have.length}`)
    units.set(f.units.container, [...have, ...f.units.units])
    return
  }
  const r = state.roots.get(f.root) ?? fail(`page for a root not in the schema: ${f.root}`)
  if (f.live && r.kind === 'list') r.live.push(...f.live.map(([k, v]): [string, KeyedValue] => [k, v]))
  else if (f.hidden && r.kind === 'list') for (const [k, v] of f.hidden) r.hidden.set(k, v)
  else if (f.entries && r.kind === 'map') for (const [k, field, v] of f.entries) r.entries.set(mapKey(k, field), v)
  else fail(`page ${f.page} doesn't fit root ${f.root}`)
}

/**
 * Not in the kit: copy-on-write for the view's immutable state. The roots an
 * op list touches are copied once, then `applyKeyedOp` runs on the copies,
 * so the state before stays whole (the view keeps it for "was / now"). With
 * the vendored KeyedWatch this goes: the watch keeps one mutable state.
 */
export function applyKeyedOps(state: KeyedState, ops: readonly WatchOp[]): KeyedState {
  const roots = new Map(state.roots)
  const copied = new Set<string>()
  for (const o of ops) {
    if (copied.has(o.root)) continue
    const r = roots.get(o.root)
    if (!r) continue
    copied.add(o.root)
    roots.set(o.root, r.kind === 'list' ? { kind: 'list', live: r.live.map((e) => [e[0], e[1]]), hidden: new Map(r.hidden) } : { kind: 'map', entries: new Map(r.entries) })
  }
  const next: KeyedState = { schema: state.schema, roots }
  for (const o of ops) applyKeyedOp(next, o)
  return next
}
