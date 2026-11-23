// Switch timing trace: how long a switch between agents (or between Home
// and Agents) takes, stage by stage, on this window. Owner report 0.46.17:
// "the time before the page switches ... just isn't snappy". This is the
// tool that says where the time goes. Dev note:
// docs/learnings/LEARNINGS-switch-timing.md.
//
// A switch starts at the click (`markSwitchStart`, from the projects store's
// switch bodies and the page store) and collects the FIRST time of each
// stage after it:
//
//   store          the projects store holds the new workspace
//   restore        restoreWorkspace resolved (tabs for it are in the store)
//   page_paint     the first frame after a Home <-> Agents page change
//   terminal_frame the first frame a terminal of the new view painted
//   terminal_live  that terminal is attached (grid socket up) and painted
//   thread_paint   the first frame the Thread drew its messages
//
// plus the `/cli` calls that started during it (cli-timing.ts's ring).
//
// Recording is a few number writes per switch and always on (tests read
// it). The console line is off unless `localStorage['k2.debug.switchTiming']
// = '1'` (or `k2SwitchTiming.enable(true)`): one line per switch, printed
// when it settles (every stage in, or 3 s after the click).
//
// Tiny module with one import (cli-timing, itself import-free) so
// kessel-term and the stores can both use it.

import { getCliTimings, type CliTiming } from './cli-timing'

export type SwitchStage =
  | 'store'
  | 'restore'
  | 'page_paint'
  | 'terminal_frame'
  | 'terminal_live'
  | 'thread_paint'

export type SwitchKind = 'workspace' | 'project' | 'page'

export interface SwitchTrace {
  id: number
  kind: SwitchKind
  /** What was switched to: a `project:workspace` key or a page name. */
  target: string
  /** performance.now() of the click (the input event's own time stamp when
   *  the switch ran inside one, else the switch body's start). */
  clickAt: number
  /** performance.now() when the switch body ran. */
  handlerAt: number
  /** Date.now() at the switch body, for matching `/cli` calls. */
  wallAt: number
  /** Stage -> ms after `clickAt` (first occurrence only). */
  stages: Partial<Record<SwitchStage, number>>
  /** Extra notes per stage (e.g. `reused=1` on a terminal frame). */
  notes: Partial<Record<SwitchStage, string>>
  settled: boolean
}

export const SWITCH_TIMING_FLAG = 'k2.debug.switchTiming'
/** A switch stops collecting stages this long after its click. */
export const SWITCH_SETTLE_MS = 3000
/** Stages after which a switch counts as settled early. */
const TERMINAL_STAGES: readonly SwitchStage[] = ['terminal_live', 'thread_paint']
const MAX_KEPT = 50

const kept: SwitchTrace[] = []
let current: SwitchTrace | null = null
let nextId = 1
let settleTimer: ReturnType<typeof setTimeout> | null = null
/** A second start in the same task (a page change that selects a workspace)
 *  joins the first switch instead of restarting the clock. */
let startedThisTask = false
let logOn = readFlag()

function readFlag(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(SWITCH_TIMING_FLAG) === '1'
  } catch {
    return false
  }
}

/** The input event this switch runs inside, if any (a click or a key). */
function inputEventStamp(now: number): number | null {
  const ev = (typeof window !== 'undefined' ? (window as unknown as { event?: Event }).event : undefined) ?? null
  const ts = ev?.timeStamp
  if (typeof ts !== 'number' || !Number.isFinite(ts) || ts <= 0) return null
  // timeStamp is on the performance.now() clock in WebKit and Chromium.
  // Anything not within the last second is not this switch's input.
  if (ts > now || now - ts > 1000) return null
  return ts
}

/** "A switch started now" (the click). */
export function markSwitchStart(kind: SwitchKind, target: string): void {
  const now = performance.now()
  if (current && startedThisTask && !current.settled) {
    // Same task as the switch already running: one gesture, one switch.
    if (current.kind === 'page') current.target = `${current.target} -> ${target}`
    return
  }
  if (current && !current.settled) settle(current)
  startedThisTask = true
  queueMicrotask(() => {
    startedThisTask = false
  })
  const trace: SwitchTrace = {
    id: nextId++,
    kind,
    target,
    clickAt: inputEventStamp(now) ?? now,
    handlerAt: now,
    wallAt: Date.now(),
    stages: {},
    notes: {},
    settled: false,
  }
  current = trace
  kept.push(trace)
  if (kept.length > MAX_KEPT) kept.splice(0, kept.length - MAX_KEPT)
  if (settleTimer) clearTimeout(settleTimer)
  settleTimer = setTimeout(() => {
    settleTimer = null
    if (current === trace) settle(trace)
  }, SWITCH_SETTLE_MS)
}

/** Note a stage of the switch running now (first occurrence wins). No-op
 *  when no switch is running. */
export function markSwitchStage(stage: SwitchStage, note?: string): void {
  const trace = current
  if (!trace || trace.settled) return
  if (trace.stages[stage] !== undefined) return
  trace.stages[stage] = performance.now() - trace.clickAt
  if (note) trace.notes[stage] = note
  if (TERMINAL_STAGES.every((s) => trace.stages[s] !== undefined)) settle(trace)
}

/** Stages already waiting for their paint (one rAF per stage per switch). */
const awaitingPaint = new Set<string>()

/** Note a stage once the DOM as it is now has been painted: the next
 *  animation frame, then a task after it (the frame's paint runs between). */
