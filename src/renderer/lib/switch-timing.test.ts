// @vitest-environment jsdom
// The switch timing trace (k2SwitchTiming): one trace per switch, the first
// time of each stage, a page change and the workspace it selects as one
// switch, the /cli calls made during it, and the console line only when
// the debug setting is on.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  SWITCH_SETTLE_MS,
  SWITCH_TIMING_FLAG,
  firstContentMs,
  formatSwitchLine,
  getSwitchTimings,
  markSwitchStage,
  markSwitchStageOnPaint,
  markSwitchStart,
  readSwitchStart,
  resetSwitchTimingForTests,
  setSwitchTimingLog,
  switchInFlight,
} from './switch-timing'
import { recordCliTiming, resetCliTimingsForTests } from './cli-timing'

/** The next task: a switch started in this one stops joining. */
const nextTask = () => new Promise((r) => setTimeout(r, 0))

beforeEach(() => {
  resetSwitchTimingForTests()
  resetCliTimingsForTests()
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  try {
    localStorage.removeItem(SWITCH_TIMING_FLAG)
  } catch {
    /* none */
  }
})

describe('switch timing', () => {
  it('records each stage once, after the click, and settles when the terminal is live and the Thread drawn', async () => {
    markSwitchStart('workspace', 'p1:w1')
    expect(switchInFlight()).toBe(true)
    markSwitchStage('store')
    markSwitchStage('terminal_frame', 'webgl-reused ready')
    markSwitchStage('terminal_frame', 'later')
    markSwitchStage('terminal_live')
    expect(switchInFlight()).toBe(true)
    markSwitchStage('thread_paint')
    expect(switchInFlight()).toBe(false)
    const [t] = getSwitchTimings()
    expect(t.kind).toBe('workspace')
    expect(Object.keys(t.stages).sort()).toEqual(['store', 'terminal_frame', 'terminal_live', 'thread_paint'])
    expect(t.notes.terminal_frame).toBe('webgl-reused ready')
    for (const v of Object.values(t.stages)) expect(v).toBeGreaterThanOrEqual(0)
    expect(firstContentMs(t)).toBe(Math.min(t.stages.terminal_frame!, t.stages.thread_paint!))
    // Nothing is recorded after it settled.
    markSwitchStage('restore')
    expect(t.stages.restore).toBeUndefined()
  })

  it('a page change and the workspace it selects in the same task are one switch', async () => {
    markSwitchStart('page', 'agents -> home')
    markSwitchStart('workspace', 'p2:w1')
    expect(getSwitchTimings()).toHaveLength(1)
    expect(getSwitchTimings()[0].target).toBe('agents -> home -> p2:w1')
    await nextTask()
    markSwitchStart('workspace', 'p3:w1')
    const all = getSwitchTimings()
    expect(all).toHaveLength(2)
    // A new switch settles the one before it.
    expect(all[0].settled).toBe(true)
  })

  it('settles on its own SWITCH_SETTLE_MS after the click', () => {
    vi.useFakeTimers()
    markSwitchStart('workspace', 'p1:w1')
    vi.advanceTimersByTime(SWITCH_SETTLE_MS)
    expect(switchInFlight()).toBe(false)
  })

  it('a paint stage lands after the next frame', async () => {
    markSwitchStart('workspace', 'p1:w1')
    markSwitchStageOnPaint('page_paint', 'home')
    expect(getSwitchTimings()[0].stages.page_paint).toBeUndefined()
    await new Promise((r) => setTimeout(r, 60))
    expect(getSwitchTimings()[0].stages.page_paint).toBeGreaterThanOrEqual(0)
  })

  it('the line names the stages and the /cli calls made during the switch', async () => {
    recordCliTiming({ at: Date.now() - 60_000, host: 'local', route: 'old', method: 'GET', lane: 'local', queuedMs: 0, networkMs: 5, totalMs: 5, status: 200 })
    markSwitchStart('workspace', 'p1:w1')
    recordCliTiming({ at: Date.now(), host: 'local', route: 'projects/activate', method: 'POST', lane: 'local', queuedMs: 0, networkMs: 12, totalMs: 12, status: 200 })
    markSwitchStage('store')
    markSwitchStage('terminal_live')
    markSwitchStage('thread_paint')
    const line = formatSwitchLine(getSwitchTimings()[0])
    expect(line).toMatch(/^\[switch-timing\] #\d+ workspace p1:w1 handler=\d+ store=\d+ terminal_live=\d+ thread_paint=\d+ first_content=\d+ cli=\[POST projects\/activate 12ms\]$/)
    expect(line).not.toContain('old')
  })

  it('prints one line per switch only when the setting is on', () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => {})
    markSwitchStart('workspace', 'a')
    markSwitchStage('terminal_live')
    markSwitchStage('thread_paint')
    expect(info).not.toHaveBeenCalled()
    setSwitchTimingLog(true)
    expect(localStorage.getItem(SWITCH_TIMING_FLAG)).toBe('1')
    resetSwitchTimingForTests()
    setSwitchTimingLog(true)
    markSwitchStart('workspace', 'b')
    markSwitchStage('terminal_live')
    markSwitchStage('thread_paint')
    expect(info).toHaveBeenCalledTimes(1)
    expect(String(info.mock.calls[0][0])).toContain('[switch-timing]')
  })

  it('is on window as k2SwitchTiming, and keeps the retention metric its click time', () => {
    const api = (window as unknown as { k2SwitchTiming?: { list(): unknown[]; enable(on?: boolean): void } }).k2SwitchTiming
    expect(api).toBeDefined()
    expect(readSwitchStart()).toBeNull()
    markSwitchStart('workspace', 'p1:w1')
    expect(readSwitchStart()).toBe(getSwitchTimings()[0].clickAt)
    expect(api!.list()).toHaveLength(1)
  })
})
