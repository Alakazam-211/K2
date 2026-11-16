// The sample world: the people, the books of the turn (rev 30 to 35, folded
// from the sample's pages and frames through the adapter), and the turn
// itself as a scenario (the frame script, event by event). The other
// scenarios start from here (scenarios-*.ts).

import { addActor, EXT_ID } from './actors'
import { applyFrame, bookFromPages, clone } from './adapter'
import { mkFrame } from './diff'
import { modelOf } from './fold'
import type { MediaFacts } from './picture'
import { tpPtr } from './playheads'
import { GOLD, type PresencePeer, type TurnEvent } from './samples'
import { KEYED_ROOTS, type Book, type Claim, type OutsideSaveEvent, type Pointer, type RationalTime, type Refusal, type TimelineFrame, type Viewport } from './schema'
import { blankState, ensurePeer, simulateRaw, type FoldCtx, type Peer, type Scenario, type SimEvent, type SimState, type Step } from './sim'
import { PER_S } from './time'

export const ADA = 'person:ada'
export const SAM = 'person:sam'
export const ATLAS = 'agent:atlas'
export const BROOK = 'agent:brook'
export const KAI = 'person:kai'
export const EXT = EXT_ID

for (const a of Object.values(GOLD.turn.actors)) addActor(a)
for (const p of GOLD.pc.points) for (const q of p.peers) addActor({ id: q.handle, name: q.name, kind: q.kind, colour: q.colour })
addActor({ id: KAI, name: 'Kai', kind: 'person', colour: '#3fa37a' }) // the presence close-up only

export const FRAMES: TimelineFrame[] = GOLD.turn.frames
const welcomeEvent = GOLD.turn.timeline.find((e) => e.event === 'welcome')
export const WELCOME = (welcomeEvent?.frame || {}) as { self?: string; rights?: string; turn_idle_ms?: number }
export const SELF = WELCOME.self || ADA

/** Misfits while folding (an op that doesn't fit the state): the checks expect none. */
export const MISFITS: string[] = []
export const BOOKS: Record<number, Book> = { [GOLD.state.upto]: bookFromPages(GOLD.state, GOLD.index.containers) }
for (const f of FRAMES) BOOKS[f.rev] = applyFrame(BOOKS[f.rev - 1], f, MISFITS)
export const LAST_REV = FRAMES[FRAMES.length - 1].rev
export const CTX: FoldCtx = { roots: KEYED_ROOTS, misfits: MISFITS, books: BOOKS }

export const B30 = BOOKS[GOLD.state.upto]
export const B35 = BOOKS[LAST_REV]
/** Ticks in a frame at the sample's 24 fps. */
export const FRT = modelOf(B30).FR

/** K2 Render's facts per source, plus the drop-frame street the states scenario adds. */
export const FACTS: MediaFacts = {
  ...GOLD.media.media,
  'Street 2997.mov': GOLD.media.media['Street 2997.mov'] || { kind: 'video', hash: 'b3:5d7e0c11a2f49b30', proxy: { ladder_px: [640, 1280, 1920] }, poster: { px: 640 }, filmstrip: { tile_px: 160, every_s: 2 }, _footage: { scene: 'road', pal: ['#bcd6ee', '#c9a46a', '#3b3b42', '#f2f2f2'], seed: 41 } },
}

/** Presence is rational time at the timeline's rate (ruled). */
export const rt = (f: number): RationalTime => ({ value: f, rate: modelOf(B30).T.rate!.text })
export const ptr = (f: number, track?: string | null): Pointer => ({ at: rt(f), track: track || null })
export const vpt = (f0: number, f1: number, top: string, bottom: string): Viewport => ({ from: rt(f0), to: rt(f1), tracks: [top, bottom] })

/** A presence event; a pointer goes on the wire as the kit's time position (paused, in ticks). */
export function pres(who: string, type: string, value: unknown): SimEvent {
  if (type === 'pointer' && value) {
    const v = value as Pointer
    return { kind: 'presence', who, type: 'timePosition', value: { at: String(BigInt(Math.round(v.at.value)) * (PER_S / 24n)), state: 'paused', at_ms: 0, ...(v.track ? { _track: v.track } : {}) } }
  }
  return { kind: 'presence', who, type, value: clone(value) }
}

const PEER_FIELDS = ['handle', 'name', 'kind', 'colour', 'away', 'focus', 'selection', 'viewport', 'timePosition', 'viewAddress'] as const
function peerFrom(q: PresencePeer): Peer {
  const o = Object.fromEntries(PEER_FIELDS.map((k) => [k, clone((q as unknown as Record<string, unknown>)[k] ?? null)])) as unknown as Peer
  o.pointer = tpPtr(o.timePosition)
  return o
}

export const P1 = GOLD.pc.points[0]

/** A blank state on a book, with the welcome's rights and turn idle (every sample viewer is Ada). */
export const blank = (B: Book): SimState => blankState(B, SELF, { rights: WELCOME.rights, turnIdle: WELCOME.turn_idle_ms })

