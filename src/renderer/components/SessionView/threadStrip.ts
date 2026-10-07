// The words of the Thread working strip (prd-daemon-activity-and-thread-
// working-v1 S7: TW12–TW13, mockup-thread-working-v1). Pure: a turn and a
// clock in, three lines out. The renderer only labels what the daemon
// decided; it never works out a state of its own.
//
//   line 1  a pulse, the state word where it adds something, the time
//           since the user's message, Stop
//   line 2  the current step, the main line: the tool line, "Thinking… 8s",
//           "Delivering…", "Stuck on a permission prompt"
//   line 3  the tally: "Read 3 files · ran 2 commands", subagents,
//           background tasks
//
// "Working" is said at most once: while a step line says what the agent is
// doing the header has no state word (the pulse says it), and when the step
// is unknown the header reads "Working" and there is no step line. The
// strip never tells anyone to go to the terminal: a wait is stated as a
// fact, with no call to action.
//
// After the agent replies, a turn whose subagents or background tasks are
// still running stays on the strip in phase `children` (Rosson
// 2026-10-07: a reply must not hide live work): the header is the children
// ("2 subagents running, 1 done · 4m 10s"), the tally the tools so far, and
// there is no step line and no Stop (Esc reaches only the lead).
//
// Every clock is the daemon's: server times read as `Date.now() + skewMs`
// (the frame's `serverNow`). An ended turn shows nothing (Decision 2, Q8),
// except a turn you stopped, which reads "Stopped" for a moment so the
// click visibly landed.

import { minutesWithoutUpdate } from '@/lib/activity-copy'
import type { ThreadTurn, ThreadTurnTally } from './overlayThread'

/** How long "Stopped" stays after an interrupt (the mockup's 3 s). */
export const STOPPED_LINGER_MS = 3_000

export type ThreadStripTone = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped'

export interface ThreadStripStep {
  /** `line`: a tool line with `code` spans; `thinking` shimmers. */
  kind: 'line' | 'thinking' | 'plain' | 'needs'
  text: string
  /** Time in this step ("12s"), or null when the text carries it. */
  elapsed: string | null
}

export interface ThreadStripView {
  tone: ThreadStripTone
  /** The state word, only where it adds something: "Working" (no step
   *  known), "Monitoring", "Needs you", "Stopped", "No update in 34m".
   *  Empty while the step line says what the agent is doing. */
  label: string
  /** The header's clock or note: "1m 12s", "you stopped it after 14s", … */
  since: string
  /** Hover text for `since` ("since your message"), or null. */
  sinceTitle: string | null
  /** The whole strip in words, for assistive tech: "Agent working, 1m 12s
   *  since your message". */
  ariaLabel: string
  step: ThreadStripStep | null
  /** Muted bits, joined with " · ". */
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

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

/** "Read 3 files", "searched 1 time", "ran 2 commands", "edited 1 file". */
export function tallyBits(t: ThreadTurnTally): string[] {
  const out: string[] = []
  if (t.read) out.push(`read ${plural(t.read, 'file', 'files')}`)
  if (t.search) out.push(`searched ${plural(t.search, 'time', 'times')}`)
  if (t.cmd) out.push(`ran ${plural(t.cmd, 'command', 'commands')}`)
  if (t.edit) out.push(`edited ${plural(t.edit, 'file', 'files')}`)
  if (out.length) out[0] = out[0].charAt(0).toUpperCase() + out[0].slice(1)
  return out
}

/** TW7: counts only, never labels. */
function childBits(turn: ThreadTurn): string[] {
  const out: string[] = []
  if (turn.subagents > 0) {
    const done = turn.subagentsDone > 0 ? `, ${turn.subagentsDone} done` : ''
    out.push(`${plural(turn.subagents, 'subagent', 'subagents')} running${done}`)
  } else if (turn.subagentsDone > 0) {
    out.push(`${plural(turn.subagentsDone, 'subagent', 'subagents')} done`)
  }
  if (turn.background > 0) out.push(plural(turn.background, 'background task', 'background tasks'))
  return out
}

/** The step line, or null when the step is unknown (the header then says
 *  "Working" instead). */
function stepOf(turn: ThreadTurn, serverNow: number): ThreadStripStep | null {
  const inPhase = stripClock(serverNow - turn.phaseSince)
  switch (turn.phase) {
    case 'tool':
      return turn.line ? { kind: 'line', text: turn.line, elapsed: inPhase } : null
    case 'thinking':
      return { kind: 'thinking', text: `Thinking… ${inPhase}`, elapsed: null }
    case 'waiting':
      return {
        kind: 'needs',
        text: turn.waitingOn === 'question' ? 'Stuck on a question' : 'Stuck on a permission prompt',
        elapsed: inPhase,
      }
    case 'delivering':
      return { kind: 'plain', text: 'Delivering…', elapsed: null }
    case 'working':
    case 'stale':
    case 'children':
      return null
  }
}

/** A step for assistive tech: the tool line without its backticks. */
function spokenStep(step: ThreadStripStep | null): string {
  return step ? `: ${step.text.replace(/`/g, '')}` : ''
}

/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state the strip draws). */
export function threadStripView(turn: ThreadTurn, now: number): ThreadStripView | null {
  const serverNow = now + turn.skewMs
  if (turn.end) {
    if (turn.end.reason !== 'interrupted' || serverNow - turn.end.at >= STOPPED_LINGER_MS) return null
    const since = `you stopped it after ${stripClock(turn.end.at - turn.startedAt)}`
    return {
      tone: 'stopped',
      label: 'Stopped',
      since,
      sinceTitle: null,
      ariaLabel: `Stopped, ${since}`,
      step: null,
      tally: [],
      stoppable: false,
    }
  }
  const clock = stripClock(serverNow - turn.startedAt)
  const sinceMessage = `${clock} since your message`
  const tally = [...tallyBits(turn.tally), ...childBits(turn)]
  const step = stepOf(turn, serverNow)
  if (turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring')) {
    const children = childBits(turn)
    const label = children.length ? children.join(' · ') : 'Background work'
    return {
      tone: turn.state,
      label,
      since: clock,
      sinceTitle: 'since your message',
      ariaLabel: `Agent replied; ${label.charAt(0).toLowerCase()}${label.slice(1)}, ${sinceMessage}`,
      step: null,
      tally: tallyBits(turn.tally),
      stoppable: false,
    }
  }
  switch (turn.state) {
    case 'working':
    case 'monitoring': {
      // The pulse says "working"; the word only stands in for an unknown
      // step. Monitoring keeps its word: it tells you something.
      const word = turn.state === 'working' ? 'Working' : 'Monitoring'
      return {
        tone: turn.state,
        label: turn.state === 'monitoring' || !step ? word : '',
        since: clock,
        sinceTitle: 'since your message',
        ariaLabel: `Agent ${word.toLowerCase()}, ${sinceMessage}${spokenStep(step)}`,
        step,
        tally,
        stoppable: true,
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
      // A24: phase `stale` starts at the row's `staleSince`.
      const minutes = minutesWithoutUpdate({ evidenceAt: null, staleSince: turn.phaseSince }, now, turn.skewMs)
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
