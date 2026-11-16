// The live timeline's state, folded from events: books from change frames,
// presence, holds, refusals, outside saves, the connection, and the viewer's
// own local state (whom they follow, the preview). A scenario is a base
// state plus steps of events; `simulate` plays it to a step. The sample's
// scenarios run through it, and so will the real source's frames (T7).

import { actorOf, EXT_ID } from './actors'
import { applyFrame, clone } from './adapter'
import { builtOf } from './diff'
import { modelOf } from './fold'
import { DEFAULT_PV, type PreviewState } from './picture'
import { tpPtr, verbLanding } from './playheads'
import { CONT, LIST_OF, UNIT, type Book, type Claim, type Focus, type OutsideSaveEvent, type Pointer, type RationalTime, type Refusal, type SampleTimePosition, type Selection, type TimelineFrame, type Viewport } from './schema'

export interface Peer {
  handle: string
  name: string
  kind: string
  colour: string | null
  away: boolean
  focus: Focus | null
  selection: Selection | null
  viewport: Viewport | null
  pointer: Pointer | null
  timePosition: SampleTimePosition | null
  viewAddress: string | null
  mode?: 'editing' | 'selecting' | 'viewing'
  isSelf?: boolean
}

export interface SayRef {
  at: RationalTime
  item?: string | null
  track?: string | null
}

export type LogEntry =
  | { i: number; kind: 'frame'; frame: TimelineFrame; actor: string; rev: number; at: number; before: Book; after: Book }
  | { i: number; kind: 'claim'; claim: Claim; actor: string; at: number }
  | { i: number; kind: 'claim_end'; id: string; how: string; claim?: Claim; actor: string; at: number; keys?: string[]; was?: string | null }
  | { i: number; kind: 'turn_end'; turn: string; actor: string; at: number }
  | { i: number; kind: 'save'; save: OutsideSaveEvent; actor: string; at: number }
  | { i: number; kind: 'refused'; actor: string; r: Refusal; at: number; book: Book }
  | { i: number; kind: 'say'; actor: string; to: string; text: string; ref: SayRef | null; reply: boolean; at: number; failed: boolean; why: string | null; tries: number }
  | { i: number; kind: 'snap'; actor: string; text: string; at: number }

/** What the viewer does locally in a step (the sample's script of it). */
export interface LocalStep {
  follow?: boolean | string
  back?: string
  center?: number
  loop?: { f0: number; f1: number; rev: number; who: string; verb: string }
  left?: string
  skipped?: boolean
  retry?: 'trying' | 'failed'
  caught?: number
  at?: number
}

export type SimEvent =
  | { kind: 'frame'; frame: TimelineFrame; quiet?: boolean }
  | { kind: 'claim_started'; claim: Claim }
  | { kind: 'claim_ended'; id: string; how: string; at_ms: number; at_rev?: number | null; actor?: unknown; keys?: string[] }
  | { kind: 'turn_end'; turn: string; actor: unknown; at_ms: number }
  | { kind: 'welcome'; frame: { self: string; turn_idle_ms?: number; rights?: string } }
  | { kind: 'snapshot'; seq?: number }
  | { kind: 'presence'; who: string; type: string; value: unknown }
  | { kind: 'host'; host: { type: string; rev?: number }; at_ms?: number }
  | { kind: 'outside_save'; save: OutsideSaveEvent }
  | { kind: 'loading'; v: LoadingState | null }
  | { kind: 'tier'; tier: string; decline?: { code: string; title: string; text: string; detail: string } | null }
  | { kind: 'zoom'; fit?: boolean; ppf?: number; center?: number }
  | { kind: 'local'; v: LocalStep }
  | { kind: 'refused'; r: Refusal }
  | { kind: 'conn'; state: string | null }
  | { kind: 'book'; B: Book; note?: string; at_ms?: number }
  | { kind: 'pv'; v: Partial<PreviewState> }
  | { kind: 'leave'; who: string }
  | { kind: 'say'; from: string; to: string; at_ms: number; text: string; ref?: SayRef | null; reply?: boolean; failed?: boolean; why?: string; tries?: number }

export interface LoadingState {
  phase: string
  upto: number
  roots: { name: string; n: number; kind: string; i: number }[]
  unitPages: { key: string; title: string; n: number }[]
}

export interface Step {
  at: number | null
  label: string | null
  events: SimEvent[]
  what: string
  start?: boolean
  rev?: number
  point?: string
}

