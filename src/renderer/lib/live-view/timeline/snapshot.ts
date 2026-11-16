// A folded timeline state, as the shared shell reads it: people for the
// avatar stack and Watching, the header's one sentence, Changes (edits,
// refusals, outside saves and holds that ended outside K2), Messages, the
// notice (an outside save, a refusal, the connection). The lanes and the
// monitor (T3, T4) read `timeline` beside it. Pure: the sample source and
// the real one (T7) both hand their state through here.

import type { TimePositionJson } from '@k2/mpc-client'
import type { LivePerson } from '../shared/people'
import type { SampleSnapshot } from '../shared/samples'
import type { LiveChange, LiveMessage, LiveShellSnapshot } from '../shared/source'
import type { OutsideSave } from '../shared/notices'
import { actorOf } from './actors'
import { modelOf, type TimelineModel } from './fold'
import type { MediaFacts } from './picture'
import { headOf, rtF } from './playheads'
import type { SimState } from './sim'
import { framePhrase, heldWhat, holdGoneText, placeOf, plural, refusalPhrase, trackRenames } from './wording'

/** The clock the sample's times count from (its at_ms are offsets). */
export const SAMPLE_CLOCK0 = Date.UTC(2026, 10, 13, 14, 15, 0)

export interface TimelineView {
  state: SimState
  model: TimelineModel
  /** The scenario and step this came from (the sample's). */
  scenario?: string
  step?: number
  /** The sample world: only then does the monitor draw the sample's footage. */
  sample?: boolean
  /** K2 Render's facts per source (the sample's own, until T8). */
  facts?: MediaFacts
}

export interface TimelineSnapshot extends SampleSnapshot {
  timeline: TimelineView
}

export interface SnapshotOpts {
  facts: MediaFacts
  /** Added to the state's times (the sample's are offsets from 0). */
  clock0?: number
  scenario?: string
  /** The sample world (its footage may be drawn). */
  sample?: boolean
}

/** The agent to follow: whoever made the latest change; else whoever acted last; else an agent with a playhead; else any agent. */
export function agentOf(S: SimState): string | null {
  for (let i = S.log.length - 1; i >= 0; i--) {
    const e = S.log[i]
    if (e.kind === 'frame' && actorOf(e.actor).kind === 'agent') return actorOf(e.actor).id
  }
  for (let i = S.log.length - 1; i >= 0; i--) {
    const a = actorOf(S.log[i].actor)
    if (a.kind === 'agent') return a.id
  }
  const peers = Object.values(S.peers)
  const p = peers.find((x) => x.kind === 'agent' && x.pointer) || peers.find((x) => x.kind === 'agent')
  return p ? p.handle : null
}

/** "now" in the state's own time: the latest thing it has seen. */
export function nowOf(S: SimState): number {
  let n = S.at || 0
  for (const e of S.log) if (e.at != null && e.at > n) n = e.at
  return n
}

const tpWire = (t: (TimePositionJson & { _track?: string }) | null): TimePositionJson | null => {
  if (!t) return null
  const { _track, ...rest } = t
  void _track
  return rest
}

function people(S: SimState, M: TimelineModel, o: SnapshotOpts): LivePerson[] {
  const now = nowOf(S)
  const rank = (p: { isSelf?: boolean; kind: string }): number => (p.isSelf ? 0 : p.kind === 'agent' ? 2 : 1)
  return Object.values(S.peers)
    .slice()
    .sort((a, b) => rank(a) - rank(b))
    .map((p): LivePerson => {
      const mine = S.claims.filter((c) => actorOf(c.actor).id === p.handle)
      const hd = headOf(M, p, now)
      const f = hd ? hd.f : null
      const tr = p.focus ? M.byT[p.focus.key] : undefined
      const ended = S.log.filter((e) => e.kind === 'claim_end' && e.i === S.i && e.actor === p.handle).pop()
      let doing: string
      let doingTip: string | null = null
      if (p.away) doing = 'away'
      else if (p.isSelf) doing = f != null ? `at ${placeOf(M, f, null, o.facts)}` : 'here'
      else if (mine.length) {
        const c = mine[mine.length - 1]
        doing = `holding ${heldWhat(M, c.keys)}`
        doingTip = `${doing}: “${c.note || ''}”`
      } else if (ended && ended.kind === 'claim_end' && ended.how === 'gone_outside_save') doing = holdGoneText(M, ended).replace(/^[^’]+’s /, 'its ')
      else if (ended) doing = 'let go of its hold just now'
      else doing = f != null ? `${hd!.playing ? 'playing' : 'at'} ${placeOf(M, f, p.pointer?.track, o.facts)}` : tr ? 'on ' + tr.label : p.kind === 'agent' ? 'in the file, not editing' : 'watching'
      const at = f != null ? placeOf(M, f, p.pointer?.track, o.facts) : null
      return {
        id: p.handle,
        name: p.isSelf ? 'You' : p.name,
        kind: p.kind === 'agent' ? 'agent' : 'person',
        colour: p.isSelf ? 'var(--color-accent-fill)' : p.colour || '#888',
        away: p.away,
        self: p.isSelf,
        doing,
        doingTip,
        // "Interview B on V1": what a message to them points at (the shell keeps the clip).
        place: at ? at.replace(/ · ([AV]\d+)$/, ' on $1') : null,
        viewAddress: p.viewAddress,
        timePosition: tpWire(p.timePosition),
        editing: p.kind === 'agent' && p.mode === 'editing',
      }
    })
}

