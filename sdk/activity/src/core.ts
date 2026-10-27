// @k2/activity: one model of an agent's Thread turn, one set of words for
// it, and one driver that keeps it live and right (0.46.5 activity SDK,
// review-thread-activity-surface-v1 §2). FSL-1.1-Apache-2.0, Alakazam Labs.
//
// Pure TypeScript, no imports, no DOM: the desktop renderer, the Garden's
// `k2-activity@1` library, the App server SDK (`@k2/server`) and App pages
// (`<k2-activity-bar>`) all build from this one file, so every bar K2 draws
// says the same thing for the same turn.
//
// - `coerceTurn(raw, receivedAt)` reads any turn K2 sends: the owner body,
//   the App guest body (thread.activity, thread_activity frames,
//   k2.thread.onActivity), schema 1 (0.46.3–0.46.4) and schema 2 (0.46.5:
//   `stepKind`, `childCounts`, `openBecause`, `evidenceAt`). Unknown words
//   from a newer server stay `unknown` and draw as a quiet "Working" (W9).
// - `reduceTurn(current, incoming)`: one turn moves only forward by `rev`;
//   another `turnId` replaces it (W2).
// - `stripView` / `drawerLine` / `describe`: the words (one copy, moved
//   from the desktop's threadStrip.ts). `monitoring` never animates.
// - `watchActivity(transport, cb, opts)`: socket first, snapshot on every
//   open, frames newer than the snapshot win, re-read on reset and on a
//   60 s self-check, at most one read a second, 429 back-off (W1–W8).
//
// NEVER infer a turn: not from terminal output, not from "I sent and no
// reply came yet", not from the agent's reply (a reply doesn't end a turn).
// No turn = no bar.

// ── Model ───────────────────────────────────────────────────────────────

/** The newest turn schema this file reads. */
export const ACTIVITY_SCHEMA = 2

export type TurnState = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped' | 'idle' | 'unknown'
export type TurnPhase = 'delivering' | 'working' | 'tool' | 'thinking' | 'waiting' | 'stale' | 'children' | 'unknown'
export type StepKind = 'read' | 'search' | 'cmd' | 'edit' | 'web' | 'subagent' | 'tool'
export type OpenBecause = 'lead' | 'children' | 'waiting' | 'stale'
export type ChildKind = 'subagent' | 'shell' | 'monitor' | 'task'
/** `done` / `owed`: a subagent that finished whose result the agent hasn't taken yet. */
export type ChildState = 'running' | 'waiting' | 'done' | 'owed'

export interface TurnTally {
  read: number
  search: number
  cmd: number
  edit: number
}

export interface TurnChild {
  kind: ChildKind
  state: ChildState
  /** Owner frames only: a subagent's type, a shell's redacted line. */
  label: string | null
  /** Server ms. */
  since: number
}

export interface ChildCounts {
  subagent: number
  shell: number
  monitor: number
  task: number
}

export interface TurnEnd {
  /** Owner: reply | done | interrupted | failed | session_gone | delivery_failed
   *  | superseded | stale. Guest: done | stopped | failed. */
  reason: string
  /** Owner only. */
  detail: string | null
  /** Server ms. */
  at: number
}

export interface Turn {
  turnId: string
  rev: number
  /** 1: a 0.46.3–0.46.4 body; 2: it names its step kind and why it is open. */
  schema: 1 | 2
  state: TurnState
  phase: TurnPhase
  /** Why the turn is still open; null once it ended. */
  openBecause: OpenBecause | null
  /** Server ms: the person's (first) message. */
  startedAt: number
  /** Server ms: the newest message that joined the turn. */
  lastComposeAt: number
  /** Server ms: the current phase. */
  phaseSince: number
  /** Server ms: the newest sign of life, when the server says (schema 2). */
  evidenceAt: number | null
  /** `serverNow − Date.now()` when the frame arrived: server times read on
   *  this client's clock as `Date.now() + skewMs` (W6). */
  skewMs: number
  /** Server ms: when the frame was built. */
  serverNow: number
  /** `kind` while the phase is `tool` (schema 2, or read from the owner's
   *  line); `text`: the owner's redacted tool line (never for a guest). */
  step: { kind: StepKind | null; text: string | null }
  tally: TurnTally
  subagents: number
  subagentsDone: number
  background: number
  childCounts: ChildCounts
  /** By name, owner frames only; null when the frame has none. */
  children: TurnChild[] | null
  messages: number
  /** The agent posted in Thread during this turn; null when not said. */
  replied: boolean | null
  /** `owner`: a guest's coarse word ("waiting on the owner"). */
  waitingOn: 'permission' | 'question' | 'owner' | null
  end: TurnEnd | null
  /** Owner frames carry text (a line, labels, an end detail); guest frames never. */
  viewer: 'owner' | 'guest'
}