export interface SimState {
  B: Book
  base: Book
  tier: string
  self: string
  rights: string | null
  peers: Record<string, Peer>
  claims: Claim[]
  ended: { id: string; how: string; at_ms: number; at_rev?: number | null; actor: unknown }[]
  log: LogEntry[]
  recent: { f: TimelineFrame; before: Book; after: Book; age: number; i: number }[]
  at: number | null
  follow: boolean
  turnEnds: Record<string, number>
  turnIdle: number | null
  i: number
  group?: Step
  banner: (OutsideSaveEvent & { at?: number; who?: string }) | null
  paused?: { code: string; message: string }
  missing?: boolean
  loading: LoadingState | null
  decline: { code: string; title: string; text: string; detail: string } | null
  conn: string | null
  zoom: { fit: boolean; ppf: number | null; center: number | null; at: number } | null
  local: (LocalStep & { at: number }) | null
  bounce: { keys: string[]; at: number } | null
  refused: (Refusal & { at: number }) | null
  written: number | null
  rule: Record<string, { focus: Focus | null; selection: Selection | null }>
  ruleChecks: { i: number; rev: number; handle: string; diff: string[] }[]
  pv?: PreviewState
  left?: { who: string; name: string; at: number }
}

export interface Scenario {
  id: string
  group: string
  title: string
  steps: Step[]
  base: () => SimState
}

const strip = (o: unknown): unknown => {
  if (Array.isArray(o)) return o.map(strip)
  if (o && typeof o === 'object') return Object.fromEntries(Object.entries(o).filter(([k]) => k[0] !== '_').map(([k, v]) => [k, strip(v)]))
  return o
}
/** Equal once the sample's `_` annotations are dropped. */
export const same = (a: unknown, b: unknown): boolean => JSON.stringify(strip(a)) === JSON.stringify(strip(b))

export const step = (label: string | null, events?: SimEvent[], what?: string, extra?: Partial<Step>): Step => ({ at: null, label, events: events || [], what: what || '', ...(extra || {}) })

export function blankState(B: Book, self: string, w?: { rights?: string | null; turnIdle?: number | null }): SimState {
  return { B, base: B, tier: 'live', self, rights: w?.rights ?? null, peers: {}, claims: [], ended: [], log: [], recent: [], at: null, follow: true, turnEnds: {}, turnIdle: w?.turnIdle ?? null, i: 0, banner: null, loading: null, decline: null, conn: null, zoom: null, local: null, bounce: null, refused: null, written: null, rule: {}, ruleChecks: [] }
}

export function ensurePeer(S: SimState, h: string): Peer {
  if (!S.peers[h]) {
    const a = actorOf(h)
    S.peers[h] = { handle: h, name: a.name, kind: a.kind, colour: a.colour, away: false, focus: null, selection: null, viewport: null, pointer: null, timePosition: null, viewAddress: null }
  }
  return S.peers[h]
}

/** What a change touched, in order (kit touched_in_order). */
export function touchedOf(f: TimelineFrame): { list: string; key: string }[] {
  const out: { list: string; key: string }[] = []
  for (const o of f.ops) {
    const list = o.kind === 'list' ? o.root : LIST_OF[o.root]
    if (list) out.push({ list, key: o.key })
  }
  return out
}

/** The kit's agent rule (elem_presence verb_presence): focus is the last touched clip's track; selection what it touched in that list, live, at most 256. */
function ruleFrame(B1: Book, r: { focus: Focus | null; selection: Selection | null }, f: TimelineFrame): void {
  const t = touchedOf(f)
  if (!t.length) return
  const M = modelOf(B1)
  const last = t[t.length - 1].list
  const live = (l: string, k: string): boolean => (B1.lists[l] || []).some((x) => x[0] === k)
  for (let j = t.length - 1; j >= 0; j--) {
    const x = t[j]
    if (x.list === UNIT && M.items[x.key] && M.byT[M.items[x.key].track]) {
      r.focus = { root: CONT, key: M.items[x.key].track }
      break
    }
  }
  const keys = [...new Set(t.filter((x) => x.list === last && live(last, x.key)).map((x) => x.key))].slice(-256)
  r.selection = keys.length ? { root: last, keys } : null
}