function activity(S: SimState, M: TimelineModel, agent: string | null): LiveShellSnapshot['activity'] {
  if (S.loading) return null
  if (S.tier !== 'live') return { who: null, text: 'Not live: you see the file’s saved versions' }
  const lastFrame = S.log.filter((e) => e.kind === 'frame').pop()
  const letGo = S.log.filter((e) => e.kind === 'claim_end' && e.i === S.i && (e.how === 'released' || e.how === 'gone_outside_save')).pop()
  if (S.refused && S.refused.at === S.i) {
    const p = refusalPhrase(S.refused, M, S.claims)
    return { who: actorOf(S.refused.actor).id, text: p.text }
  }
  if (letGo && letGo.kind === 'claim_end') {
    if (letGo.how === 'gone_outside_save') return { who: null, text: holdGoneText(M, letGo) }
    const n = letGo.claim ? letGo.claim.keys.length : 0
    return { who: letGo.actor, text: `let go of ${n ? plural(n, 'clip') : 'its hold'}` }
  }
  const who = lastFrame ? actorOf(lastFrame.actor) : agent ? actorOf(agent) : null
  const claim = who ? S.claims.filter((c) => actorOf(c.actor).id === who.id).pop() : undefined
  const fresh = lastFrame && lastFrame.kind === 'frame' && (!claim || claim.started_ms <= lastFrame.at) ? lastFrame : null
  if (fresh && fresh.kind === 'frame') {
    const ph = framePhrase(fresh.frame, fresh.before, fresh.after)
    const text = ph.text.replace(' · ' + ph.place, '').replace(' at ' + ph.place, '').replace(' to ' + ph.place, '')
    return actorOf(fresh.actor).kind === 'file' ? { who: null, text: `Saved outside K2: ${trackRenames(fresh.frame) || text}` } : { who: fresh.actor, text }
  }
  if (claim && who) return { who: who.id, text: `holds ${plural(claim.keys.length, 'clip')}: “${claim.note || ''}”` }
  const here = agent && S.peers[agent]
  return here ? { who: agent, text: 'hasn’t changed anything yet' } : { who: null, text: 'Just you here' }
}

function changes(S: SimState, M: TimelineModel, clock0: number): LiveChange[] {
  const out: LiveChange[] = []
  for (const e of S.log) {
    const id = `${e.kind}-${e.i}-${out.length}`
    if (e.kind === 'frame') {
      const p = framePhrase(e.frame, e.before, e.after)
      const a = actorOf(e.actor)
      const file = a.kind === 'file'
      const text = file ? `· ${trackRenames(e.frame) || p.text}` : p.text.replace(' · ' + p.place, '').replace(' at ' + p.place, '')
      // An outside save's change frame lists at the save's own time, so the shell knows its notice says it.
      const save = file ? S.log.find((x) => x.kind === 'save' && x.i === e.i) : undefined
      out.push({ id, who: file ? 'file' : a.id, verb: file ? 'saved' : text.split(' ')[0], text, at_ms: clock0 + (save ? save.at : e.at), target: p.target?.item ?? null, ...(file ? { tone: 'outside' as const } : {}) })
    } else if (e.kind === 'refused') {
      const p = refusalPhrase(e.r, modelOf(e.book), S.claims)
      out.push({ id, who: e.actor, verb: 'couldn’t', text: p.text, at_ms: clock0 + e.at, target: p.target?.item ?? null, tone: 'refused' })
    } else if (e.kind === 'save' && e.save.outcome !== 'ingested' && e.save.outcome !== 'kept_both') {
      out.push({ id, who: 'file', verb: 'saved', text: e.save.outcome === 'declined' ? '· the file can’t be live any more' : '· the file was deleted or moved', at_ms: clock0 + e.at, target: null, tone: 'outside' })
    } else if (e.kind === 'snap') {
      out.push({ id, who: 'K2', verb: 'took', text: `took in a new snapshot ${e.text.replace(/^A new snapshot came in /, '')}`, at_ms: clock0 + e.at, target: null })
    } else if (e.kind === 'claim_end' && e.how === 'gone_outside_save') {
      out.push({ id, who: e.actor, verb: 'lost', text: holdGoneText(M, e).replace(/^[^’]+’s hold on (.+?) ended:/, 'lost its hold on $1:'), at_ms: clock0 + e.at, target: null, tone: 'outside' })
    }
  }
  return out.sort((a, b) => a.at_ms - b.at_ms)
}

