// The scene tab's sample source: the mock's 24 scenarios played through the
// shell's player, each step a LiveShellSnapshot (who's here, the header's
// state, changes, messages, notices) plus the scene's own state for the
// panel and, later, the picture. A step is worked out when first shown (the
// large scene folds 2,000 objects), then kept. Sample data, never the file
// the person opened: the banner says so.

import type { LivePerson } from '../shared/people'
import type { PlayerScenario } from '../shared/sample-player'
import type { SampleSnapshot, ShellSample } from '../shared/samples'
import { SampleShellSource, type SampleShellOptions } from '../shared/sample-shell'
import type { LiveChange, LiveMessage, LiveShellSnapshot } from '../shared/source'
import type { OutsideSave } from '../shared/notices'
import { nameOf } from './fold'
import { actorOf, framePhrase, VERB_WORD } from './wording'
import { agentId, type SimState } from './sim'
import { simulate, type Scenario } from './turn'
import { SCENARIOS } from './scenarios-more'
import { agentDone, peopleRows, viewShort } from './panel'

export const SCENE_SAMPLE_FILE = 'living-room'
export const SCENE_SAMPLE_REPLY = 'On it.'

/** A scene step: the shell's snapshot and the scene's own state. */
export interface SceneSnapshot extends SampleSnapshot {
  scene: { S: SimState; scenario: string; step: number; label: string; fileName: string }
}

export const isSceneSnapshot = (s: LiveShellSnapshot): s is SceneSnapshot => 'scene' in s && !!(s as SceneSnapshot).scene

const CONN: Record<string, LiveShellSnapshot['connection']> = { lost: 'reconnecting', gaveup: 'cant' }

/** One step of a scenario as the shell reads it. */
export function sceneSnapshot(S: SimState, sc: Scenario, step: number): SceneSnapshot {
  const live = S.tier === 'live' || S.tier === 'paused'
  const rows = peopleRows(S)
  const people: LivePerson[] = rows.map((r) => ({
    id: r.id,
    name: r.self ? 'You' : r.name,
    kind: r.kind === 'agent' ? 'agent' : 'person',
    colour: r.colour,
    away: r.away,
    self: r.self,
    doing: r.what,
    place: r.view,
    viewAddress: S.peers[r.id]?.view ?? null,
    editing: r.mode === 'editing',
  }))
  const agent = agentId(S)
  const agentHere = agent && S.peers[agent] ? agent : null
  const hold = S.claims.find((c) => actorOf(c.actor).id === agentHere)
  const lastFrame = [...S.log].reverse().find((e) => e.kind === 'frame' && actorOf(e.actor).kind === 'agent')
  const activity = hold ? { who: agentHere, text: `is ${hold.label}` } : lastFrame && lastFrame.kind === 'frame' ? { who: lastFrame.actor, text: framePhrase(lastFrame.frame, lastFrame.before, lastFrame.after).text } : null
  const changes: LiveChange[] = S.log.flatMap((e): LiveChange[] => {
    if (e.kind !== 'frame') return []
    const p = framePhrase(e.frame, e.before, e.after)
    const file = actorOf(e.actor).kind === 'file'
    return [{ id: `rev-${e.rev}`, who: file ? 'file' : e.actor, verb: p.text.split(' ')[0], text: p.text, at_ms: e.at ?? S.at ?? 0, target: p.target, ...(file ? { tone: 'outside' as const } : {}) }]
  })
  const messages: LiveMessage[] = S.log.flatMap((e): LiveMessage[] => {
    if (e.kind === 'said') return [{ id: `said-${e.i}`, from: S.self, to: e.to, text: e.text, at_ms: e.at ?? 0, address: e.ref ? { label: e.ref.startsWith('@obj:') ? nameOf(S.D, e.ref) : viewShort(S.D, e.ref), address: e.ref } : null }]
    if (e.kind === 'reply') return [{ id: `reply-${e.i}`, from: e.actor, to: S.self, text: e.text, at_ms: e.at ?? 0 }]
    return []
  })
  const callout = S.callout && S.callout.i === S.i ? S.callout.r : null
  const back = S.conn === 'ok' && !!S.toast?.text?.startsWith('Back live')
  return {
    tier: S.loading ? 'loading' : S.tier === 'blob' ? 'saved-only' : 'live',
    connection: back ? 'back' : (CONN[S.conn] ?? null),
    caughtUp: back ? 0 : undefined,
    self: S.self,
    people: live ? people : people.filter((p) => p.self),
    agent: agentHere,
    agentDone: agentHere ? agentDone(S, agentHere) : false,
    activity,
    changes,
    messages,
    outsideSave: S.notice ? ({ ...stripI(S.notice), at_ms: S.at ?? 0 } as OutsideSave & { at_ms: number }) : null,
    refusal: callout ? { actor: actorOf(callout.actor).name, verb: VERB_WORD[callout.verb] || 'change', target: nameOf(S.D, callout.unit), holder: callout.holder ? actorOf(callout.holder).name : null, at_ms: callout.at_ms ?? S.at ?? 0 } : null,
    behind: S.pv.state === 'failed' && S.pv.shown != null,
    features: { viewAddress: true, timePosition: false, say: true },
    clock: S.at ?? undefined,
    hints: { sayFails: S.sendFail },
    scene: { S, scenario: sc.id, step, label: sc.steps[step].label ?? '', fileName: SCENE_SAMPLE_FILE },
  }
}

function stripI<T extends { i: number; type?: string }>(n: T): Omit<T, 'i' | 'type'> {
  const { i: _i, type: _t, ...rest } = n
  return rest
}

/** The scenario list for the banner: the mock's 24, each step worked out when first shown. */
export function sceneScenarios(): PlayerScenario<SceneSnapshot>[] {
  return SCENARIOS.map((sc) => ({
    id: sc.id,
    label: scenarioLabel(sc),
    group: sc.group,
    steps: sc.steps.map((st, i) => {
      let memo: SceneSnapshot | null = null
      return {
        label: stepLabel(st),
        get state(): SceneSnapshot {
          return (memo = memo ?? sceneSnapshot(simulate(sc, i), sc, i))
        },
      }
    }),
  }))
}

/** The banner's menu shows each scenario's title, grouped, as the mock's player does. */
export const scenarioLabel = (sc: Scenario): string => sc.title

/** A step's short name: its label, else what happens in it (the mock's player). */
export function stepLabel(st: Scenario['steps'][number]): string {
  if (st.label) return st.label
  for (const e of st.events) {
    if (e.kind === 'frame') return e.frame ? `rev ${e.frame.rev}` : 'a change'
    if (e.kind === 'claim_started') return e.claim?.id ?? 'a hold'
    if (e.kind === 'claim_ended') return 'let go'
    if (e.kind === 'turn_end') return 'turn ends'
    if (e.kind === 'refused') return 'refused'
    if (e.kind === 'settled') return (e.v as { outcome: string }).outcome
    if (e.kind === 'preview') return 'picture'
    if (e.kind === 'presence') return e.type ?? 'presence'
  }
  return '—'
}

export function sceneSample(): ShellSample {
  return { kind: 'scene', fileName: SCENE_SAMPLE_FILE, scenarios: sceneScenarios(), reply: SCENE_SAMPLE_REPLY }
}

/** The scene's sample, on the shell's sample source (so the banner and player are the shell's). */
export class SampleSceneSource extends SampleShellSource {
  constructor(o: SampleShellOptions = {}) {
    super(sceneSample(), { ...o, scenario: o.scenario ?? 'turn' })
  }
  override snapshot(): SceneSnapshot {
    return super.snapshot() as SceneSnapshot
  }
}
