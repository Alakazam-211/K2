// The turn: the sample's timeline (welcome, the snapshot, roster flushes,
// changes, previews, a refusal, turn ends and an outside save) cut into the
// steps the player walks, and the one turn grouping Activity reads (an
// agent's changes, its claim and its refusals under one turn; a turn_end
// closes it). A scenario is a base state and steps; `simulate` replays the
// steps up to one.

import { GOLD } from './samples'
import { clone } from './math'
import { DOCS } from './fold'
import { actorOf } from './wording'
import { applyGroup, blankState, claimRecord, finish, peerFrom, SELF, type LogEntry, type SimEvent, type SimState, type SimStep } from './sim'
import type { Actor, Frame } from './types'

export interface Scenario {
  id: string
  group: string
  title: string
  steps: SimStep[]
  base: () => SimState
  /** Outside-save scenarios: their case. */
  c?: unknown
}

const FRAMES = GOLD.turn.frames

function normTimeline(e: (typeof GOLD.turn.timeline)[number]): SimEvent[] {
  if (e.event === 'welcome') return [{ kind: 'welcome', frame: e.frame as unknown as Frame }]
  if (e.event === 'keyed_snapshot') return [{ kind: 'snapshot' }]
  if (e.event === 'ops_change') return [{ kind: 'frame', frame: FRAMES.find((f) => f.rev === e.rev) }]
  if (e.event === 'turn_end') {
    const fr = e.frame as { turn: string; actor: Actor; at_ms: number }
    return [{ kind: 'turn_end', turn: fr.turn, actor: fr.actor, at_ms: fr.at_ms }]
  }
  if (e.event === 'verb_refused') return [{ kind: 'refused', r: e.frame as never }]
  if (e.event === 'outside_save') return [{ kind: 'settled', v: e.frame }]
  if (e.event === 'preview') return [{ kind: 'preview', p: e.frame as never }]
  if (e.event === 'roster_flush')
    return (e.roster_events ?? []).map((r): SimEvent => {
      if (r.kind === 'claim_started_elems') return { kind: 'claim_started', claim: claimRecord(r) }
      if (r.kind === 'claim_ended') return { kind: 'claim_ended', id: r.claim as string, how: r.how as string, at_ms: r.at_ms as number, at_rev: r.at_rev as number, actor: r.actor as Actor }
      if (r.kind === 'ViewAddress') return { kind: 'presence', who: r.who as string, type: 'viewAddress', value: r.viewAddress }
      return { kind: 'presence', who: r.who as string, type: r.kind as string, value: clone(r[r.kind as string]) }
    })
  return [{ kind: 'unknown', v: e }]
}

export function initTurn(): SimState {
  const S = blankState(DOCS[GOLD.state.upto], SELF)
  for (const q of GOLD.pc.start) S.peers[q.handle] = peerFrom(q as unknown as Record<string, unknown>)
  return S
}

function buildTurn(): Scenario {
  const steps: SimStep[] = []
  for (const e of GOLD.turn.timeline) {
    const evs = normTimeline(e)
    const last = steps[steps.length - 1]
    const startish = e.event === 'welcome' || e.event === 'keyed_snapshot'
    if (last && ((startish && last.start) || e.after_rev != null || last.at === e.at_ms)) last.events.push(...evs)
    else
      steps.push({
        at: e.at_ms,
        events: evs,
        start: startish,
        label: startish ? 'start' : null,
        kind: startish ? 'state' : null,
        what: startish
          ? `Ada opens living-room.blend as a tab: the snapshot at rev ${GOLD.state.upto} and the scene's still from K2 Render. Sam has Floor selected; Atlas has joined. Agents act, people watch.`
          : e._note || null,
      })
  }
  let D = DOCS[GOLD.state.upto]
  const revAt = steps.map((s) => {
    for (const e of s.events) if (e.kind === 'frame' && e.frame) D = DOCS[e.frame.rev]
    return D.rev
  })
  for (const P of GOLD.pc.points) {
    const k = P.after.startsWith('claim') ? steps.findIndex((s) => s.events.some((e) => e.kind === 'claim_started' && e.claim?.id === P.after.split(' ')[0])) : revAt.indexOf(P.rev)
    if (k >= 0) steps[k].point = P.id
  }
  for (const s of steps)
    if (s.events.some((e) => e.kind === 'preview' && e.p?.state === 'ready' && e.p.reused_from == null) && !s.events.some((e) => e.kind === 'frame')) {
      s.label = 'picture'
      s.dwell = 450
    }
  return { id: 'turn', group: 'The turn', title: 'The turn: Atlas moves, turns and resizes; a refusal; Brook renames; an outside save', steps, base: initTurn }
}

