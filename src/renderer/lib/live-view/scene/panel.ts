// What the scene's side panel says, worked out from the model: pure, so the
// counts, rows and words are tested without a browser. Four sections, in
// one PANEL order (the mock's v2.2 order): Activity, People and holds,
// Outliner, Properties. At narrow widths People and Properties start closed
// (who's here stays in the header), Activity and the outliner stay open.
// The words are the mock's (live-scene-v2-notes.md, v2.5).

import { GOLD } from './samples'
import { ID16, same, xformParts } from './math'
import { UNITS, metres, nameOf, numsOf, objOf, worldOf, type Nums, type SceneObj } from './fold'
import { flatRows, sceneTree, type Row, type SceneTree } from './tree'
import { actorOf, changesOf, framePhrase, phraseOne, refusalText, type Touched } from './wording'
import { agentId, agentTarget, claimOn, whoOn, type Claim, type LogEntry, type Peer, type SimState } from './sim'
import { fromMockView } from './sample-views'
import { SCENE_VIEW, parseView } from './view-address'
import { objectOfView, objectView, viewName } from './views'
import type { Actor, Doc, Frame } from './types'

export type SectionId = 'activity' | 'people' | 'outliner' | 'props'
export const PANEL: readonly { id: SectionId; title: string; grow?: boolean }[] = [
  { id: 'activity', title: 'Activity' },
  { id: 'people', title: 'People and holds' },
  { id: 'outliner', title: 'Outliner', grow: true },
  { id: 'props', title: 'Properties' },
]
/** A section's open state until the viewer toggles it: all open, except People and Properties when narrow. */
export const openByDefault = (id: SectionId, narrow: boolean): boolean => !(narrow && (id === 'people' || id === 'props'))

export const isLive = (S: SimState): boolean => S.tier === 'live' && !S.loading && S.conn === 'ok'

/** The viewer's own pick (theirs, else the scenario's), when it is still in the doc. */
export function myPick(S: SimState, local: string | null | undefined): string | null {
  const a = local !== undefined && local !== null ? local : S.pick
  return a && S.D && S.D.props[a] ? a : null
}
/** What Properties shows: your pick, else what the agent is on. */
export function target(S: SimState, local: string | null | undefined): string | null {
  if (!S.D) return null
  const a = myPick(S, local) || agentTarget(S)
  return a && (S.D.props[a] || S.D.removed[a]) ? a : null
}

/** A view address (K2 Render's, view-address.ts) in words: "Scene", "Camera 2", "Table, framed", "Turntable". */
export const viewShort = (D: Doc, v: string): string => viewName(D, v)

/** A peer's view as K2 Render names it (the sample keeps the mock's names inside). */
export const peerView = (S: SimState, h: string): string | null => fromMockView(S.D, S.peers[h]?.view, S.ui.tt ?? 0)

/** The view on screen, as K2 Render names it: whom you follow's, else yours (`view`, already K2 Render's) or the scenario's. */
export function viewNow(S: SimState, o: { followingId?: string | null; view?: string | null; pick?: string | null } = {}): string {
  const w = o.followingId && S.peers[o.followingId] ? o.followingId : null
  let v = (w ? peerView(S, w) : o.view || fromMockView(S.D, S.view, S.ui.tt ?? 0)) || SCENE_VIEW
  if (v === '@obj:*') {
    const a = myPick(S, o.pick) || agentTarget(S)
    v = a && S.D ? objectView(S.D, a) : SCENE_VIEW
  }
  const kind = parseView(v)?.kind
  if (!kind || ((kind === 'object' || kind === 'camera') && !(S.D && objectOfView(S.D, v)))) v = SCENE_VIEW
  return v
}

/** An agent is done once its turn has ended and it hasn't acted since. */
export function agentDone(S: SimState, h: string): boolean {
  for (let i = S.log.length - 1; i >= 0; i--) {
    const x = S.log[i]
    if (x.actor !== h) continue
    if (x.kind === 'turn_end') return true
    if (x.kind === 'frame' || x.kind === 'refused' || x.kind === 'claim') return false
  }
  return false
}

/** "now", "40 s ago", "2 min ago"; nothing while not connected. */
export function ago(S: SimState, at: number | null): string {
  if (at == null || S.at == null || (S.conn && S.conn !== 'ok')) return ''
  const s = Math.max(0, Math.round((S.at - at) / 1000))
  return s < 2 ? 'now' : s < 60 ? s + ' s ago' : Math.round(s / 60) + ' min ago'
}

/** "arranging the dining set" → "dining set" (the full note is on hover and in the panel). */
export const shortHold = (l: string | null | undefined): string => (l || '').replace(/^[a-z]+ing\s+(the\s+)?/i, '')

