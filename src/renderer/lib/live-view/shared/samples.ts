// The shell's sample worlds, one per live view (the 3D scene and the
// timeline), in the mocks' words: the same people (Atlas and Brook, agents;
// Sam, a person; you), their changes and the shell's states. Phase 1 has
// four scenarios: first open (the default), who's here, the four outside
// saves, and the connection. Later phases add their own (the picture, the
// lanes) beside these. Sample data, never a file: the banner says so.
//
// People's colours are data (the kit's actor colours), not theme tokens.

import type { PlayerScenario } from './sample-player'
import type { LivePerson } from './people'
import type { LiveChange, LiveMessage, LiveShellSnapshot } from './source'

export type ShellKind = 'scene' | 'timeline'

/** A sample step: the shell's snapshot plus what only a sample needs. */
export interface SampleSnapshot extends LiveShellSnapshot {
  hints?: {
    /** People off screen at an edge of the content (the content draws them). */
    offscreen?: { side: 'left' | 'right' | 'top' | 'bottom'; who: string[] }[]
    /** A send on this step fails (the sample's say). */
    sayFails?: boolean
  }
}

export interface ShellSample {
  kind: ShellKind
  fileName: string
  scenarios: PlayerScenario<SampleSnapshot>[]
  /** The sample's agent's reply to a message. */
  reply: string
}

export const SHELL_SCENARIOS = [
  { id: 'open', label: 'First open' },
  { id: 'people', label: 'Who’s here' },
  { id: 'saves', label: 'Outside saves' },
  { id: 'connection', label: 'Connection' },
] as const

const T0 = Date.UTC(2026, 10, 15, 14, 0)
const min = (n: number): number => T0 - n * 60_000

