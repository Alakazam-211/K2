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
export function childBits(turn: ThreadTurn): string[] {
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

// ── The drawer's one line (0.45.6 Garden 1, ported from K2 Companion) ───
// Garden 1's activity drawer above the message box (`ZenActivityDrawer`).
// Collapsed it is one line: pulse · step · icon counters · clock. The
// icons are decoration; the drawer's aria-label stays the full sentence
// above, and each counter carries a visually hidden word equivalent.
// Subagents read `[agent] ⠋1  ✓2`: the agent icon, a spinner and the
// running count, then a check and the done count, each group only when it
// is above zero. Expanded it is the detail in words.

export type StripCounterKey = 'subagents' | 'background' | 'read' | 'search' | 'cmd' | 'edit'

/** Display order, left to right. */
const COUNTER_ORDER: StripCounterKey[] = ['subagents', 'background', 'read', 'search', 'cmd', 'edit']

/** Dropped first to last when the line is too narrow (after the
 *  subagents' done group). Subagents and background tasks always show. */
export const COUNTER_DROP_ORDER: StripCounterKey[] = ['search', 'read', 'edit', 'cmd']

export interface StripCounter {
  key: StripCounterKey
  /** Subagents: the running ones. */
  count: number
  /** Subagents only: finished ones (the check group). */
  done: number
}

/** Non-zero counters, in display order. */
export function stripCounters(turn: Pick<ThreadTurn, 'subagents' | 'subagentsDone' | 'background' | 'tally'>): StripCounter[] {
  const counts: Record<StripCounterKey, number> = {
    subagents: turn.subagents,
    background: turn.background,
    read: turn.tally.read,
    search: turn.tally.search,
    cmd: turn.tally.cmd,
    edit: turn.tally.edit,
  }
  const out: StripCounter[] = []
  for (const key of COUNTER_ORDER) {
    const count = Math.max(0, counts[key] || 0)
    const done = key === 'subagents' ? Math.max(0, turn.subagentsDone || 0) : 0
    if (count > 0 || done > 0) out.push({ key, count, done })
  }
  return out
}

/** The counter in words, for screen readers. */
export function counterSpoken(c: StripCounter): string {
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

/** Gap (px) between counters. */
export const COUNTER_GAP_PX = 6
/** A counter's icon (px square) and the space between it and the number. */
export const COUNTER_ICON_PX = 13
export const COUNTER_ICON_GAP_PX = 2
/** The subagents' check icon (px square). */
export const COUNTER_CHECK_PX = 11
/** Space between the subagents' running and done groups. */
export const COUNTER_GROUP_GAP_PX = 7
/** The working grid's layout box (the 3×3 cell spinner), in em of the
 *  counter text: its width in the counter row. */
export const COUNTER_SPINNER_EM = 0.8
/** The working grid's ink height, in em of the counter text: the digits'
 *  height (≈ cap height), as tall as the braille it replaced. */
export const COUNTER_SPINNER_INK_EM = 0.72
/** The grid's ink is 14 of its 24-unit box (StatusGlyphs), so the svg is
 *  drawn this many em square to make the ink COUNTER_SPINNER_INK_EM tall. */
export const COUNTER_SPINNER_SVG_EM = Number(((COUNTER_SPINNER_INK_EM * 24) / 14).toFixed(3))

/** A conservative width estimate (px) of drawer text at `fontPx`, digits
 *  tabular. No canvas, so it runs anywhere (and in tests). */
export function estimateStripTextPx(text: string, fontPx = 12): number {
  let w = 0
  for (const ch of text) w += (/[0-9:]/.test(ch) ? 0.62 : 0.6) * fontPx
  return Math.ceil(w)
}

/** A counter's width (px): its icon, a gap, the number. Subagents: the
 *  agent icon, then the spinner and running count, then a gap, the check
 *  and the done count (each group only when above zero). */
export function counterWidthPx(c: StripCounter, fontPx = 12): number {
  let w = COUNTER_ICON_PX + COUNTER_ICON_GAP_PX
  if (c.key !== 'subagents') return w + estimateStripTextPx(String(c.count), fontPx)
  if (c.count > 0) w += Math.ceil(COUNTER_SPINNER_EM * fontPx) + estimateStripTextPx(String(c.count), fontPx)
  if (c.done > 0) {
    w += (c.count > 0 ? COUNTER_GROUP_GAP_PX : 0) + COUNTER_CHECK_PX + COUNTER_ICON_GAP_PX
    w += estimateStripTextPx(String(c.done), fontPx)
  }
  return w
}

function countersWidth(list: StripCounter[], measure: (c: StripCounter) => number): number {
  if (!list.length) return 0
  return list.reduce((sum, c) => sum + measure(c), 0) + COUNTER_GAP_PX * (list.length - 1)
}

/**
 * The counters that fit `budgetPx` (the room left after the step's minimum
 * width, the pulse and the clock). Drops, in order: the subagents' done
 * group (while some still run), then search, read, edit, commands. The
 * subagents counter and background tasks always stay, even over budget.
 */
export function fitStripCounters(
  counters: StripCounter[],
  budgetPx: number,
  measure: (c: StripCounter) => number = (c) => counterWidthPx(c),
): StripCounter[] {
  let list = counters.slice()
  const fits = (): boolean => countersWidth(list, measure) <= budgetPx
  if (!Number.isFinite(budgetPx) || fits()) return list
  list = list.map((c) => (c.key === 'subagents' && c.count > 0 && c.done > 0 ? { ...c, done: 0 } : c))
  if (fits()) return list
  for (const key of COUNTER_DROP_ORDER) {
    list = list.filter((c) => c.key !== key)
    if (fits()) return list
  }
  return list
}

/** The clock at the far right: "0:08", "1:12", "1:02:03". */
export function stripClockShort(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = String(sec % 60).padStart(2, '0')
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`
}

export interface ThreadDrawerLine {
  tone: ThreadStripTone
  /** Full sentence, from `threadStripView`. */
  ariaLabel: string
  /** The middle of the collapsed line: the step, or the state word. */
  step: ThreadStripStep
  counters: StripCounter[]
  /** Far right, "1:12". */
  clock: string
  /** Expanded, in words. */
  detail: {
    /** "Working · 1m 12s since your message". */
    head: string
    /** The full step with its time in step, or null. */
    step: ThreadStripStep | null
    /** "Read 3 files · searched 1 time · ran 2 commands · edited 1 file". */
    tally: string
    /** "2 subagents running, 1 done · 1 background task". */
    children: string
  }
}

/** The drawer for `turn` at `now` (this client's clock), or null: nothing
 *  to show. */
export function threadDrawerLine(turn: ThreadTurn, now: number): ThreadDrawerLine | null {
  const view = threadStripView(turn, now)
  if (!view) return null
  const tally = tallyBits(turn.tally).join(' · ')
  const children = childBits(turn).join(' · ')
  if (turn.end) {
    return {
      tone: view.tone,
      ariaLabel: view.ariaLabel,
      step: { kind: 'plain', text: 'Stopped', elapsed: null },
      counters: [],
      clock: stripClockShort(turn.end.at - turn.startedAt),
      detail: { head: `Stopped after ${stripClock(turn.end.at - turn.startedAt)}`, step: null, tally, children },
    }
  }
  const serverNow = now + turn.skewMs
  const sinceMessage = `${stripClock(serverNow - turn.startedAt)} since your message`
  const replied = turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring')
  let word: string
  let step: ThreadStripStep
  if (replied) {
    word = 'Replied'
    step = { kind: 'plain', text: 'Replied', elapsed: null }
  } else if (view.tone === 'unverifiable') {
    word = view.label
    step = { kind: 'plain', text: view.label, elapsed: null }
  } else {
    word = turn.state === 'needs-you' ? 'Needs you' : turn.state === 'monitoring' ? 'Monitoring' : 'Working'
    step = view.step ?? { kind: 'plain', text: view.label || word, elapsed: null }
  }
  return {
    tone: view.tone,
    ariaLabel: view.ariaLabel,
    step,
    counters: stripCounters(turn),
    clock: stripClockShort(serverNow - turn.startedAt),
    detail: {
      head: `${word} · ${sinceMessage}`,
      step: replied || view.tone === 'unverifiable' ? null : view.step,
      tally,
      children,
    },
  }
}
