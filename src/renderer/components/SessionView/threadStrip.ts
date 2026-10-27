// The words of the Thread working strip (prd-daemon-activity-and-thread-
// working-v1 S7: TW12–TW13, mockup-thread-working-v1). Pure: a turn and a
// clock in, three lines out. The renderer only labels what the daemon
// decided; it never works out a state of its own.
//
// 0.46.5 activity SDK: the words live in ONE place, @k2/activity
// (sdk/activity/src/core.ts), which the Garden's `k2-activity@1`, App
// pages' `<k2-activity-bar>` and the App server SDK build from too. This
// file keeps the desktop's names and types and hands each ThreadTurn to it
// (`coreTurn`), plus the drawer's pixel fitting, which only the desktop
// does. Change a word in core.ts, never here.
//
//   line 1  a pulse, the state word where it adds something, the time
//           since the user's message, Stop
//   line 2  the current step, the main line: the tool line, "Thinking… 8s",
//           "Delivering…", "Stuck on a permission prompt"
//   line 3  the tally: "Read 3 files · ran 2 commands", subagents,
//           background tasks
//
// After the agent replies, a turn whose subagents or background tasks are
// still running stays on the strip in phase `children` (Rosson
// 2026-10-07: a reply must not hide live work). Every clock is the
// daemon's: server times read as `Date.now() + skewMs`. An ended turn
// shows nothing, except a turn you stopped, which reads "Stopped" for a
// moment so the click visibly landed.

import {
  STOPPED_LINGER_MS as CORE_STOPPED_LINGER_MS,
  childItems as coreChildItems,
  childText as coreChildText,
  coerceTurn,
  counterSpoken as coreCounterSpoken,
  counters as coreCounters,
  drawerLine as coreDrawerLine,
  stripClock as coreStripClock,
  stripClockShort as coreStripClockShort,
  stripView as coreStripView,
  tallyItems as coreTallyItems,
  type Counter,
  type CounterKey,
  type DetailItem,
  type DrawerLine,
  type StripStep,
  type StripView,
  type Tone,
  type Turn,
} from '@k2/activity'
import type { ThreadTurn, ThreadTurnChild, ThreadTurnTally } from './overlayThread'

/** How long "Stopped" stays after an interrupt (the mockup's 3 s). */
export const STOPPED_LINGER_MS = CORE_STOPPED_LINGER_MS

export type ThreadStripTone = Tone
export type ThreadStripStep = StripStep
export type ThreadStripView = StripView
export type StripDetailItem = DetailItem
export type StripCounterKey = CounterKey
export type StripCounter = Counter
export type ThreadDrawerLine = DrawerLine

/** The desktop's ThreadTurn as @k2/activity's Turn (its own skew kept). */
export function coreTurn(turn: ThreadTurn): Turn {
  const t = coerceTurn(turn, 0)
  if (!t) throw new Error(`threadStrip: not a Thread turn: ${turn.turnId}`)
  return t
}

/** "8s", "1m 05s", "1h 02m". */
export const stripClock: (ms: number) => string = coreStripClock

/** "Read 3 files", "searched 1 time", "ran 2 commands", "edited 1 file". */
export function tallyItems(t: ThreadTurnTally): StripDetailItem[] {
  return coreTallyItems(t)
}

/** The tally in words only (`tallyItems` without the keys). */
export function tallyBits(t: ThreadTurnTally): string[] {
  return tallyItems(t).map((i) => i.text)
}

/** One named child in words: "Explore subagent running", "Running `npm
 *  run dev` in the background". */
export function childText(c: ThreadTurnChild): string {
  return coreChildText(c)
}

/** TW7: what holds the turn. By name when the daemon sends `children`;
 *  from an older server, counts only. */
export function childItems(turn: ThreadTurn): StripDetailItem[] {
  return coreChildItems({ ...turn, children: turn.children ?? null })
}

/** The children in words only (`childItems` without the keys). */
export function childBits(turn: ThreadTurn): string[] {
  return childItems(turn).map((i) => i.text)
}

/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state the strip draws). */
export function threadStripView(turn: ThreadTurn, now: number): ThreadStripView | null {
  return coreStripView(coreTurn(turn), now)
}

// ── The drawer's one line (0.45.6 Garden 1, ported from K2 Companion) ───
// Collapsed it is one line: pulse · step · icon counters · clock. The
// icons are decoration; the drawer's aria-label stays the full sentence,
// and each counter carries a visually hidden word equivalent.

/** Dropped first to last when the line is too narrow (after the
 *  subagents' done group). Subagents and background tasks always show. */
export const COUNTER_DROP_ORDER: StripCounterKey[] = ['search', 'read', 'edit', 'cmd']

/** Non-zero counters, in display order. */
export function stripCounters(turn: Pick<ThreadTurn, 'subagents' | 'subagentsDone' | 'background' | 'tally'>): StripCounter[] {
  return coreCounters(turn)
}

/** The counter in words, for screen readers. */
export function counterSpoken(c: StripCounter): string {
  return coreCounterSpoken(c)
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
/** The working grid's ink height, in em of the counter text: 0.8em − 2px
 *  at the 12px default (7.6px), a little under the digits and centred on
 *  them (Rosson's tune; em so it follows the text). */
export const COUNTER_SPINNER_INK_EM = 0.633
/** The counter row uses the narrow 2×3 grid (Rosson: narrow spots get
 *  2×3): its ink is 9 units wide by 14 tall. */
export const COUNTER_SPINNER_COLS = 2
/** The working grid's layout box width, in em of the counter text: its ink
 *  width (9/14 of its height), rounded up. The width estimate uses it. */
export const COUNTER_SPINNER_EM = Math.ceil(((COUNTER_SPINNER_INK_EM * 9) / 14) * 100) / 100
/** Space (px) between the working grid and the running count after it. */
export const COUNTER_SPINNER_GAP_PX = 2
/** Extra space (px) between the agent icon and the working grid, on top of
 *  the icon's COUNTER_ICON_GAP_PX (Rosson's tune: 1px more there). */
export const COUNTER_SPINNER_LEAD_PX = 1
/** The grid's ink is 14 of its 24-unit height (StatusGlyphs), so the svg is
 *  drawn this many em tall to make the ink COUNTER_SPINNER_INK_EM tall. */
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
  if (c.count > 0) w += COUNTER_SPINNER_LEAD_PX + Math.ceil(COUNTER_SPINNER_EM * fontPx) + COUNTER_SPINNER_GAP_PX + estimateStripTextPx(String(c.count), fontPx)
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
export const stripClockShort: (ms: number) => string = coreStripClockShort

/** The drawer for `turn` at `now` (this client's clock), or null: nothing
 *  to show. */
export function threadDrawerLine(turn: ThreadTurn, now: number): ThreadDrawerLine | null {
  return coreDrawerLine(coreTurn(turn), now)
}
