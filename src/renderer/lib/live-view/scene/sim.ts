// The sample's state, event by event: the doc, who's here (the kit's roster:
// focus, selection, viewAddress, away), claims, the log Activity reads, the
// picture's state (the still on screen and what K2 Render says of the
// newest), outside saves and refusals, and what the scenario asks of the
// view (a pick, a view, a search). Pure functions over plain data.

import { GOLD } from './samples'
import { clone, strip } from './math'
import { SCENE_VIEW } from './view-address'
import { applyFrame } from './fold'
import { multiPlaceFrame, placeFrame, renameFrame, type PlaceChange } from './frames'
import { ATLAS, actorOf } from './wording'
import { docFromIndex } from './fold'
import type { Actor, Doc, Frame, IndexJson, Refusal, SettledFrame } from './types'

export interface Peer {
  handle: string
  name: string
  kind: string
  colour: string
  away: boolean
  focus: { root: string; key: string } | null
  selection: { root: string; keys: string[] } | null
  /** The peer's viewAddress (kit main 02ddb6aa). */
  view: string | null
  mode?: 'editing' | 'selecting' | 'viewing'
  isSelf?: boolean
}

export interface Claim {
  id: string
  actor: Actor
  root: string
  keys: string[]
  label: string
  hold: boolean
  started_ms: number | null
  expires_ms: number | null
}

export interface PreviewEvent {
  rev: number
  view?: string
  state: 'rendering' | 'ready' | 'failed' | 'none'
  ms?: number
  reused_from?: number | null
  without_modifiers?: boolean
  reason?: string
  width?: number
}

export interface Picture {
  shown: number | null
  want: number | null
  state: 'none' | 'ready' | 'rendering' | 'stale' | 'failed' | 'nopic'
  ms: number | null
  reused: number | null
  without_modifiers: boolean
  reason: string | null
  landed?: boolean
  recovered?: boolean
  retrying?: boolean
  again?: boolean
}

export type LogEntry =
  | { i: number; kind: 'frame'; frame: Frame; actor: string; rev: number; at: number | null; before: Doc; after: Doc }
  | { i: number; kind: 'claim'; claim: Claim; actor: string; at: number | null }
  | { i: number; kind: 'claim_end'; id: string; how: string; claim: Claim | undefined; actor: string; at: number | null }
  | { i: number; kind: 'turn_end'; turn: string; actor: string; at: number | null }
  | { i: number; kind: 'refused'; r: Refusal; actor: string; at: number | null }
  | { i: number; kind: 'settled'; v: SettledFrame; actor: string; at: number | null }
  | { i: number; kind: 'said'; to: string; text: string; ref: string | null; actor: string; at: number | null }
  | { i: number; kind: 'reply'; actor: string; text: string; ref: string | null; at: number | null }

export interface SimEvent {
  kind: string
  frame?: Frame | null
  made?: Frame | null
  multi?: [PlaceChange[], string, string[]?]
  rename?: [string, string, string, string?]
  big?: [string, number[], string?]
  claim?: Claim
  id?: string
  how?: string
  at_ms?: number | null
  at_rev?: number | null
  actor?: Actor | string
  turn?: string
  who?: string
  type?: string
  value?: unknown
  r?: Refusal
  v?: unknown
  p?: PreviewEvent
  to?: string
  from?: string
  text?: string
  ref?: string | null
  tone?: string
  ms?: number
  addr?: string | null
  delta?: { key: string; parent_after: string | null }
  page?: { container: string; from: number; next: number | null; total: number; units: string[] }
  menuCur?: string
  tt?: number
  q?: string
  collapsed?: string[]
  expand?: string[]
  tier?: string
}

export interface SimStep {
  at: number | null
  label: string | null
  events: SimEvent[]
  what?: string | null
  start?: boolean
  kind?: string | null
  point?: string
  dwell?: number
}