export function initTurn(): SimState {
  const S = blank(B30)
  S.follow = false // v2.4: nobody follows until they press Follow
  for (const q of P1.peers) {
    if (q.kind === 'agent') ensurePeer(S, q.handle)
    else S.peers[q.handle] = peerFrom(q)
  }
  return S
}

export function claimRecord(r: Record<string, unknown>): Claim {
  return { id: r.claim as string, actor: clone(r.actor) as Claim['actor'], root: r.root as string, keys: (r.keys as string[]).slice(), note: r.note as string, hold: r.hold as boolean, started_ms: r.started_ms as number, expires_ms: r.expires_ms as number }
}

/** On kept both the outside save becomes the live file: the kit sends it as a change from kit:external first. */
export const SAVED_B = bookFromPages({ upto: 0, roots: GOLD.saved.roots }, GOLD.index.containers)
let savedF: TimelineFrame | null = null
export function savedFrame(o: { rev: number; _at_ms: number }): TimelineFrame {
  if (savedF) return savedF
  savedF = mkFrame(BOOKS[o.rev - 1], o.rev, o._at_ms - 1, EXT, null, (B) => {
    B.lists = clone(SAVED_B.lists)
    B.maps = clone(SAVED_B.maps)
    if (SAVED_B.hidden) B.hidden = clone(SAVED_B.hidden)
  })
  return savedF
}

function normTimeline(e: TurnEvent): SimEvent[] {
  const fr = e.frame as Record<string, unknown> | undefined
  switch (e.event) {
    case 'welcome':
      return [{ kind: 'welcome', frame: fr as { self: string } }]
    case 'keyed_snapshot':
      return [{ kind: 'snapshot', seq: e.seq }]
    case 'ops_change':
      return [{ kind: 'frame', frame: FRAMES.find((f) => f.rev === e.rev)! }]
    case 'turn_end':
      return [{ kind: 'turn_end', turn: fr!.turn as string, actor: fr!.actor, at_ms: fr!.at_ms as number }]
    case 'host':
      return [{ kind: 'host', host: e.host!, at_ms: e.at_ms }]
    case 'verb_refused':
      return [{ kind: 'refused', r: fr as unknown as Refusal }]
    case 'outside_save': {
      const o = { _at_ms: e.at_ms, ...fr } as unknown as OutsideSaveEvent & { rev: number }
      return [...(o.outcome === 'kept_both' ? [{ kind: 'frame' as const, frame: savedFrame(o) }] : []), { kind: 'outside_save', save: o }]
    }
    case 'roster_flush':
      return (e.roster_events || []).map((r): SimEvent => {
        if (r.kind === 'claim_started_elems') return { kind: 'claim_started', claim: claimRecord(r) }
        if (r.kind === 'claim_ended') return { kind: 'claim_ended', id: r.claim!, how: r.how!, at_ms: r.at_ms!, at_rev: r.at_rev, actor: r.actor }
        if (r.kind === 'TimePosition') return { kind: 'presence', who: r.who!, type: 'timePosition', value: clone(r.timePosition) }
        if (r.kind === 'ViewAddress') return { kind: 'presence', who: r.who!, type: 'viewAddress', value: clone(r.viewAddress) }
        return { kind: 'presence', who: r.who!, type: r.kind, value: clone(r[r.kind]) }
      })
  }
  return []
}

function buildTurn(): Scenario & { steps: Step[] } {
  const steps: Step[] = []
  for (const e of GOLD.turn.timeline) {
    const evs = normTimeline(e)
    const last = steps[steps.length - 1]
    const startish = e.event === 'welcome' || e.event === 'keyed_snapshot'
    if (last && (last.at === e.at_ms || (startish && last.start))) last.events.push(...evs)
    else
      steps.push({
        at: e.at_ms,
        events: evs,
        start: startish,
        label: startish ? 'start' : null,
        what: startish ? `Ada's welcome and the snapshot at rev ${GOLD.state.upto}: Ada and Sam are here watching; Atlas and Brook have joined.` : '',
      })
  }
  let rev = GOLD.state.upto
  for (const s of steps) {
    for (const e of s.events) if (e.kind === 'frame') rev = e.frame.rev
    s.rev = rev
  }
  for (const P of GOLD.pc.points) {
    const i = steps.findIndex((s) => s.at === P.at_ms)
    if (i >= 0) steps[i].point = P.id
  }
  return { id: 'turn', group: 'The turn', title: 'Watching two agents: Atlas trims and moves, Brook renames and marks, a refused trim, an outside save', steps, base: initTurn }
}

export const TURN = buildTurn()

export const stepOf = (sc: Scenario, pred: (s: Step) => boolean): number => sc.steps.findIndex(pred)

/** A base state: the turn played to step j, its fresh marks cleared. */
export const fromTurn = (j: number) => (): SimState => {
  const S = simulateRaw(TURN, j, CTX)
  S.recent = []
  S.ruleChecks = []
  return S
}

export const jFirst = stepOf(TURN, (s) => s.events.some((e) => e.kind === 'frame' && e.frame.rev === GOLD.state.upto + 1))
export const jP2 = stepOf(TURN, (s) => s.point === 'P2')