function messages(S: SimState, M: TimelineModel, clock0: number): LiveMessage[] {
  return S.log.flatMap((e): LiveMessage[] => {
    if (e.kind !== 'say' || e.failed) return []
    const it = e.ref?.item ? M.items[e.ref.item] : undefined
    const f = e.ref ? rtF(e.ref.at, M) : null
    return [{ id: `say-${e.i}`, from: e.actor, to: e.to, text: e.text, at_ms: clock0 + e.at, address: e.ref ? { label: it ? it.name : 'a gap', address: `${it ? it.key : '@timeline'}${f != null ? ' @ ' + M.tcF(f) : ''}` } : null }]
  })
}

function outsideSave(S: SimState, clock0: number): LiveShellSnapshot['outsideSave'] {
  const b = S.banner
  if (!b) return null
  const at_ms = clock0 + (b._at_ms ?? b.at ?? 0)
  switch (b.outcome) {
    case 'ingested':
      return { outcome: 'ingested', rev: b.rev, at_ms }
    case 'kept_both':
      return { outcome: 'kept_both', rev: b.rev, kept: Number(String(b.kept).split('@').pop()), copy: b.copy, at_ms }
    case 'declined':
      return { outcome: 'declined', code: b.code, message: b.message, at_ms }
    case 'missing':
      return { outcome: 'missing', at_ms } as OutsideSave & { at_ms: number }
  }
}

function connection(S: SimState): Pick<LiveShellSnapshot, 'connection' | 'caughtUp' | 'retried'> {
  const lc = S.local && S.local.at === S.i ? S.local : null
  if (S.conn === 'reconnecting') return { connection: lc?.retry === 'trying' ? 'trying' : 'reconnecting' }
  if (S.conn === 'failed') return { connection: 'cant', retried: lc?.retry === 'failed' }
  if (S.conn) return { connection: 'offline' }
  if (lc && lc.caught != null) return { connection: 'back', caughtUp: lc.caught }
  return { connection: null }
}

/** A folded state as the shell's snapshot, with the timeline beside it. */
export function timelineSnapshot(S: SimState, o: SnapshotOpts): TimelineSnapshot {
  const M = modelOf(S.B)
  const clock0 = o.clock0 ?? 0
  const agent = agentOf(S)
  const ppl = people(S, M, o)
  const refused = S.refused && S.refused.at === S.i ? refusalPhrase(S.refused, M, S.claims) : null
  const failedSay = S.log.some((e) => e.kind === 'say' && e.failed && e.i === S.i)
  const agentPeer = agent ? S.peers[agent] : undefined
  return {
    tier: S.tier !== 'live' ? 'saved-only' : S.loading ? 'loading' : 'live',
    ...connection(S),
    self: S.self,
    people: S.loading ? ppl.filter((p) => p.self) : ppl,
    agent: agentPeer ? agent : null,
    agentDone: false,
    activity: activity(S, M, agentPeer ? agent : null),
    changes: changes(S, M, clock0),
    messages: messages(S, M, clock0),
    outsideSave: outsideSave(S, clock0),
    refusal: refused && S.refused ? { actor: refused.who, verb: refused.verb, target: refused.clip, holder: refused.holder, at_ms: clock0 + S.refused.at_ms } : null,
    features: { viewAddress: true, timePosition: true, say: true },
    clock: clock0 + nowOf(S),
    hints: failedSay ? { sayFails: true } : {},
    timeline: { state: S, model: M, scenario: o.scenario, step: S.i, sample: !!o.sample, facts: o.facts },
  }
}