export interface SimState {
  D: Doc
  tier: string
  self: string
  peers: Record<string, Peer>
  claims: Claim[]
  ended: { id: string; how: string; at_ms: number | null; at_rev: number | null; claim: Claim | undefined }[]
  log: LogEntry[]
  recent: { f: Frame; before: Doc; after: Doc; age: number }[]
  at: number | null
  follow: boolean
  turnEnds: Record<string, number | null>
  turnIdle: number | null
  group: SimStep | null
  i: number
  toast: { tone?: string; text?: string; ms?: number } | null
  banner: unknown
  loading: unknown
  conn: string
  notice: (SettledFrame & { i: number }) | null
  ui: { q: string; collapsed: string[]; menuCur?: string; tt?: number }
  big: string | null
  pick: string | null
  rights: string
  pv: Picture
  revDocs: Record<number, Doc>
  view: string | null
  watch: string | false
  lastFollow: string | null
  compose: { to: string; text: string; ref: string | null } | null
  pages: Record<string, { loaded: number; total: number; next: number | null; arrived?: boolean }> | null
  sendFail: boolean
  sentRetry?: boolean
  collMoved: { key: string; to: string | null; i: number } | null
  callout: { r: Refusal; i: number } | null
  menu: boolean
  layout: number
  lostSince?: unknown
  pinned?: boolean
  pinnedName?: string
  pageIx?: IndexJson
  loaded?: string[]
}

const WELCOME = (GOLD.turn.timeline.find((e) => e.event === 'welcome')?.frame ?? {}) as { self?: string; turn_idle_ms?: number; rights?: string }
export const SELF: string = WELCOME.self || (GOLD.pc.start.find((q) => q.isSelf) as { handle: string }).handle

const PEER_FIELDS = ['handle', 'name', 'kind', 'colour', 'away', 'focus', 'selection', 'viewAddress'] as const
export function peerFrom(q: Record<string, unknown>): Peer {
  const p = Object.fromEntries(PEER_FIELDS.map((k) => [k, clone(strip(q[k] === undefined ? null : q[k]))])) as Record<string, unknown>
  p.view = p.viewAddress || null
  delete p.viewAddress
  return p as unknown as Peer
}

export function ensurePeer(S: SimState, h: string): Peer {
  if (!S.peers[h]) {
    const a = actorOf(h)
    S.peers[h] = { handle: h, name: a.name, kind: a.kind, colour: a.colour, away: false, focus: null, selection: null, view: SCENE_VIEW }
  }
  return S.peers[h]
}

function claimRecord(ev: Record<string, unknown>): Claim {
  return { id: ev.claim as string, actor: clone((ev.actor as Actor) || actorOf(ev.who as string)), root: ev.root as string, keys: (ev.keys as string[]).slice(), label: ev.note as string, hold: ev.hold as boolean, started_ms: ev.started_ms as number, expires_ms: ev.expires_ms as number }
}
export { claimRecord }

export function modeOf(p: Peer, claims: Claim[]): 'editing' | 'selecting' | 'viewing' {
  const mine = claims.filter((c) => actorOf(c.actor).id === p.handle)
  const sel = p.selection?.keys ?? []
  if (mine.some((c) => c.keys.some((k) => sel.includes(k)))) return 'editing'
  return sel.length ? 'selecting' : 'viewing'
}
export const claimOn = (S: SimState, a: string): Claim | null => S.claims.find((c) => c.keys.includes(a)) ?? null
export const whoOn = (S: SimState, a: string): Peer[] => Object.values(S.peers).filter((p) => p.selection && p.selection.keys.includes(a))

const PV0 = (GOLD.previews.records ?? []).find((r) => r.view === SCENE_VIEW) ?? null
export function pvBlank(rev: number | null): Picture {
  return { shown: rev, want: rev, state: rev == null ? 'none' : 'ready', ms: PV0 ? PV0.ms : null, reused: null, without_modifiers: false, reason: null }
}

function applyPreview(S: SimState, p: PreviewEvent): void {
  const v = S.pv
  if (p.state === 'rendering') {
    v.retrying = v.state === 'failed'
    v.want = p.rev
    v.state = v.shown == null ? 'rendering' : 'stale'
    v.reason = null
  } else if (p.state === 'ready') {
    v.shown = p.rev
    v.want = p.rev
    v.state = 'ready'
    v.ms = p.ms != null ? p.ms : v.ms
    v.reused = p.reused_from != null ? p.reused_from : null
    v.without_modifiers = !!p.without_modifiers
    v.reason = null
    v.landed = true
    v.recovered = !!v.retrying
    v.retrying = false
    v.again = false
  } else if (p.state === 'failed') {
    v.again = !!v.retrying
    v.retrying = false
    v.want = p.rev
    v.state = 'failed'
    v.reason = p.reason || null
  } else if (p.state === 'none') {
    v.shown = null
    v.state = 'nopic'
    v.reason = p.reason || 'no_mesh_read'
  }
  S.revDocs[p.rev] = S.revDocs[p.rev] || S.D
}