// ── People and holds ──

export interface PersonRow {
  id: string
  name: string
  kind: string
  colour: string
  away: boolean
  self: boolean
  mode: string
  /** Where they look, in words, and whether it's a camera. */
  view: string
  camera: boolean
  following: boolean
  /** What they're doing: their holds' notes, "done", "on Floor", "away" or "viewing". */
  what: string
  holding: boolean
}

export function peopleRows(S: SimState, o: { followingId?: string | null; view?: string | null; pick?: string | null } = {}): PersonRow[] {
  const ps = Object.values(S.peers).sort((x, y) => (x.isSelf ? 1 : 0) - (y.isSelf ? 1 : 0))
  return ps.map((p) => {
    const cl = S.claims.filter((c) => actorOf(c.actor).id === p.handle)
    const view = p.isSelf ? viewNow(S, o) : peerView(S, p.handle)
    const done = p.kind === 'agent' && agentDone(S, p.handle)
    const what = cl.length ? cl.map((c) => c.label).join(', ') : done ? 'done' : p.selection && p.selection.keys.length ? 'on ' + p.selection.keys.map((k) => nameOf(S.D, k)).join(', ') : p.away ? 'away' : 'viewing'
    const v = view || SCENE_VIEW
    return { id: p.handle, name: p.isSelf ? 'You' : p.name, kind: p.kind, colour: p.colour, away: p.away, self: !!p.isSelf, mode: p.mode ?? 'viewing', view: viewShort(S.D, v), camera: v.startsWith('@cam'), following: o.followingId === p.handle, what, holding: cl.length > 0 }
  })
}

// ── Activity ──

export type ActivityItem =
  | { kind: 'now'; actor: Actor; text: string }
  | { kind: 'change'; actor: Actor; who: string; text: string; when: string; cur: boolean; outside?: boolean }
  | { kind: 'let-go'; actor: Actor; text: string; when: string; cur: boolean }
  | { kind: 'refused'; who: string; text: string; holder: Actor | null; detail: string; cur: boolean }
  | { kind: 'saved'; actor: Actor; who: string; text: string; when: string; cur: boolean }
  | { kind: 'said'; to: Actor; text: string; ref: string | null; refName: string; cur: boolean }
  | { kind: 'reply'; actor: Actor; text: string; ref: string | null; refName: string; cur: boolean }
  | { kind: 'done'; actor: Actor; when: string; cur: boolean }

const SAVEW: Record<string, string> = { ingested: 'now in the live file', kept_both: 'now the live file; earlier edits kept as a copy', declined: 'live edits paused', missing: 'file moved or deleted; live version kept' }
export const EMPTY_ACTIVITY = 'No changes yet. What agents do shows here first.'

/** Activity: a Now line per agent hold, the conversation above it when it is newest, then the last few changes, newest first (four lines in all). */
export function activityItems(S: SimState, fileName: string): ActivityItem[] {
  const L = S.log.filter((e) => ['frame', 'refused', 'settled', 'turn_end', 'said', 'reply', 'claim_end'].includes(e.kind))
  const folded: (LogEntry & { settled?: unknown })[] = []
  L.forEach((e, k) => {
    const prev = L[k - 1]
    if (e.kind === 'settled' && k > 0 && prev.kind === 'frame' && actorOf(prev.actor).kind === 'file') {
      folded[folded.length - 1] = { ...prev, settled: e.v }
      return
    }
    folded.push(e)
  })
  const now = isLive(S) ? S.claims.filter((c) => actorOf(c.actor).kind === 'agent' && S.peers[actorOf(c.actor).id]).map((c) => ({ kind: 'now' as const, actor: actorOf(c.actor), text: c.label || 'working' })) : []
  const hist = folded.slice(-(4 - Math.min(2, now.length))).reverse()
  const talk: typeof hist = []
  while (hist.length && (hist[0].kind === 'said' || hist[0].kind === 'reply')) talk.push(hist.shift()!)
  const refName = (r: string | null): string => (!r ? '' : r.startsWith('@obj:') ? nameOf(S.D, r) : viewShort(S.D, r))
  const one = (e: (typeof hist)[number]): ActivityItem => {
    const cur = e.i === S.i
    if (e.kind === 'frame' && e.settled) {
      const p = framePhrase(e.frame, e.before, e.after)
      return { kind: 'change', actor: actorOf(e.actor), who: 'Saved outside K2', text: ` · ${SAVEW[(e.settled as { outcome: string }).outcome]}: ${p.text}`, when: ago(S, e.at), cur, outside: true }
    }
    if (e.kind === 'frame') {
      const p = framePhrase(e.frame, e.before, e.after)
      return { kind: 'change', actor: actorOf(e.actor), who: p.who, text: p.sep + p.text, when: ago(S, e.at), cur }
    }
    if (e.kind === 'claim_end') return { kind: 'let-go', actor: actorOf(e.actor), text: ` let go of the ${e.claim?.label ? shortHold(e.claim.label) : 'what it held'}`, when: ago(S, e.at), cur }
    if (e.kind === 'refused') {
      const t0 = refusalText(e.r, S.D)
      const still = !e.r.claim || S.claims.some((c) => c.id === e.r.claim)
      return { kind: 'refused', who: t0.who, text: ' ' + (still ? t0.text : t0.text.replace(/ is holding it$/, ' was holding it')), holder: e.r.holder ? actorOf(e.r.holder) : null, detail: t0.detail, cur }
    }
    if (e.kind === 'settled') {
      const a = actorOf('kit:external')
      if (e.v.outcome === 'missing') return { kind: 'saved', actor: a, who: fileName, text: ' was moved or deleted outside K2', when: ago(S, e.at), cur }
      if (e.v.outcome === 'declined') return { kind: 'saved', actor: a, who: 'Saved outside K2 again', text: ', in a newer format · live edits paused', when: ago(S, e.at), cur }
      return { kind: 'saved', actor: a, who: 'Saved outside K2', text: ` · ${SAVEW[e.v.outcome] || e.v.outcome}`, when: ago(S, e.at), cur }
    }
    if (e.kind === 'said') return { kind: 'said', to: actorOf(e.to), text: e.text, ref: e.ref, refName: refName(e.ref), cur }
    if (e.kind === 'reply') return { kind: 'reply', actor: actorOf(e.actor), text: e.text, ref: e.ref, refName: refName(e.ref), cur }
    return { kind: 'done', actor: actorOf(e.actor), when: ago(S, e.at), cur }
  }
  return [...talk.map(one), ...now.slice(0, 2), ...hist.map(one)]
}