/** What a watcher hands its callback for one agent or room. */
export interface ActivityState {
  /** The turn, or null: nothing is running (never a guess). */
  turn: Turn | null
  /** A turn is running (not ended). */
  live: boolean
  /** `thread`: the server reports Thread turns; `room`: an older server's
   *  room-level guess, labelled as one; `none`: no turns at all. */
  source: 'thread' | 'room' | 'none'
  fresh: 'live' | 'catching-up' | 'retrying'
}

const STATES: readonly TurnState[] = ['working', 'monitoring', 'needs-you', 'unverifiable', 'stopped', 'idle']
const PHASES: readonly TurnPhase[] = ['delivering', 'working', 'tool', 'thinking', 'waiting', 'stale', 'children']
const STEP_KINDS: readonly StepKind[] = ['read', 'search', 'cmd', 'edit', 'web', 'subagent', 'tool']
const OPEN_BECAUSE: readonly OpenBecause[] = ['lead', 'children', 'waiting', 'stale']
const CHILD_KINDS: readonly ChildKind[] = ['subagent', 'shell', 'monitor', 'task']
const CHILD_STATES: readonly ChildState[] = ['running', 'waiting', 'done', 'owed']

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function finite(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function count(v: unknown): number {
  const n = finite(v)
  return n !== null && n > 0 ? Math.floor(n) : 0
}

function word<T extends string>(list: readonly T[], v: unknown): T | null {
  return list.find((x) => x === v) ?? null
}

/** The step kind an owner's tool line names (a schema-1 owner frame). */
function kindFromLine(line: string | null): StepKind | null {
  if (!line) return null
  if (/^Read(ing)?\b/.test(line)) return 'read'
  if (/^(Search(ing)?|Grep|Glob|Find(ing)?)\b/.test(line)) return 'search'
  if (/^(Run(ning)?|Ran)\b/.test(line)) return 'cmd'
  if (/^(Edit(ing)?|Writ(e|ing))\b/.test(line)) return 'edit'
  if (/^(Fetch(ing)?|Brows(e|ing))\b/.test(line)) return 'web'
  return 'tool'
}

function coerceChildren(raw: unknown, fallbackSince: number): TurnChild[] | null {
  if (!Array.isArray(raw)) return null
  const out: TurnChild[] = []
  for (const c of raw) {
    if (!isObj(c)) continue
    const kind = word(CHILD_KINDS, c.kind)
    const state = word(CHILD_STATES, c.state)
    if (!kind || !state) continue
    const label = typeof c.label === 'string' && c.label.trim() ? c.label.trim() : null
    out.push({ kind, state, label, since: finite(c.since) ?? fallbackSince })
  }
  return out
}

function childCountsOf(a: Obj, children: TurnChild[] | null, subagents: number, background: number): ChildCounts {
  if (isObj(a.childCounts)) {
    const c = a.childCounts
    return { subagent: count(c.subagent), shell: count(c.shell), monitor: count(c.monitor), task: count(c.task) }
  }
  if (children) {
    const live = children.filter((c) => c.state === 'running' || c.state === 'waiting')
    const n = (k: ChildKind): number => live.filter((c) => c.kind === k).length
    return { subagent: n('subagent'), shell: n('shell'), monitor: n('monitor'), task: n('task') }
  }
  // A guest's schema-1 body: background tasks of no named kind.
  return { subagent: subagents, shell: 0, monitor: 0, task: background }
}

function openBecauseOf(raw: unknown, phase: TurnPhase, ended: boolean): OpenBecause | null {
  if (ended) return null
  const said = word(OPEN_BECAUSE, raw)
  if (said) return said
  if (phase === 'waiting') return 'waiting'
  if (phase === 'stale') return 'stale'
  if (phase === 'children') return 'children'
  return 'lead'
}

/**
 * One turn body as K2 sends it (any viewer, schema 1 or 2), or null when
 * it isn't one. `receivedAt` is this client's clock when it arrived. An
 * object that is already a coerced turn (or the renderer's own ThreadTurn,
 * with `skewMs` and `line`) reads the same.
 */
export function coerceTurn(raw: unknown, receivedAt: number): Turn | null {
  if (!isObj(raw)) return null
  const a = raw
  const turnId = typeof a.turnId === 'string' ? a.turnId : ''
  if (!turnId) return null
  // A turn already coerced (it carries `skewMs`) keeps its own skew; a
  // wire body's skew is its `serverNow` against this arrival.
  const givenSkew = finite(a.skewMs)
  const wireNow = finite(a.serverNow)
  const skewMs = givenSkew ?? (wireNow !== null ? wireNow - receivedAt : 0)
  const serverNow = wireNow ?? receivedAt + skewMs
  const startedAt = finite(a.startedAt) ?? finite(a.phaseSince) ?? serverNow
  const state = word(STATES, a.state) ?? 'unknown'
  const phase = word(PHASES, a.phase) ?? 'unknown'
  const tally = isObj(a.tally) ? a.tally : {}
  let end: TurnEnd | null = null
  if (isObj(a.end)) {
    end = {
      reason: typeof a.end.reason === 'string' ? a.end.reason : 'done',
      detail: typeof a.end.detail === 'string' ? a.end.detail : null,
      at: finite(a.end.at) ?? serverNow,
    }
  }
  const stepIn = isObj(a.step) ? a.step : null
  const line =
    typeof a.line === 'string' && a.line.trim()
      ? a.line
      : stepIn && typeof stepIn.text === 'string' && stepIn.text.trim()
        ? stepIn.text
        : null
  const children = coerceChildren(a.children, startedAt)
  const subagents = count(a.subagents)
  const background = count(a.background)
  // The owner body always carries `line` (null between tools); a guest's
  // never does. A coerced turn says which it was.
  const owner = a.viewer === 'owner' || (a.viewer !== 'guest' && ('line' in a || children !== null))
  const stepKind = word(STEP_KINDS, a.stepKind) ?? (stepIn ? word(STEP_KINDS, stepIn.kind) : null)
  const waiting = a.waitingOn
  const schema = a.schema === 1 || a.schema === 2 ? a.schema : 'stepKind' in a || 'openBecause' in a || 'childCounts' in a ? 2 : 1
  return {
    turnId,
    rev: finite(a.rev) ?? 0,
    schema,
    state,
    phase,
    openBecause: openBecauseOf(a.openBecause, phase, end !== null),
    startedAt,
    lastComposeAt: finite(a.lastComposeAt) ?? startedAt,
    phaseSince: finite(a.phaseSince) ?? startedAt,
    evidenceAt: finite(a.evidenceAt),
    skewMs,
    serverNow,
    step: {
      kind: phase === 'tool' ? stepKind ?? kindFromLine(line) : null,
      text: phase === 'tool' ? line : null,
    },
    tally: { read: count(tally.read), search: count(tally.search), cmd: count(tally.cmd), edit: count(tally.edit) },
    subagents,
    subagentsDone: count(a.subagentsDone),
    background,
    childCounts: childCountsOf(a, children, subagents, background),
    children,
    messages: Math.max(1, count(a.messages)),
    replied: typeof a.replied === 'boolean' ? a.replied : null,
    waitingOn: waiting === 'permission' || waiting === 'question' || waiting === 'owner' ? waiting : null,
    end,
    viewer: owner ? 'owner' : 'guest',
  }
}

/** A newer state of the turn wins: another turn replaces it, the same
 *  turn moves only forward by `rev` (W2). */
export function reduceTurn(current: Turn | null, incoming: Turn | null): Turn | null {
  if (!incoming) return current
  if (current && current.turnId === incoming.turnId && incoming.rev < current.rev) return current
  return incoming
}

/** The turn is running (not ended). */
export function isLive(turn: Turn | null | undefined): turn is Turn {
  return !!turn && turn.end === null
}

// ── Words ───────────────────────────────────────────────────────────────
// One copy for every bar K2 draws (desktop drawer and strip, Zen, the
// Garden library, App pages). Moved from SessionView/threadStrip.ts.

/** One word per state, everywhere K2 names one (the Agents widget's dot,
 *  the drawer's head, a bar's aria): the Thread turn's and an agent row's
 *  vocabulary. `unknown` (a newer server's word) reads as Working. */
export const STATE_WORDS = {
  working: 'Working',
  monitoring: 'Monitoring',
  'needs-you': 'Needs you',
  unverifiable: 'No update',
  stopped: 'Stopped',
  idle: 'Idle',
  unknown: 'Working',
} as const satisfies Record<TurnState, string>

/** How long "Stopped" stays after an interrupt. */
export const STOPPED_LINGER_MS = 3_000
/** A turn with no sign of life reads "No update" after this (row.rs). */
export const STALE_AFTER_MS = 30 * 60_000

export type Tone = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped'

export interface StripStep {
  /** `line`: a tool line with `code` spans; `thinking` shimmers. */
  kind: 'line' | 'thinking' | 'plain' | 'needs'
  text: string
  /** Time in this step ("12s"), or null when the text carries it. */
  elapsed: string | null
}

export interface StripView {
  tone: Tone
  /** The state word, only where it adds something. */
  label: string
  since: string
  sinceTitle: string | null
  ariaLabel: string
  step: StripStep | null
  tally: string[]
  /** The owner may press Stop (Esc to the session). */
  stoppable: boolean
}

/** "8s", "1m 05s", "1h 02m". */
export function stripClock(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = sec % 60
  if (h) return `${h}h ${String(m).padStart(2, '0')}m`
  if (m) return `${m}m ${String(s).padStart(2, '0')}s`
  return `${s}s`
}

/** The clock at the far right: "0:08", "1:12", "1:02:03". */
export function stripClockShort(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = String(sec % 60).padStart(2, '0')
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

/** Minutes with no sign of life: from `evidenceAt`, else from the stale
 *  phase's start less the stale wait. */
export function minutesQuiet(evidenceAt: number | null, staleSince: number | null, now: number, skewMs = 0): number {
  const from = evidenceAt ?? (staleSince !== null ? staleSince - STALE_AFTER_MS : null)
  if (from === null) return 0
  return Math.max(0, Math.floor((now + skewMs - from) / 60_000))
}

export type CounterKey = 'subagents' | 'background' | 'read' | 'search' | 'cmd' | 'edit'

/** One expanded-detail bit with the counter it explains. */
export interface DetailItem {
  key: CounterKey
  text: string
}

/** "Read 3 files", "searched 1 time", "ran 2 commands", "edited 1 file". */
export function tallyItems(t: TurnTally): DetailItem[] {
  const out: DetailItem[] = []
  if (t.read) out.push({ key: 'read', text: `read ${plural(t.read, 'file', 'files')}` })
  if (t.search) out.push({ key: 'search', text: `searched ${plural(t.search, 'time', 'times')}` })
  if (t.cmd) out.push({ key: 'cmd', text: `ran ${plural(t.cmd, 'command', 'commands')}` })
  if (t.edit) out.push({ key: 'edit', text: `edited ${plural(t.edit, 'file', 'files')}` })
  const first = out[0]
  if (first) out[0] = { ...first, text: first.text.charAt(0).toUpperCase() + first.text.slice(1) }
  return out
}

/** One named child in words: "Explore subagent running", "Running `npm
 *  run dev` in the background". */
export function childText(c: Pick<TurnChild, 'kind' | 'state' | 'label'>): string {
  if (c.kind === 'subagent') {
    const name = c.label ? `${c.label} subagent` : 'A subagent'
    const state = c.state === 'waiting' ? 'waiting on a prompt' : c.state === 'done' || c.state === 'owed' ? 'done' : 'running'
    return `${name} ${state}`
  }
  const name = c.label ?? (c.kind === 'monitor' ? 'A monitor' : c.kind === 'task' ? 'A task' : 'A background task')
  return `${name} in the background`
}

/** What holds the turn: by name when the frame names its children, else
 *  from the counts (a guest, or an older server). */
export function childItems(turn: Pick<Turn, 'children' | 'subagents' | 'subagentsDone' | 'background'>): DetailItem[] {
  if (turn.children && turn.children.length > 0) {
    const named: DetailItem[] = turn.children.map((c) => ({ key: c.kind === 'subagent' ? 'subagents' : 'background', text: childText(c) }))
    const listedDone = turn.children.filter((c) => c.kind === 'subagent' && (c.state === 'done' || c.state === 'owed')).length
    const done = turn.subagentsDone - listedDone
    if (done > 0) named.push({ key: 'subagents', text: `${plural(done, 'subagent', 'subagents')} done` })
    return named
  }
  const out: DetailItem[] = []
  if (turn.subagents > 0) {
    const done = turn.subagentsDone > 0 ? `, ${turn.subagentsDone} done` : ''
    out.push({ key: 'subagents', text: `${plural(turn.subagents, 'subagent', 'subagents')} running${done}` })
  } else if (turn.subagentsDone > 0) {
    out.push({ key: 'subagents', text: `${plural(turn.subagentsDone, 'subagent', 'subagents')} done` })
  }
  if (turn.background > 0) out.push({ key: 'background', text: plural(turn.background, 'background task', 'background tasks') })
  return out
}

/** A guest's step in words, from the step kind alone (never a line). */
export function stepKindText(kind: StepKind | null): string | null {
  switch (kind) {
    case 'read':
      return 'Reading files'
    case 'search':
      return 'Searching'
    case 'cmd':
      return 'Running a command'
    case 'edit':
      return 'Editing files'
    case 'web':
      return 'Reading the web'
    case 'subagent':
      return 'Starting a subagent'
    case 'tool':
      return 'Using a tool'
    case null:
      return null
  }
}

function stepOf(turn: Turn, serverNow: number): StripStep | null {
  const inPhase = stripClock(serverNow - turn.phaseSince)
  switch (turn.phase) {
    case 'tool': {
      if (turn.step.text) return { kind: 'line', text: turn.step.text, elapsed: inPhase }
      // An owner between lines keeps "Working" (the desktop's words); a
      // guest, who never gets a line, reads the step's kind.
      if (turn.viewer === 'owner') return null
      const said = stepKindText(turn.step.kind)
      return said ? { kind: 'plain', text: said, elapsed: inPhase } : null
    }
    case 'thinking':
      return { kind: 'thinking', text: `Thinking… ${inPhase}`, elapsed: null }
    case 'waiting':
      return {
        kind: 'needs',
        text:
          turn.waitingOn === 'question'
            ? 'Stuck on a question'
            : turn.waitingOn === 'owner'
              ? 'Waiting on the owner'
              : 'Stuck on a permission prompt',
        elapsed: inPhase,
      }
    case 'delivering':
      return { kind: 'plain', text: 'Delivering…', elapsed: null }
    case 'working':
    case 'stale':
    case 'children':
    case 'unknown':
      return null
  }
}

function spokenStep(step: StripStep | null): string {
  return step ? `: ${step.text.replace(/`/g, '')}` : ''
}

/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state a bar draws). */
export function stripView(turn: Turn, now: number): StripView | null {
  const serverNow = now + turn.skewMs
  if (turn.end) {
    if (turn.end.reason !== 'interrupted' || serverNow - turn.end.at >= STOPPED_LINGER_MS) return null
    const since = `you stopped it after ${stripClock(turn.end.at - turn.startedAt)}`
    return { tone: 'stopped', label: 'Stopped', since, sinceTitle: null, ariaLabel: `Stopped, ${since}`, step: null, tally: [], stoppable: false }
  }
  const clock = stripClock(serverNow - turn.startedAt)
  const sinceMessage = `${clock} since your message`
  const children = childItems(turn).map((i) => i.text)
  const tallyText = tallyItems(turn.tally).map((i) => i.text)
  const tally = [...tallyText, ...children]
  const step = stepOf(turn, serverNow)
  if (turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring')) {
    const label = children.length ? children.join(' · ') : 'Background work'
    return {
      tone: turn.state,
      label,
      since: clock,
      sinceTitle: 'since your message',
      ariaLabel: `${turn.replied === false ? 'Agent' : 'Agent replied'}; ${label.charAt(0).toLowerCase()}${label.slice(1)}, ${sinceMessage}`,
      step: null,
      tally: tallyText,
      stoppable: false,
    }
  }
  switch (turn.state) {
    case 'working':
    case 'monitoring':
    case 'unknown': {
      // The pulse says "working"; the word only stands in for an unknown
      // step. Monitoring keeps its word: it tells you something. A word
      // from a newer server reads "Working" (W9).
      const tone = turn.state === 'monitoring' ? 'monitoring' : 'working'
      const w = tone === 'working' ? 'Working' : 'Monitoring'
      return {
        tone,
        label: tone === 'monitoring' || !step || turn.state === 'unknown' ? w : '',
        since: clock,
        sinceTitle: 'since your message',
        ariaLabel: `Agent ${w.toLowerCase()}, ${sinceMessage}${spokenStep(step)}`,
        step,
        tally,
        stoppable: turn.viewer === 'owner' && turn.state !== 'unknown',
      }
    }
    case 'needs-you':
      return {
        tone: 'needs-you',
        label: 'Needs you',
        since: clock,
        sinceTitle: 'since your message',
        ariaLabel: `Agent needs you, ${sinceMessage}${spokenStep(step)}`,
        step,
        tally,
        stoppable: false,
      }
    case 'unverifiable': {
      const minutes = minutesQuiet(turn.evidenceAt, turn.phaseSince, now, turn.skewMs)
      return {
        tone: 'unverifiable',
        label: `No update in ${minutes}m`,
        since: 'the session is still open',
        sinceTitle: null,
        ariaLabel: `No update in ${minutes}m, the session is still open`,
        step: null,
        tally,
        stoppable: false,
      }
    }
    case 'stopped':
    case 'idle':
      return null
  }
}

// ── The one line (the drawer's collapsed line and its detail) ───────────

/** Display order, left to right. */
const COUNTER_ORDER: CounterKey[] = ['subagents', 'background', 'read', 'search', 'cmd', 'edit']

export interface Counter {
  key: CounterKey
  /** Subagents: the running ones. */
  count: number
  /** Subagents only: finished ones (the check group). */
  done: number
}

/** Non-zero counters, in display order. */
export function counters(turn: Pick<Turn, 'subagents' | 'subagentsDone' | 'background' | 'tally'>): Counter[] {
  const counts: Record<CounterKey, number> = {
    subagents: turn.subagents,
    background: turn.background,
    read: turn.tally.read,
    search: turn.tally.search,
    cmd: turn.tally.cmd,
    edit: turn.tally.edit,
  }
  const out: Counter[] = []
  for (const key of COUNTER_ORDER) {
    const n = Math.max(0, counts[key] || 0)
    const done = key === 'subagents' ? Math.max(0, turn.subagentsDone || 0) : 0
    if (n > 0 || done > 0) out.push({ key, count: n, done })
  }
  return out
}

/** The counter in words, for screen readers. */
export function counterSpoken(c: Counter): string {
  switch (c.key) {
    case 'subagents':
      if (c.count > 0) {
        const done = c.done > 0 ? `, ${c.done} done` : ''
        return `${plural(c.count, 'subagent', 'subagents')} running${done}`
      }
      return `${plural(c.done, 'subagent', 'subagents')} done`
    case 'background':
      return plural(c.count, 'background task', 'background tasks')
    case 'read':
      return `read ${plural(c.count, 'file', 'files')}`
    case 'search':
      return `searched ${plural(c.count, 'time', 'times')}`
    case 'cmd':
      return `ran ${plural(c.count, 'command', 'commands')}`
    case 'edit':
      return `edited ${plural(c.count, 'file', 'files')}`
  }
}

export interface DrawerLine {
  tone: Tone
  ariaLabel: string
  /** The middle of the collapsed line: the step, or the state word. */
  step: StripStep
  counters: Counter[]
  /** Far right, "1:12". */
  clock: string
  detail: {
    head: string
    step: StripStep | null
    tally: string
    children: string
    tallyItems: DetailItem[]
    childItems: DetailItem[]
  }
}

/** The one line for `turn` at `now` (this client's clock), or null. */
export function drawerLine(turn: Turn, now: number): DrawerLine | null {
  const view = stripView(turn, now)
  if (!view) return null
  const tallyList = tallyItems(turn.tally)
  const childList = childItems(turn)
  const tally = tallyList.map((i) => i.text).join(' · ')
  const children = childList.map((i) => i.text).join(' · ')
  if (turn.end) {
    return {
      tone: view.tone,
      ariaLabel: view.ariaLabel,
      step: { kind: 'plain', text: 'Stopped', elapsed: null },
      counters: [],
      clock: stripClockShort(turn.end.at - turn.startedAt),
      detail: { head: `Stopped after ${stripClock(turn.end.at - turn.startedAt)}`, step: null, tally, children, tallyItems: tallyList, childItems: childList },
    }
  }
  const serverNow = now + turn.skewMs
  const sinceMessage = `${stripClock(serverNow - turn.startedAt)} since your message`
  const replied = turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring')
  let w: string
  let step: StripStep
  if (replied) {
    // What holds the turn, not only that the agent replied. A frame that
    // doesn't say (`replied` null) only sent this phase after a reply.
    const said = turn.replied !== false
    w = said ? 'Replied' : turn.state === 'monitoring' ? 'Monitoring' : 'Working'
    const first = childList[0]?.text
    const more = childList.length > 1 ? ` +${childList.length - 1}` : ''
    const what = first ? `${first}${more}` : null
    const text = said ? (what ? `Replied · ${what}` : 'Replied') : what ?? w
    step = { kind: text.includes('`') ? 'line' : 'plain', text, elapsed: null }
  } else if (view.tone === 'unverifiable') {
    w = view.label
    step = { kind: 'plain', text: view.label, elapsed: null }
  } else {
    w = turn.state === 'needs-you' ? 'Needs you' : turn.state === 'monitoring' ? 'Monitoring' : 'Working'
    step = view.step ?? { kind: 'plain', text: view.label || w, elapsed: null }
  }
  return {
    tone: view.tone,
    ariaLabel: view.ariaLabel,
    step,
    counters: counters(turn),
    clock: stripClockShort(serverNow - turn.startedAt),
    detail: {
      head: `${w} · ${sinceMessage}`,
      step: replied || view.tone === 'unverifiable' ? null : view.step,
      tally,
      children,
      tallyItems: tallyList,
      childItems: childList,
    },
  }
}