export function blankState(D: Doc, self: string): SimState {
  return {
    D, tier: 'live', self, peers: {}, claims: [], ended: [], log: [], recent: [], at: null, follow: true, turnEnds: {}, turnIdle: WELCOME.turn_idle_ms || null, group: null, i: 0, toast: null, banner: null, loading: null, conn: 'ok', notice: null,
    ui: { q: '', collapsed: [] }, big: null, pick: null, rights: WELCOME.rights || 'edit', pv: pvBlank(D ? D.rev : null), revDocs: D ? { [D.rev]: D } : {}, view: null, watch: false, lastFollow: null, compose: null, pages: null,
    sendFail: false, collMoved: null, callout: null, menu: false, layout: 1,
  }
}

/** The agent at work: the last agent to make a change or be refused, else the first agent here. */
export function agentId(S: SimState): string | null {
  for (let i = S.log.length - 1; i >= 0; i--) {
    const x = S.log[i]
    const a = actorOf(x.actor)
    if (a.kind === 'agent' && (x.kind === 'frame' || x.kind === 'refused')) return a.id
  }
  const p = Object.values(S.peers).find((x) => x.kind === 'agent')
  return p ? p.handle : null
}
export function agentTarget(S: SimState): string | null {
  const a = agentId(S)
  const k = a ? S.peers[a]?.selection?.keys : undefined
  return k && k.length ? k[0] : null
}