export function markSwitchStageOnPaint(stage: SwitchStage, note?: string): void {
  const trace = current
  if (!trace || trace.settled || trace.stages[stage] !== undefined) return
  const key = `${trace.id}:${stage}`
  if (awaitingPaint.has(key)) return
  awaitingPaint.add(key)
  const afterPaint = (): void => {
    setTimeout(() => {
      awaitingPaint.delete(key)
      if (current === trace) markSwitchStage(stage, note)
    }, 0)
  }
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(afterPaint)
  else afterPaint()
}

/** Is a switch collecting stages right now? */
export function switchInFlight(): boolean {
  return !!current && !current.settled
}

/** First visible new content: the earliest paint stage. */
export function firstContentMs(t: SwitchTrace): number | null {
  const paints = (['terminal_frame', 'thread_paint', 'page_paint'] as const)
    .map((s) => t.stages[s])
    .filter((v): v is number => v !== undefined)
  return paints.length ? Math.min(...paints) : null
}

/** The `/cli` calls that started while the switch ran. */
export function switchCliCalls(t: SwitchTrace): CliTiming[] {
  const end = t.wallAt + (t.settled ? lastStageMs(t) : SWITCH_SETTLE_MS) + 1
  return getCliTimings().filter((c) => c.at >= t.wallAt - 1 && c.at <= end)
}

function lastStageMs(t: SwitchTrace): number {
  const v = Object.values(t.stages) as number[]
  return v.length ? Math.max(...v) : 0
}

export function formatSwitchLine(t: SwitchTrace): string {
  const ms = (v: number | undefined): string => (v === undefined ? '-' : `${Math.round(v)}`)
  const order: SwitchStage[] = ['store', 'restore', 'page_paint', 'terminal_frame', 'terminal_live', 'thread_paint']
  let line = `[switch-timing] #${t.id} ${t.kind} ${t.target} handler=${ms(t.handlerAt - t.clickAt)}`
  for (const s of order) {
    if (t.stages[s] === undefined) continue
    line += ` ${s}=${ms(t.stages[s])}${t.notes[s] ? `(${t.notes[s]})` : ''}`
  }
  line += ` first_content=${ms(firstContentMs(t) ?? undefined)}`
  const calls = switchCliCalls(t)
  if (calls.length) {
    line +=
      ' cli=[' +
      calls
        .map((c) => `${c.method} ${c.route} ${Math.round(c.totalMs)}ms${c.queuedMs >= 1 ? ` q${Math.round(c.queuedMs)}` : ''}`)
        .join(', ') +
      ']'
  }
  return line
}

function settle(t: SwitchTrace): void {
  if (t.settled) return
  t.settled = true
  if (current === t && settleTimer) {
    clearTimeout(settleTimer)
    settleTimer = null
  }
  if (logOn) {
    // eslint-disable-next-line no-console
    console.info(formatSwitchLine(t))
  }
}

/** The kept switches, oldest first (copies of the list, live objects). */
export function getSwitchTimings(): SwitchTrace[] {
  return kept.slice()
}

export function setSwitchTimingLog(on: boolean): void {
  logOn = on
  try {
    if (on) localStorage.setItem(SWITCH_TIMING_FLAG, '1')
    else localStorage.removeItem(SWITCH_TIMING_FLAG)
  } catch {
    /* storage blocked: on for this session only */
  }
}

/** Tests: forget everything. */
export function resetSwitchTimingForTests(): void {
  kept.length = 0
  current = null
  startedThisTask = false
  awaitingPaint.clear()
  if (settleTimer) clearTimeout(settleTimer)
  settleTimer = null
  logOn = false
}

// ── The retention metric (TerminalPane [v2-perf] show_to_painted) ───────

/** The running (or just finished) switch's click time, or null when none
 *  is newer than `maxAgeMs`. */
export function readSwitchStart(maxAgeMs = 10_000): number | null {
  if (!current) return null
  return performance.now() - current.clickAt <= maxAgeMs ? current.clickAt : null
}

function median(xs: number[]): number | null {
  if (!xs.length) return null
  const s = xs.slice().sort((a, b) => a - b)
  const m = s.length >> 1
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2
}

export interface SwitchTimingConsole {
  /** Print one line per switch (remembered on this computer). */
  enable(on?: boolean): void
  readonly enabled: boolean
  last(): SwitchTrace | null
  list(): SwitchTrace[]
  /** The kept switches as a console table. */
  table(): void
  /** Medians over the kept switches, ms after the click. */
  summary(): Record<string, number | null>
  clear(): void
}

const consoleHelper: SwitchTimingConsole = {
  enable: (on = true) => setSwitchTimingLog(on),
  get enabled() {
    return logOn
  },
  last: () => kept[kept.length - 1] ?? null,
  list: getSwitchTimings,
  table: () => {
    // eslint-disable-next-line no-console
    console.table(
      kept.map((t) => ({
        id: t.id,
        kind: t.kind,
        target: t.target,
        ...Object.fromEntries(Object.entries(t.stages).map(([k, v]) => [k, Math.round(v as number)])),
        first_content: firstContentMs(t) === null ? null : Math.round(firstContentMs(t) as number),
        cli: switchCliCalls(t).length,
      })),
    )
  },
  summary: () => {
    const out: Record<string, number | null> = {}
    const stages: SwitchStage[] = ['store', 'restore', 'page_paint', 'terminal_frame', 'terminal_live', 'thread_paint']
    for (const s of stages) {
      out[s] = median(kept.map((t) => t.stages[s]).filter((v): v is number => v !== undefined))
    }
    out.first_content = median(kept.map(firstContentMs).filter((v): v is number => v !== null))
    // eslint-disable-next-line no-console
    console.table(out)
    return out
  },
  clear: () => {
    kept.length = 0
  },
}

if (typeof window !== 'undefined') {
  ;(window as unknown as { k2SwitchTiming?: SwitchTimingConsole }).k2SwitchTiming = consoleHelper
}