/** Everything a bar draws for one turn, in one object. */
export interface Description {
  tone: Tone | 'done' | 'failed'
  /** The collapsed line's words ("Running `cargo test`", "Thinking… 4s",
   *  "Replied · Explore subagent running", "Waiting on the owner"). */
  headline: string
  /** `line` text carries `code` spans in backticks. */
  headlineKind: StripStep['kind']
  /** Pulse only while the agent itself works: never for monitoring,
   *  needs-you, a quiet turn or an end (never typing dots). */
  animate: boolean
  counters: Counter[]
  clock: string
  aria: string
  detail: string[]
  /** An end frame ("Done", "Stopped", "Failed"), shown for `endLingerMs`. */
  ended: boolean
}

/** The end words a guest and an owner both read. */
export function endWord(reason: string): 'Done' | 'Stopped' | 'Failed' {
  if (reason === 'reply' || reason === 'done') return 'Done'
  if (reason === 'interrupted' || reason === 'superseded' || reason === 'stale' || reason === 'stopped') return 'Stopped'
  return 'Failed'
}

/** The bar for `turn` at `now`, or null: nothing to draw. An end frame is
 *  described (Done / Stopped / Failed) so a bar can linger on it. */
export function describe(turn: Turn | null, now: number): Description | null {
  if (!turn) return null
  if (turn.end) {
    const w = endWord(turn.end.reason)
    const tone = w === 'Done' ? 'done' : w === 'Failed' ? 'failed' : 'stopped'
    const took = stripClock(turn.end.at - turn.startedAt)
    return {
      tone,
      headline: w,
      headlineKind: 'plain',
      animate: false,
      counters: [],
      clock: stripClockShort(turn.end.at - turn.startedAt),
      aria: `${w} after ${took}`,
      detail: [`${w} after ${took}`, ...tallyItems(turn.tally).map((i) => i.text)],
      ended: true,
    }
  }
  const line = drawerLine(turn, now)
  if (!line) return null
  const detail = [line.detail.head]
  if (line.detail.step) detail.push(line.detail.step.text)
  if (line.detail.tally) detail.push(line.detail.tally)
  if (line.detail.children) detail.push(line.detail.children)
  return {
    tone: line.tone,
    headline: line.step.text,
    headlineKind: line.step.kind,
    animate: line.tone === 'working' && turn.state !== 'unknown',
    counters: line.counters,
    clock: line.clock,
    aria: line.ariaLabel,
    detail,
    ended: false,
  }
}

