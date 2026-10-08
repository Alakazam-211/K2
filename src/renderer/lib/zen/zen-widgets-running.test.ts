// Paused start (prd-zen-user-widgets-v2 UW32, TUW4.4): the boot decision
// from the per-window marker and the shell's watchdog note.
import { afterEach, describe, expect, it } from 'vitest'
import {
  bootZenPausedStart,
  clearZenWidgetsRunning,
  markZenWidgetsRunning,
  noteZenWidgetsHeartbeat,
  readZenWidgetsRunning,
  resetZenPausedStartForTests,
  resumeZenPausedStart,
  subscribeZenPausedStart,
  takeZenPausedAtBoot,
  ZEN_WIDGETS_HEALTHY_BEATS,
  zenPausedStart,
  zenPausedStartDecision,
  zenWidgetsRunningKey,
} from './zen-widgets-running'

function memStorage() {
  const m = new Map<string, string>()
  return {
    m,
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  }
}

afterEach(() => resetZenPausedStartForTests())

describe('zenPausedStartDecision', () => {
  const marker = (beats: number, at = 1_000) => ({ garden: 'g-test0001', at, beats })

  it('no marker: run as usual, whatever the watchdog did', () => {
    expect(zenPausedStartDecision(null, null)).toBeNull()
    expect(zenPausedStartDecision(null, 5_000)).toBeNull()
  })

  it('a watchdog reload after the widgets mounted pauses them', () => {
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS), 2_000)).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
    // A reload from before these widgets mounted is not theirs.
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS, 3_000), 2_000)).toBeNull()
  })

  it('widgets that never lived three healthy beats pause even without a watchdog note', () => {
    expect(zenPausedStartDecision(marker(0), null)).toEqual({ garden: 'g-test0001', reason: 'starting' })
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS - 1), null)?.reason).toBe('starting')
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS), null)).toBeNull()
  })
})

describe('marker lifecycle in one window', () => {
  it('a leftover marker pauses; Run them lifts it; the marker is cleared at boot', () => {
    const storage = memStorage()
    markZenWidgetsRunning('g-test0001', { label: 'window-a', now: 1_000, storage })
    expect(storage.m.has(zenWidgetsRunningKey('window-a'))).toBe(true)
    let seen = 0
    subscribeZenPausedStart(() => seen++)
    expect(takeZenPausedAtBoot(null, { label: 'window-a', storage })).toEqual({ garden: 'g-test0001', reason: 'starting' })
    expect(zenPausedStart()?.garden).toBe('g-test0001')
    expect(readZenWidgetsRunning('window-a', storage)).toBeNull()
    resumeZenPausedStart()
    expect(zenPausedStart()).toBeNull()
    expect(seen).toBe(2)
  })

  it('three healthy beats after mounting: a normal relaunch runs the widgets', () => {
    const storage = memStorage()
    takeZenPausedAtBoot(null, { label: 'main', storage })
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 1_000, storage })
    for (let i = 0; i < ZEN_WIDGETS_HEALTHY_BEATS + 2; i++) noteZenWidgetsHeartbeat({ label: 'main', storage })
    expect(readZenWidgetsRunning('main', storage)?.beats).toBe(ZEN_WIDGETS_HEALTHY_BEATS)
    resetZenPausedStartForTests()
    expect(takeZenPausedAtBoot(null, { label: 'main', storage })).toBeNull()
  })

  it('a watchdog reload later in the session still pauses them', () => {
    const storage = memStorage()
    takeZenPausedAtBoot(null, { label: 'main', storage })
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 1_000, storage })
    for (let i = 0; i < ZEN_WIDGETS_HEALTHY_BEATS; i++) noteZenWidgetsHeartbeat({ label: 'main', storage })
    resetZenPausedStartForTests()
    expect(takeZenPausedAtBoot(60_000, { label: 'main', storage })).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
  })

  it('heartbeats before the boot decision do not count toward the old marker', () => {
    const storage = memStorage()
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 1_000, storage })
    for (let i = 0; i < 5; i++) noteZenWidgetsHeartbeat({ label: 'main', storage })
    expect(readZenWidgetsRunning('main', storage)?.beats).toBe(0)
  })

  it('unmounting clears the marker; windows keep their own markers', () => {
    const storage = memStorage()
    markZenWidgetsRunning('g-a', { label: 'main', now: 1, storage })
    markZenWidgetsRunning('g-b', { label: 'window-2', now: 1, storage })
    clearZenWidgetsRunning({ label: 'main', storage })
    expect(readZenWidgetsRunning('main', storage)).toBeNull()
    expect(readZenWidgetsRunning('window-2', storage)?.garden).toBe('g-b')
  })

  it('a broken or missing storage never pauses and never throws', async () => {
    const throwing = {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
      removeItem: () => {
        throw new Error('blocked')
      },
    }
    markZenWidgetsRunning('g', { label: 'main', storage: throwing })
    expect(takeZenPausedAtBoot(1, { label: 'main', storage: throwing })).toBeNull()
    expect(takeZenPausedAtBoot(1, { label: 'main', storage: null })).toBeNull()
    const storage = memStorage()
    storage.m.set(zenWidgetsRunningKey('main'), '{not json')
    expect(takeZenPausedAtBoot(1, { label: 'main', storage })).toBeNull()
    // Outside the desktop app the shell call rejects: still a decision.
    await expect(bootZenPausedStart(() => Promise.reject(new Error('no tauri')))).resolves.toBeNull()
  })
})