// ── The outliner ──

export interface RecentChange {
  actor: Actor
  age: number
  colour: string
  who: string
  phrase: string
  xb: ReturnType<typeof xformParts>
  xa: ReturnType<typeof xformParts>
  touched: Touched
  f: Frame
}

/** The changes of the last two frames, by object: who, what, and the numbers before and after. */
export function recentMap(S: SimState): Record<string, RecentChange> {
  const m: Record<string, RecentChange> = {}
  for (const r of S.recent ?? []) {
    const a = actorOf(r.f.actor)
    for (const c of changesOf(r.f)) {
      if (m[c.addr] && m[c.addr].age <= r.age) continue
      m[c.addr] = { actor: a, age: r.age, colour: a.colour, who: a.name, phrase: phraseOne(c, r.before, r.after).text, xb: c.fields.xform ? xformParts(c.fields.xform.before) : null, xa: c.fields.xform ? xformParts(c.fields.xform.after) : null, touched: c, f: r.f }
    }
  }
  return m
}

export type OutRow = Row | { t: 'more'; addr: string; n: number; depth: number } | { t: 'skel'; addr: string; depth: number }

export interface OutlinerModel {
  /** The section head's count: how many objects. */
  count: string | null
  rows: OutRow[]
  /** The search found nothing. */
  none: boolean
  query: string
  T: SceneTree | null
}

export function outlinerModel(S: SimState, o: { collapsed: ReadonlySet<string>; q: string }): OutlinerModel {
  if (!S.D || S.tier === 'none') return { count: null, rows: [], none: false, query: '', T: null }
  const n = (S.D.lists[UNITS] ?? []).length
  const q = (o.q || '').trim()
  if (S.loading) return { count: n.toLocaleString('en'), rows: Array.from({ length: 14 }, (_, i) => ({ t: 'skel' as const, addr: 'skel' + i, depth: i % 3 })), none: false, query: q, T: null }
  const T = sceneTree(S.D)
  const rows: OutRow[] = [...flatRows(T, o.collapsed, q)]
  // Pages still arriving: "N more arriving…" at the end of their collection.
  for (const [c, p] of Object.entries(S.pages ?? {})) {
    if (p.loaded >= p.total || o.collapsed.has(c)) continue
    const k = rows.findIndex((r) => r.t === 'coll' && r.addr === c)
    if (k < 0) continue
    const d = rows[k].depth
    let e = k + 1
    while (e < rows.length && !(rows[e].t === 'coll' && rows[e].depth <= d)) e++
    rows.splice(e, 0, { t: 'more', addr: c + '#more', n: p.total - p.loaded, depth: d + 1 })
  }
  const none = !!q && flatRows(T, new Set(), q).hits === 0
  return { count: n.toLocaleString('en'), rows, none, query: q, T }
}