// ── The live-state driver (W1–W9) ───────────────────────────────────────

/** A key's turn as a read answered it: the raw body or null. */
export type Snapshot = Record<string, unknown>

/** What the driver needs from a transport (an App socket + GET, the
 *  Garden bridge, the desktop's overlay socket). */
export interface ActivityTransport {
  /** Open the live socket and keep it open (reconnecting on its own).
   *  Returns close. Every handler may be called any number of times. */
  connect(on: {
    /** The socket is open (each (re)open): the driver reads a snapshot. */
    open(): void
    /** One turn frame for `key` (a room id, an address). */
    frame(key: string, raw: unknown): void
    /** The server dropped frames (`activity_reset`, 0.46.5) or said the
     *  set changed. With the snapshot the reset frame carries (the
     *  overlay socket's `activity`, the room stream's `turns`) the driver
     *  replaces its state with it; without one it reads a snapshot. */
    reset(snapshot?: Snapshot): void
    /** The socket dropped and is reconnecting. */
    drop(): void
  }): () => void
  /** Read every key's current turn (null: none). Reject with an error
   *  carrying `retryAfterMs` on a 429. */
  read(): Promise<Snapshot>
}

export interface WatchOptions {
  /** Keep an end frame this long before the turn reads as none (0). */
  endLingerMs?: number
  /** Re-read while a turn shows and no frame came this long (60 s; never under 60 s). */
  selfCheckMs?: number
  /** At most one read per this many ms (1000). */
  minReadGapMs?: number
  now?: () => number
  setTimeout?: (fn: () => void, ms: number) => unknown
  clearTimeout?: (t: unknown) => void
  /** Called with a function to call when the page becomes visible
   *  again; returns the unsubscribe. Defaults to `document`'s
   *  visibilitychange where there is a document. */
  onVisible?: (fn: () => void) => () => void
  /** A read failed (not a 429). */
  onError?: (err: unknown) => void
}