export function modeOf(p: Peer, claims: Claim[]): Peer['mode'] {
  const mine = claims.filter((c) => actorOf(c.actor).id === p.handle)
  if (p.selection && mine.some((c) => p.selection!.keys.some((k) => c.keys.includes(k)))) return 'editing'
  return p.selection && p.selection.keys.length ? 'selecting' : 'viewing'
}

/** The book as far as a snapshot's pages have come: roots after `upto` still empty. */
export function partialBook(B: Book, upto: number, roots: readonly { name: string; kind: string }[]): Book {
  const P = clone(B)
  roots.forEach((r, i) => {
    if (i <= upto) return
    if (r.kind === 'list') {
      P.lists[r.name] = []
      P.hidden[r.name] = []
    } else P.maps[r.name] = {}
  })
  return P
}

/** Context the fold needs from the world: the schema's roots (loading) and the rights that see refusals. */
export interface FoldCtx {
  roots: readonly { name: string; kind: string }[]
  misfits?: string[]
  /** Books the world already folded, by rev (the turn's), reused when a frame lands on the one before. */
  books?: Record<number, Book>
}

const REFUSAL_RIGHTS = ['edit', 'owner']

export function applyEvent(S: SimState, e: SimEvent, i: number, ctx: FoldCtx): void {
  switch (e.kind) {
    case 'pv':
      S.pv = { ...(S.pv || DEFAULT_PV), ...clone(e.v), at: i }
      return
    case 'leave': {
      const p = S.peers[e.who]
      if (p) S.left = { who: e.who, name: p.name, at: i }
      delete S.peers[e.who]
      return
    }
    case 'say':
      S.log.push({ i, kind: 'say', actor: e.from, to: e.to, text: e.text, ref: clone(e.ref || null), reply: !!e.reply, at: e.at_ms, failed: !!e.failed, why: e.why || null, tries: e.tries || 1 })
      return
    case 'frame': {
      const f = e.frame
      const before = S.B
      const built = builtOf(f)
      const known = ctx.books?.[f.rev]
      S.B = built ? built.after : known && !f._constructed && ctx.books?.[f.rev - 1] === before ? known : applyFrame(before, f, ctx.misfits)
      const a = actorOf(f.actor)
      if (a.kind === 'agent' && !e.quiet) ruleFrame(S.B, (S.rule[a.id] ||= { focus: null, selection: null }), f)
      S.recent = [{ f, before, after: S.B, age: 0, i }, ...S.recent.map((r) => ({ ...r, age: r.age + 1 }))].filter((r) => r.age < 2)
      S.log.push({ i, kind: 'frame', frame: f, actor: a.id, rev: f.rev, at: f.at_ms, before, after: S.B })
      // The kit moves an agent's playhead with its verbs (0634ee32): paused, in ticks, no rate, in the verb's track.
      if (f.verb && a.kind === 'agent') {
        const L = verbLanding(f, before, S.B)
        if (L) {
          const p = ensurePeer(S, a.id)
          p.timePosition = { at: L.at, state: 'paused', at_ms: f.at_ms, ...(L.track ? { _track: L.track } : {}) }
          p.pointer = tpPtr(p.timePosition)
        }
      }
      return
    }
    case 'claim_started': {
      const c = clone(e.claim)
      S.claims.push(c)
      S.log.push({ i, kind: 'claim', claim: c, actor: actorOf(c.actor).id, at: c.started_ms })
      return
    }
    case 'claim_ended': {
      const c = S.claims.find((x) => x.id === e.id)
      const actor = (e.actor || c?.actor) as string | { id: string; name: string; kind: string; colour: string | null }
      let was: string | null = null
      if (e.how === 'gone_outside_save' && c && c.root === CONT) {
        // named as it was before the outside save
        const lf = S.log.filter((x): x is Extract<LogEntry, { kind: 'frame' }> => x.kind === 'frame' && x.i === i).pop()
        was = (modelOf(lf ? lf.before : S.B).byT[c.keys[0]] || { title: null }).title
      }
      S.claims = S.claims.filter((x) => x.id !== e.id)
      S.ended.push({ id: e.id, how: e.how, at_ms: e.at_ms, at_rev: e.at_rev, actor: clone(actor) })
      S.log.push({ i, kind: 'claim_end', id: e.id, how: e.how, claim: c, actor: actorOf(actor).id, at: e.at_ms, ...(e.how === 'gone_outside_save' ? { keys: clone(e.keys || []), was } : {}) })
      return
    }
    case 'turn_end':
      S.turnEnds[e.turn] = e.at_ms
      S.log.push({ i, kind: 'turn_end', turn: e.turn, actor: actorOf(e.actor as string).id, at: e.at_ms })
      return
    case 'welcome':
      S.self = e.frame.self
      S.turnIdle = e.frame.turn_idle_ms ?? null
      S.rights = e.frame.rights ?? null
      return
    case 'presence': {
      const p = ensurePeer(S, e.who) as unknown as Record<string, unknown>
      p[e.type] = clone(e.value)
      if (e.type === 'timePosition') {
        const pp = p as unknown as Peer
        if (pp.timePosition && !pp.timePosition.at_ms) pp.timePosition.at_ms = S.at || 0
        pp.pointer = tpPtr(pp.timePosition)
      }
      return
    }
    case 'host':
      if (e.host.type === 'written') S.written = e.host.rev ?? null
      return
    case 'outside_save': {
      // the kit's outside-save frame, once the save settles: the view stays live either way (ruled)
      const o = e.save
      S.banner = { at: o._at_ms, ...clone(o) }
      S.log.push({ i, kind: 'save', save: clone(o), actor: EXT_ID, at: o._at_ms })
      if (o.outcome === 'declined') S.paused = { code: o.code, message: o.message }
      if (o.outcome === 'missing') S.missing = true
      if (o.outcome === 'kept_both' && !o.who) {
        const n = [...new Set(S.log.filter((x) => x.kind === 'frame' && actorOf(x.actor).kind === 'agent' && (!o.rev || (x as { rev: number }).rev < o.rev)).map((x) => actorOf(x.actor).name))]
        ;(S.banner as { who?: string }).who = n.length ? n.join(' and ') : 'The live'
      }
      return
    }
    case 'loading':
      S.loading = e.v
      S.B = e.v ? partialBook(S.base, e.v.upto, ctx.roots) : S.base
      return
    case 'tier':
      S.tier = e.tier
      S.decline = e.decline || null
      if (e.tier !== 'live') {
        S.peers = {}
        ensurePeer(S, S.self)
      }
      return
    case 'zoom':
      S.zoom = { fit: !!e.fit, ppf: e.ppf || null, center: e.center == null ? null : e.center, at: i }
      return
    case 'local':
      S.local = { at: i, ...e.v }
      return
    case 'refused': {
      // the kit sends refusals to viewers with rights only (ruled)
      if (!REFUSAL_RIGHTS.includes(S.rights || '')) return
      const r = clone(e.r)
      S.refused = { at: i, ...r }
      S.bounce = { keys: [r.unit], at: i }
      S.log.push({ i, kind: 'refused', actor: actorOf(r.actor).id, r, at: r.at_ms, book: S.B })
      return
    }
    case 'conn':
      S.conn = e.state
      return
    case 'book':
      if (e.note) S.log.push({ i, kind: 'snap', actor: EXT_ID, text: e.note, at: e.at_ms || S.at || 0 })
      S.B = e.B
      S.base = e.B
      return
    case 'snapshot':
      return
  }
}