export interface ObjRowInfo {
  obj: SceneObj
  /** Driven, quaternion or linked: what can't change live, with its why. */
  badge: { kind: 'driven' | 'quat' | 'linked'; tip: string } | null
  hold: { colour: string; name: string; label: string } | null
  /** People (not agents, not you) whose selection is on it. */
  on: Peer[]
  /** The agent at work is on it. */
  agent: Actor | null
  acting: boolean
  recent: RecentChange | null
}

const BADGE: Record<string, { kind: 'driven' | 'quat' | 'linked'; tip: string }> = {
  a: { kind: 'driven', tip: 'Driven: animation sets it, so it can’t be moved live' },
  t: { kind: 'quat', tip: 'Turns by quaternion: its rotation can’t change live' },
  '0': { kind: 'linked', tip: 'Linked from another file: read-only' },
}

export function objRow(S: SimState, T: SceneTree, a: string, recent: Record<string, RecentChange>): ObjRowInfo {
  const live = isLive(S)
  const cl = live ? claimOn(S, a) : null
  const ag = live && agentTarget(S) === a ? actorOf(agentId(S) || 'agent:atlas') : null
  return {
    obj: T.objs[a],
    badge: BADGE[T.objs[a].edit] ?? null,
    hold: cl ? { colour: actorOf(cl.actor).colour, name: actorOf(cl.actor).name, label: cl.label || '' } : null,
    on: live ? whoOn(S, a).filter((p) => !p.isSelf && p.kind !== 'agent') : [],
    agent: ag,
    acting: !!ag && S.log.length > 0 && S.log[S.log.length - 1].i === S.i,
    recent: recent[a] ?? null,
  }
}

// ── Properties ──

export type PropsModel =
  | { state: 'loading' }
  | { state: 'empty'; text: string }
  | { state: 'nopick'; text: string }
  | {
      state: 'obj'
      addr: string
      obj: SceneObj
      mine: boolean
      nums: Nums | null
      lenUnit: 'm' | 'units'
      /** The values that just changed, and what they were. */
      just: RecentChange | null
      hold: Claim | null
      limit: string | null
      attached: string | null
    }

const LIMIT: Record<string, string> = {
  a: 'Animation moves this object, so its box shows where it is saved, not where it plays.',
  t: 'It turns by quaternion, so its rotation can’t change live.',
  '0': 'It comes from another file: read-only here, and not in the picture.',
}

/** An approximate box: the placement is driven, or the local bounds miss what a modifier adds. */
export function approxOf(D: Doc, a: string): 'driven' | string | null {
  const u = D.units?.[a]
  const o = objOf(D, a)
  if (o.edit === 'a' || u?.placement?.approx === 'driven') return 'driven'
  // The sample's listing: approx is a bool, the why is approx_reason.
  const b = GOLD.previews.bounds?.[a]
  return b?.approx === true ? (b.approx_reason ?? 'modifiers') : null
}

export const fmt = (n: number): string => (Math.abs(n) < 0.0005 ? 0 : n).toFixed(3)
export const angle = (r: number, rad = false): string => fmt(rad ? r : (r * 180) / Math.PI)

export function propsModel(S: SimState, pick: string | null | undefined, recent: Record<string, RecentChange>): PropsModel {
  if (S.loading) return { state: 'loading' }
  if (S.D && !(S.D.lists[UNITS] ?? []).length) return { state: 'empty', text: 'No objects yet.' }
  const a = target(S, pick)
  if (!a || !S.D || !S.D.props[a] || !(S.tier === 'live' || S.tier === 'paused')) return { state: 'nopick', text: 'Pick an object to see its numbers.' }
  const o = objOf(S.D, a)
  const rc = recent[a] ?? null
  const just = rc && rc.age === 0 && rc.xb ? rc : null
  let attached: string | null = null
  if (o.parent && S.D.props[o.parent]) {
    const wm = worldOf(S.D, a)
    const pl = S.D.units[a]?.placement
    const inv = !!pl && !!pl.steps[0]?.matrix && !same(pl.steps[0].matrix, ID16)
    attached = `Attached to ${nameOf(S.D, o.parent)}${inv ? ', which also turns and moves it' : ''}. Where it sits in the scene: ${wm ? [12, 13, 14].map((i) => fmt(wm[i])).join(', ') : '?'}.`
  }
  return {
    state: 'obj',
    addr: a,
    obj: o,
    mine: myPick(S, pick) === a,
    nums: numsOf(o),
    lenUnit: metres(S.D) ? 'm' : 'units',
    just,
    hold: claimOn(S, a),
    limit: LIMIT[o.edit] ?? (approxOf(S.D, a) === 'modifiers' ? 'Its box is approximate: a modifier adds parts the box leaves out.' : null),
    attached,
  }
}