export const SELF_CHECK_MS = 60_000

/** A 429's wait in ms: `retryAfterMs`, or `retryAfterSecs` (the server
 *  SDK's K2Error), on the error or inside its `body` / `error`. */
function retryAfter(err: unknown): number | null {
  for (const o of [err, isObj(err) ? err.body : null, isObj(err) ? err.error : null]) {
    if (!isObj(o)) continue
    const ms = finite(o.retryAfterMs)
    if (ms !== null) return ms
    const secs = finite(o.retryAfterSecs)
    if (secs !== null) return secs * 1000
  }
  return null
}

function defaultVisible(fn: () => void): () => void {
  const d = (globalThis as { document?: { visibilityState?: string; addEventListener?: Function; removeEventListener?: Function } }).document
  if (!d || typeof d.addEventListener !== 'function' || typeof d.removeEventListener !== 'function') return () => undefined
  const h = (): void => {
    if (d.visibilityState === 'visible') fn()
  }
  d.addEventListener('visibilitychange', h)
  return () => d.removeEventListener?.('visibilitychange', h)
}

/**
 * Keep every key's turn live and right. `cb` gets a fresh Map on every
 * change (key → ActivityState). Returns the unsubscribe: it closes the
 * socket, clears every timer and calls nothing after it returns (W8).
 */
