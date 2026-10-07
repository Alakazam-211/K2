// The words of the Thread working strip (prd-daemon-activity-and-thread-
// working-v1 S7: TW12–TW13, mockup-thread-working-v1). Pure: a turn and a
// clock in, three lines out. The renderer only labels what the daemon
// decided; it never works out a state of its own.
//
//   line 1  the state, the time since the user's message, Stop
//   line 2  the current step: the tool line, "Thinking… 8s", "Working…"
//   line 3  the tally: "Read 3 files · ran 2 commands", subagents,
//           background tasks
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
  /** "Working", "Monitoring", "Needs you", "Stopped", "No update in 34m". */
  label: string
  /** "1m 12s since your message", "you stopped it after 14s", … */
  since: string
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
    out.push(`${plural(turn.subagents, 'subagent', 'subagents')} working${done}`)
  } else if (turn.subagentsDone > 0) {
    out.push(`${plural(turn.subagentsDone, 'subagent', 'subagents')} done`)
  }
  if (turn.background > 0) out.push(plural(turn.background, 'background task', 'background tasks'))
  return out
}

function stepOf(turn: ThreadTurn, serverNow: number): ThreadStripStep | null {
  const inPhase = stripClock(serverNow - turn.phaseSince)
  switch (turn.phase) {
    case 'tool':
      return turn.line ? { kind: 'line', text: turn.line, elapsed: inPhase } : { kind: 'plain', text: 'Working…', elapsed: null }
    case 'thinking':
      return { kind: 'thinking', text: `Thinking… ${inPhase}`, elapsed: null }
    case 'waiting':
      return { kind: 'needs', text: 'Waiting for you', elapsed: inPhase }
    case 'delivering':
      return { kind: 'plain', text: 'Delivering…', elapsed: null }
    case 'working':
      return { kind: 'plain', text: 'Working…', elapsed: null }
    case 'stale':
      return null
  }
}

/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state the strip draws). */
export function threadStripView(turn: ThreadTurn, now: number): ThreadStripView | null {
  const serverNow = now + turn.skewMs
  if (turn.end) {
    if (turn.end.reason !== 'interrupted' || serverNow - turn.end.at >= STOPPED_LINGER_MS) return null
    return {
      tone: 'stopped',
      label: 'Stopped',
      since: `you stopped it after ${stripClock(turn.end.at - turn.startedAt)}`,
      step: null,
      tally: [],
      stoppable: false,
    }
  }
  const sinceMessage = `${stripClock(serverNow - turn.startedAt)} since your message`
  const tally = [...tallyBits(turn.tally), ...childBits(turn)]
  switch (turn.state) {
    case 'working':
    case 'monitoring':
      return {
        tone: turn.state,
        label: turn.state === 'working' ? 'Working' : 'Monitoring',
        since: sinceMessage,
        step: stepOf(turn, serverNow),
        tally,
        stoppable: true,
      }
    case 'needs-you':
      return { tone: 'needs-you', label: 'Needs you', since: sinceMessage, step: stepOf(turn, serverNow), tally, stoppable: false }
    case 'unverifiable': {
      // A24: phase `stale` starts at the row's `staleSince`.
      const minutes = minutesWithoutUpdate({ evidenceAt: null, staleSince: turn.phaseSince }, now, turn.skewMs)
      return {
        tone: 'unverifiable',
        label: `No update in ${minutes}m`,
        since: 'the session is still open',
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