export const TURN = buildTurn()

export function simulateRaw(sc: Scenario, upto: number): SimState {
  const S = sc.base()
  for (let i = 0; i <= upto; i++) applyGroup(S, sc.steps[i], i)
  return S
}
export const simulate = (sc: Scenario, upto: number): SimState => finish(simulateRaw(sc, upto))

export const stepOfRev = (rev: number): number => TURN.steps.findIndex((s) => s.events.some((e) => e.kind === 'frame' && e.frame?.rev === rev))
/** The turn's "picture" step after a rev. */
export const stepOfLabelAfter = (rev: number): number => stepOfRev(rev) + 1
export const fromTurn = (j: number) => (): SimState => {
  const S = simulateRaw(TURN, j)
  S.recent = []
  return S
}

export interface TurnGroup {
  turn: string | null
  actor: Actor
  items: LogEntry[]
  ended_at: number | null
  ended?: boolean
  pendingOnly?: boolean
}

export function turnGroups(S: SimState): TurnGroup[] {
  const groups: TurnGroup[] = []
  const byTurn: Record<string, TurnGroup> = {}
  let pending: LogEntry[] = []
  for (const e of S.log) {
    if (e.kind === 'frame') {
      const t = e.frame.turn || 'solo:' + e.actor + ':' + e.rev
      let g = byTurn[t]
      if (!g) {
        g = byTurn[t] = { turn: e.frame.turn, actor: actorOf(e.actor), items: [], ended_at: null }
        groups.push(g)
      }
      const mine = pending.filter((p) => p.actor === e.actor)
      pending = pending.filter((p) => p.actor !== e.actor)
      g.items.push(...mine, e)
    } else if (e.kind === 'turn_end') {
      const g = byTurn[e.turn]
      if (g) {
        g.ended_at = e.at
        g.items.push(e)
      }
    } else if (e.kind === 'settled') {
      const g = groups[groups.length - 1]
      const lf = g && g.items.filter((x) => x.kind === 'frame').pop()
      if ((e.v.outcome === 'ingested' || e.v.outcome === 'kept_both') && g && g.actor.id === e.actor && !g.turn && lf) g.items.push(e)
      else groups.push({ turn: null, actor: actorOf(e.actor), items: [e], ended_at: null })
    } else {
      let g: TurnGroup | null = null
      for (let j = groups.length - 1; j >= 0; j--)
        if (groups[j].actor.id === e.actor) {
          g = groups[j]
          break
        }
      if (g && (!g.turn || !g.ended_at) && (g.turn || e.kind === 'claim_end')) g.items.push(e)
      else pending.push(e)
    }
  }
  const pend: Record<string, LogEntry[]> = {}
  for (const p of pending) (pend[p.actor] = pend[p.actor] ?? []).push(p)
  for (const items of Object.values(pend)) groups.push({ turn: null, actor: actorOf(items[0].actor), items, ended_at: null, pendingOnly: true })
  for (const g of groups) {
    const last = g.items.filter((x) => x.at != null).reduce((m, x) => Math.max(m, x.at as number), 0)
    const idle = g.turn && !g.ended_at && S.turnIdle && S.at != null && last && S.at - last >= S.turnIdle
    g.ended = !!g.ended_at || (!g.turn && !g.pendingOnly) || !!idle
  }
  return groups
}