export function watchActivity(
  transport: ActivityTransport,
  cb: (states: Map<string, ActivityState>) => void,
  opts: WatchOptions = {},
): () => void {
  const now = opts.now ?? (() => Date.now())
  const setT = opts.setTimeout ?? ((fn: () => void, ms: number) => setTimeout(fn, ms))
  const clearT = opts.clearTimeout ?? ((t: unknown) => clearTimeout(t as ReturnType<typeof setTimeout>))
  const linger = Math.max(0, opts.endLingerMs ?? 0)
  const selfCheck = Math.max(SELF_CHECK_MS, opts.selfCheckMs ?? SELF_CHECK_MS)
  const minGap = Math.max(0, opts.minReadGapMs ?? 1000)
  let stopped = false
  let fresh: ActivityState['fresh'] = 'catching-up'
  const turns = new Map<string, Turn | null>()
  /** Frames seen per key: a read older than a frame never overwrites it (W1). */
  const gen = new Map<string, number>()
  const lastFrameAt = new Map<string, number>()
  const lingerTimers = new Map<string, unknown>()
  let reading = false
  let again = false
  let lastReadAt = -Infinity
  let readTimer: unknown = null
  let checkTimer: unknown = null

  const emit = (): void => {
    if (stopped) return
    const out = new Map<string, ActivityState>()
    for (const [k, t] of turns) out.set(k, { turn: t, live: isLive(t), source: 'thread', fresh })
    cb(out)
  }

  const setTurn = (key: string, t: Turn | null): void => {
    const old = lingerTimers.get(key)
    if (old !== undefined) {
      clearT(old)
      lingerTimers.delete(key)
    }
    if (t && t.end) {
      if (linger === 0) {
        turns.set(key, null)
        return
      }
      turns.set(key, t)
      lingerTimers.set(
        key,
        setT(() => {
          lingerTimers.delete(key)
          if (stopped || turns.get(key) !== t) return
          turns.set(key, null)
          emit()
        }, linger),
      )
      return
    }
    turns.set(key, t)
  }

  /** REPLACE each key's turn with the snapshot's (W1): an ended turn reads
   *  as none; a key whose frames moved on since `since` keeps its frame. */
  const applySnapshot = (snap: Snapshot, since: Map<string, number> | null): void => {
    const at = now()
    for (const [key, raw] of Object.entries(snap)) {
      if (since && (gen.get(key) ?? 0) !== (since.get(key) ?? 0)) continue // a frame came after the read began: it wins
      const t = raw === null || raw === undefined ? null : coerceTurn(raw, at)
      if (raw !== null && raw !== undefined && !t) continue
      setTurn(key, t && t.end ? null : t)
      lastFrameAt.set(key, at)
    }
    fresh = 'live'
    emit()
  }

  const read = (): void => {
    if (stopped) return
    if (reading) {
      again = true
      return
    }
    const wait = lastReadAt + minGap - now()
    if (wait > 0) {
      if (readTimer === null) {
        readTimer = setT(() => {
          readTimer = null
          read()
        }, wait)
      }
      return
    }
    reading = true
    again = false
    lastReadAt = now()
    const startGen = new Map(gen)
    transport.read().then(
      (snap) => {
        reading = false
        if (stopped) return
        applySnapshot(snap, startGen)
        if (again) read()
      },
      (err: unknown) => {
        reading = false
        if (stopped) return
        const ms = retryAfter(err)
        if (ms === null) opts.onError?.(err)
        // 429: wait as told (at least the gap), then read again.
        const wait = Math.max(minGap, ms ?? 5_000)
        if (readTimer === null) {
          readTimer = setT(() => {
            readTimer = null
            read()
          }, wait)
        }
      },
    )
  }

  const scheduleCheck = (): void => {
    checkTimer = setT(() => {
      checkTimer = null
      if (stopped) return
      const t = now()
      for (const [key, turn] of turns) {
        if (isLive(turn) && t - (lastFrameAt.get(key) ?? 0) >= selfCheck) {
          read()
          break
        }
      }
      scheduleCheck()
    }, selfCheck)
  }

  const close = transport.connect({
    open: () => {
      if (stopped) return
      fresh = 'catching-up'
      emit()
      read()
    },
    frame: (key, raw) => {
      if (stopped) return
      const t = coerceTurn(raw, now())
      if (!t) return
      gen.set(key, (gen.get(key) ?? 0) + 1)
      lastFrameAt.set(key, now())
      const cur = turns.get(key) ?? null
      const next = reduceTurn(cur, t)
      if (next === cur) return
      setTurn(key, next)
      emit()
    },
    reset: (snapshot) => {
      if (stopped) return
      if (snapshot) applySnapshot(snapshot, null)
      else read()
    },
    drop: () => {
      if (stopped) return
      fresh = 'retrying'
      emit()
    },
  })
  scheduleCheck()
  const offVisible = (opts.onVisible ?? defaultVisible)(() => {
    for (const t of turns.values()) {
      if (isLive(t)) {
        read()
        return
      }
    }
  })

  return () => {
    if (stopped) return
    stopped = true
    if (readTimer !== null) clearT(readTimer)
    if (checkTimer !== null) clearT(checkTimer)
    for (const t of lingerTimers.values()) clearT(t)
    lingerTimers.clear()
    offVisible()
    close()
  }
}