export function applyGroup(S: SimState, g: Step, i: number, ctx: FoldCtx): void {
  S.i = i
  S.group = g
  S.bounce = null
  S.refused = S.refused && S.refused.at === i ? S.refused : null
  if (g.at != null) S.at = g.at
  for (const e of g.events) applyEvent(S, e, i, ctx)
  // the kit's agent rule against the stream, after each flush that carries the agent's presence
  for (const [h, r] of Object.entries(S.rule)) {
    if (!g.events.some((e) => e.kind === 'presence' && e.who === h)) continue
    const p = S.peers[h]
    if (!p) continue
    const diff = (['focus', 'selection'] as const).filter((k) => r[k] !== null && !same(p[k], r[k]))
    S.ruleChecks = [...S.ruleChecks, { i, rev: S.B.rev, handle: h, diff }]
  }
}

export function finish(S: SimState): SimState {
  for (const p of Object.values(S.peers)) {
    p.mode = modeOf(p, S.claims)
    p.isSelf = p.handle === S.self
  }
  return S
}

export function simulateRaw(sc: Scenario, upto: number, ctx: FoldCtx): SimState {
  const S = sc.base()
  for (let i = 0; i <= upto; i++) applyGroup(S, sc.steps[i], i, ctx)
  return S
}

export const simulate = (sc: Scenario, upto: number, ctx: FoldCtx): SimState => finish(simulateRaw(sc, upto, ctx))