export function applyEvent(S: SimState, e: SimEvent, i: number): void {
  const k = e.kind
  if (k === 'frame') {
    const f =
      e.frame ||
      e.made ||
      (e.multi
        ? multiPlaceFrame(S.D, e.multi[0], S.D.rev + 1, e.multi[1], e.multi[2])
        : e.rename
          ? renameFrame(S.D, e.rename[0], e.rename[1], S.D.rev + 1, e.rename[2], e.rename[3])
          : placeFrame(S.D, e.big![0], e.big![1], S.D.rev + 1, e.big![2] || ATLAS))
    const before = S.D
    e.made = f
    S.D = applyFrame(before, f)
    S.revDocs[f.rev] = S.D
    S.recent = [{ f, before, after: S.D, age: 0 }].concat(S.recent.map((r) => ({ ...r, age: r.age + 1 }))).filter((r) => r.age < 2)
    S.log.push({ i, kind: 'frame', frame: f, actor: actorOf(f.actor).id, rev: f.rev, at: f.at_ms, before, after: S.D })
  } else if (k === 'claim_started') {
    const c = clone(e.claim as Claim)
    S.claims.push(c)
    S.log.push({ i, kind: 'claim', claim: c, actor: actorOf(c.actor).id, at: c.started_ms })
  } else if (k === 'claim_ended') {
    const c = S.claims.find((x) => x.id === e.id)
    S.claims = S.claims.filter((x) => x.id !== e.id)
    S.ended.push({ id: e.id as string, how: e.how as string, at_ms: e.at_ms ?? null, at_rev: e.at_rev ?? null, claim: c })
    S.log.push({ i, kind: 'claim_end', id: e.id as string, how: e.how as string, claim: c, actor: actorOf(e.actor || (c && c.actor)).id, at: e.at_ms ?? null })
  } else if (k === 'turn_end') {
    S.turnEnds[e.turn as string] = e.at_ms ?? null
    S.log.push({ i, kind: 'turn_end', turn: e.turn as string, actor: actorOf(e.actor).id, at: e.at_ms ?? null })
  } else if (k === 'welcome') {
    const w = e.frame as unknown as { self: string; turn_idle_ms: number; rights: string }
    S.self = w.self
    S.turnIdle = w.turn_idle_ms
    S.rights = w.rights
  } else if (k === 'presence') {
    const p = ensurePeer(S, e.who as string) as unknown as Record<string, unknown>
    p[e.type === 'viewAddress' ? 'view' : (e.type as string)] = clone(e.value)
  } else if (k === 'refused') {
    // verb_refused (kit c23d2349) reaches every viewer who can read; K2 shows what arrives.
    if (!S.rights) return
    const r = clone(e.r as Refusal)
    S.log.push({ i, kind: 'refused', r, actor: actorOf(r.actor).id, at: r.at_ms })
    S.callout = { r, i }
  } else if (k === 'settled') {
    // The outside_save frame once the save settles: ingested, kept_both, declined (edits pause) or missing (the live version is kept).
    const v = clone(e.v as SettledFrame)
    S.log.push({ i, kind: 'settled', v, actor: 'kit:external', at: S.at })
    S.notice = { i, ...v }
    if (v.outcome === 'declined' || v.outcome === 'missing') S.tier = 'paused'
  } else if (k === 'preview') applyPreview(S, e.p as PreviewEvent)
  else if (k === 'view') {
    S.view = e.v as string
    if (S.watch) S.lastFollow = S.watch
    S.watch = false
  } else if (k === 'watch') {
    S.watch = e.v === false ? false : e.who || agentId(S) || ATLAS
    if (S.watch) S.lastFollow = null
  } else if (k === 'leave') {
    const who = e.who as string
    const p = S.peers[who]
    const nm = p ? p.name : actorOf(who).name
    delete S.peers[who]
    if (S.watch === who) {
      S.view = (p && p.view) || S.view
      S.watch = false
      S.toast = { tone: 'info', text: nm + ' left the file', ms: 4000 }
    }
    if (S.lastFollow === who) S.lastFollow = null
  } else if (k === 'say') S.compose = { to: e.to as string, text: e.text || '', ref: e.ref || null }
  else if (k === 'sendfail') {
    S.compose = { to: e.to as string, text: e.text as string, ref: e.ref || null }
    S.sendFail = true
  } else if (k === 'sent') {
    S.sentRetry = !!S.sendFail
    S.sendFail = false
    S.log.push({ i, kind: 'said', to: e.to as string, text: e.text as string, ref: e.ref ?? null, actor: S.self, at: S.at })
    S.compose = { to: e.to as string, text: '', ref: e.ref || null }
  } else if (k === 'reply') S.log.push({ i, kind: 'reply', actor: e.from as string, text: e.text as string, ref: e.ref || null, at: S.at })
  else if (k === 'layout') S.layout = e.v as number
  else if (k === 'menu') S.menu = e.v as boolean
  else if (k === 'toast') S.toast = e
  else if (k === 'banner') S.banner = e
  else if (k === 'loading') S.loading = e.v
  else if (k === 'tier') {
    S.tier = e.tier as string
    if (S.tier !== 'live') {
      S.peers = {}
      ensurePeer(S, S.self)
      S.claims = []
    }
  } else if (k === 'conn') S.conn = e.v as string
  else if (k === 'ui') {
    if ('menuCur' in e) S.ui.menuCur = e.menuCur
    if ('tt' in e) S.ui.tt = e.tt
    if ('q' in e) S.ui.q = e.q as string
    if (e.collapsed) S.ui.collapsed = e.collapsed.slice()
    if (e.expand) S.ui.collapsed = S.ui.collapsed.filter((c) => !e.expand!.includes(c))
  } else if (k === 'pick') {
    S.pick = e.addr ?? null
    S.follow = false
  } else if (k === 'lost') S.lostSince = e.v
  else if (k === 'collmove') {
    const d = e.delta as { key: string; parent_after: string | null }
    S.D = clone(S.D)
    S.D.pinned = true
    if (d.parent_after) S.D.props[d.key].parent = d.parent_after
    else delete S.D.props[d.key].parent
    S.collMoved = { key: d.key, to: d.parent_after, i }
  } else if (k === 'page') {
    const pg = e.page!
    S.loaded = (S.loaded ?? []).concat(pg.units)
    S.D = docFromIndex(S.pageIx as IndexJson, new Set(S.loaded))
    S.pages = S.pages ?? {}
    S.pages[pg.container] = { loaded: pg.from + pg.units.length, total: pg.total, next: pg.next, arrived: pg.next == null }
    S.revDocs[S.D.rev] = S.D
  } else if (k === 'follow') S.follow = e.v as boolean
}

export function applyGroup(S: SimState, g: SimStep, i: number): void {
  S.i = i
  S.group = g
  S.toast = null
  S.sentRetry = false
  S.menu = false
  if (S.callout && S.callout.i < i) S.callout = null
  if (S.pv) S.pv.landed = false
  if (g.at != null) S.at = g.at
  for (const e of g.events) applyEvent(S, e, i)
}

export function finish(S: SimState): SimState {
  for (const p of Object.values(S.peers)) {
    p.mode = modeOf(p, S.claims)
    p.isSelf = p.handle === S.self
  }
  return S
}