const SELF: LivePerson = { id: 'you', name: 'You', kind: 'person', colour: 'var(--color-accent-fill)', away: false, self: true }
const ATLAS = { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' } as const
const BROOK = { id: 'brook', name: 'Brook', kind: 'agent', colour: '#be185d' } as const
const SAM = { id: 'sam', name: 'Sam', kind: 'person', colour: '#0284c7' } as const

interface World {
  fileName: string
  people: (away: boolean) => LivePerson[]
  changes: LiveChange[]
  activity: { who: string; text: string }[]
  message: LiveMessage
  reply: string
  outsideChange: LiveChange
  declinedMessage: string
}

const SCENE: World = {
  fileName: 'living-room',
  people: (away) => [
    SELF,
    { ...SAM, away, doing: away ? 'away' : 'on Floor', place: 'Scene' },
    { ...ATLAS, away: false, doing: 'arranging the dining set', doingTip: 'Holding the dining set: “arranging the dining set”', place: 'Table, framed', editing: true },
    { ...BROOK, away: false, doing: 'holding Vase', doingTip: 'Holding Vase: “restyling the vase”', place: 'Camera 2' },
  ],
  changes: [
    { id: 's1', who: 'atlas', verb: 'moved', text: 'moved Table, with Vase on it', at_ms: min(6), target: '@obj:7' },
    { id: 's2', who: 'atlas', verb: 'turned', text: 'turned Chair 2 by 90°', at_ms: min(2), target: '@obj:9' },
    { id: 's3', who: 'atlas', verb: 'renamed', text: 'renamed Lamp to Floor lamp', at_ms: T0 - 20_000, target: '@obj:12' },
  ],
  activity: [
    { who: 'atlas', text: 'is arranging the dining set' },
    { who: 'atlas', text: 'turned Chair 2 by 90°' },
    { who: 'atlas', text: 'renamed Lamp to Floor lamp' },
  ],
  message: { id: 'm1', from: 'you', to: 'atlas', text: 'Can the table sit nearer the window?', at_ms: min(1), address: { label: 'Table', address: '@obj:7' } },
  reply: 'On it: I’ll move it once Brook lets go of the vase.',
  outsideChange: { id: 'so', who: 'file', verb: 'saved', text: 'put Rug back and moved Floor lamp', at_ms: T0 - 10_000, target: '@obj:5', tone: 'outside' },
  declinedMessage: 'The save is in a newer format K2 can’t read live.',
}

const TIMELINE: World = {
  fileName: 'rough-cut',
  people: (away) => [
    SELF,
    { ...SAM, away, doing: away ? 'away' : 'at a gap on V1', place: 'a gap on V1' },
    { ...ATLAS, away: false, doing: 'holding 2 clips', doingTip: 'Holding 2 clips: “tightening the interview”', place: 'Interview B on V1', editing: true },
    { ...BROOK, away: false, doing: 'in the file, not editing' },
  ],
  changes: [
    { id: 't1', who: 'atlas', verb: 'trimmed', text: 'trimmed the start of Interview B by 1s', at_ms: min(6), target: '@item:2' },
    { id: 't2', who: 'atlas', verb: 'moved', text: 'moved B-roll 03 later by 2s', at_ms: min(2), target: '@item:7' },
    { id: 't3', who: 'atlas', verb: 'renamed', text: 'renamed B-roll 03 to Harbour at dawn', at_ms: T0 - 20_000, target: '@item:7' },
  ],
  activity: [
    { who: 'atlas', text: 'trimmed the start of Interview B by 1s' },
    { who: 'atlas', text: 'moved B-roll 03 later by 2s' },
    { who: 'atlas', text: 'renamed B-roll 03 to Harbour at dawn' },
  ],
  message: { id: 'm1', from: 'you', to: 'atlas', text: 'Can you keep the cut on the breath?', at_ms: min(1), address: { label: 'Interview B', address: '@item:2' } },
  reply: 'On it: I’ll try it there once Brook lets go of Interview B.',
  outsideChange: { id: 'to', who: 'file', verb: 'saved', text: 'moved Interview B back and put B-roll 03 back', at_ms: T0 - 10_000, target: '@item:2', tone: 'outside' },
  declinedMessage: 'The save isn’t a timeline K2 can read live.',
}

function base(w: World, kind: ShellKind): SampleSnapshot {
  return {
    tier: 'live',
    connection: null,
    self: 'you',
    people: w.people(false),
    agent: 'atlas',
    agentDone: false,
    activity: w.activity[0],
    changes: w.changes.slice(0, 1),
    messages: [],
    outsideSave: null,
    features: { viewAddress: true, timePosition: kind === 'timeline', say: true },
    clock: T0,
  }
}

function scenarios(w: World, kind: ShellKind): PlayerScenario<SampleSnapshot>[] {
  const b = base(w, kind)
  const saved = (o: SampleSnapshot['outsideSave'], extra: Partial<SampleSnapshot> = {}): SampleSnapshot => ({
    ...b,
    people: w.people(false),
    changes: [...w.changes.slice(0, 2), ...(o && (o.outcome === 'ingested' || o.outcome === 'kept_both') ? [w.outsideChange] : [])],
    activity: w.activity[1],
    outsideSave: o,
    ...extra,
  })
  const conn = (connection: SampleSnapshot['connection'], extra: Partial<SampleSnapshot> = {}): SampleSnapshot => ({
    ...b,
    changes: w.changes.slice(0, 2),
    activity: w.activity[1],
    connection,
    hints: { sayFails: true },
    ...extra,
  })
  const copy = { name: `${w.fileName} (conflict Atlas 2026-11-15 140022)`, path: `/home/alex/agents/atlas/${w.fileName} (conflict Atlas 2026-11-15 140022)` }
  return [
    {
      id: 'open',
      label: 'First open',
      steps: [{ label: 'Opening', state: { ...b, tier: 'loading', people: [SELF], agent: null, activity: null, changes: [] } }],
    },
    {
      id: 'people',
      label: 'Who’s here',
      steps: [
        { label: 'Atlas at work', state: { ...b, people: w.people(true), hints: { offscreen: [{ side: 'right', who: ['brook'] }] } } },
        { label: 'Two changes', state: { ...b, people: w.people(true), activity: w.activity[1], changes: w.changes.slice(0, 2), hints: { offscreen: [{ side: 'left', who: ['atlas', 'brook'] }] } } },
        { label: 'A message', state: { ...b, people: w.people(true), activity: w.activity[2], changes: w.changes, messages: [w.message], hints: { offscreen: [{ side: 'right', who: ['brook'] }] } } },
        { label: 'Sending fails', state: { ...b, people: w.people(true), activity: w.activity[2], changes: w.changes, hints: { sayFails: true } } },
        { label: 'Atlas is done', state: { ...b, people: w.people(false).map((p) => (p.id === 'atlas' ? { ...p, doing: 'in the file, not editing', doingTip: null, editing: false } : p)), activity: w.activity[2], changes: w.changes, agentDone: true } },
      ],
    },
    {
      id: 'saves',
      label: 'Outside saves',
      steps: [
        { label: 'Taken in', state: saved({ outcome: 'ingested', rev: 31, at_ms: T0 - 10_000 }) },
        { label: 'Kept both', state: saved({ outcome: 'kept_both', rev: 32, kept: 30, copy, at_ms: T0 - 10_000 }) },
        { label: 'Declined', state: saved({ outcome: 'declined', code: 'unsupported_format', message: w.declinedMessage, at_ms: T0 - 10_000 }) },
        { label: 'Missing', state: saved({ outcome: 'missing', at_ms: T0 - 10_000 }) },
      ],
    },
    {
      id: 'connection',
      label: 'Connection',
      steps: [
        { label: 'Reconnecting', state: conn('reconnecting') },
        { label: 'Trying', state: conn('trying') },
        { label: 'Can’t reconnect', state: conn('cant') },
        { label: 'Offline', state: conn('offline') },
        { label: 'Back', state: conn('back', { changes: w.changes, activity: w.activity[2], caughtUp: 1, hints: {} }) },
      ],
    },
  ]
}

export function shellSample(kind: ShellKind): ShellSample {
  const w = kind === 'scene' ? SCENE : TIMELINE
  return { kind, fileName: w.fileName, scenarios: scenarios(w, kind), reply: w.reply }
}

/** The clock the samples' times are written against ("2 min ago"). */
export const SAMPLE_NOW = T0
