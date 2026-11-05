// Paused start (prd-zen-user-widgets-v2 UW32, TUW4.4): the boot decision
// from the per-window marker and the shell's watchdog note. Since Rosson
// 2026-11-05 ("relaunch the widgets after the restart is done") a restart
// relaunches them once on its own; only a freeze right after that relaunch
// pauses, and a window that was hidden (throttled timers) never pauses.
import { afterEach, describe, expect, it } from 'vitest'
import {
  bootZenPausedStart,
  clearZenWidgetsRunning,
  markZenWidgetsRunning,
  noteZenWidgetsHeartbeat,
  noteZenWidgetsVisibility,
  readZenWidgetsRunning,
  resetZenPausedStartForTests,
  resumeZenPausedStart,
  subscribeZenPausedStart,
  takeZenPausedAtBoot,
  ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS,
  ZEN_WIDGETS_HEALTHY_BEATS,
  zenPausedStart,
  zenPausedStartDecision,
  zenWidgetsFroze,
  zenWidgetsRunningKey,
  type ZenWidgetsRunning,
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
  const marker = (beats: number, at = 1_000, extra: Partial<ZenWidgetsRunning> = {}): ZenWidgetsRunning => ({
    garden: 'g-test0001',
    at,
    beats,
    ...extra,
  })

  it('no marker: run as usual, whatever the watchdog did', () => {
    expect(zenPausedStartDecision(null, null)).toBeNull()
    expect(zenPausedStartDecision(null, 5_000)).toBeNull()
  })

  it('what counts as a freeze: a watchdog reload after the mount, or fewer than three healthy beats', () => {
    expect(zenWidgetsFroze(marker(ZEN_WIDGETS_HEALTHY_BEATS), 2_000)).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
    // A reload from before these widgets mounted is not theirs.
    expect(zenWidgetsFroze(marker(ZEN_WIDGETS_HEALTHY_BEATS, 3_000), 2_000)).toBeNull()
    expect(zenWidgetsFroze(marker(0), null)).toEqual({ garden: 'g-test0001', reason: 'starting' })
    expect(zenWidgetsFroze(marker(ZEN_WIDGETS_HEALTHY_BEATS - 1), null)?.reason).toBe('starting')
    expect(zenWidgetsFroze(marker(ZEN_WIDGETS_HEALTHY_BEATS), null)).toBeNull()
  })

  it('the first freeze after a restart relaunches them: no pause', () => {
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS), 2_000, 3_000)).toBeNull()
    expect(zenPausedStartDecision(marker(0), null, 3_000)).toBeNull()
  })

  it('a freeze right after K2’s own relaunch pauses them', () => {
    const now = 1_000 + ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS - 1
    expect(zenPausedStartDecision(marker(0, 1_000, { autoAt: 1_000 }), 2_000, now)).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
    expect(zenPausedStartDecision(marker(1, 1_000, { autoAt: 1_000 }), null, now)).toEqual({ garden: 'g-test0001', reason: 'starting' })
  })

  it('a freeze long after K2’s relaunch is a new restart: relaunched again', () => {
    const now = 1_000 + ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS
    expect(zenPausedStartDecision(marker(0, 1_000, { autoAt: 1_000 }), 2_000, now)).toBeNull()
  })

  it('a window that was hidden when it went quiet never pauses (its timers were throttled)', () => {
    expect(zenPausedStartDecision(marker(0, 1_000, { hidden: true, autoAt: 1_000 }), 2_000, 3_000)).toBeNull()
  })

  it('a healthy run that the watchdog did not reload runs, auto relaunch or not', () => {
    expect(zenPausedStartDecision(marker(ZEN_WIDGETS_HEALTHY_BEATS, 1_000, { autoAt: 1_000 }), null, 2_000)).toBeNull()
  })
})

describe('marker lifecycle in one window', () => {
  it('a restart relaunches once; a freeze right after pauses; Run them lifts it; the marker is cleared at boot', () => {
    const storage = memStorage()
    markZenWidgetsRunning('g-test0001', { label: 'window-a', now: 1_000, storage, hidden: false })
    expect(storage.m.has(zenWidgetsRunningKey('window-a'))).toBe(true)
    // Boot 1: the widgets never got healthy (K2 restarted): K2 relaunches them.
    expect(takeZenPausedAtBoot(null, { label: 'window-a', storage, now: 2_000 })).toBeNull()
    expect(readZenWidgetsRunning('window-a', storage)).toBeNull()
    markZenWidgetsRunning('g-test0001', { label: 'window-a', now: 2_100, storage, hidden: false })
    expect(readZenWidgetsRunning('window-a', storage)).toEqual({ garden: 'g-test0001', at: 2_100, beats: 0, autoAt: 2_000 })
    // Boot 2: the watchdog reloaded the window again right after: paused.
    resetZenPausedStartForTests()
    let seen = 0
    subscribeZenPausedStart(() => seen++)
    expect(takeZenPausedAtBoot(5_000, { label: 'window-a', storage, now: 6_000 })).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
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

  it('a watchdog reload later in the session relaunches them (Rosson 2026-11-05), a second one right after pauses', () => {
    const storage = memStorage()
    takeZenPausedAtBoot(null, { label: 'main', storage })
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 1_000, storage, hidden: false })
    for (let i = 0; i < ZEN_WIDGETS_HEALTHY_BEATS; i++) noteZenWidgetsHeartbeat({ label: 'main', storage })
    resetZenPausedStartForTests()
    expect(takeZenPausedAtBoot(60_000, { label: 'main', storage, now: 61_000 })).toBeNull()
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 61_500, storage, hidden: false })
    resetZenPausedStartForTests()
    expect(takeZenPausedAtBoot(80_000, { label: 'main', storage, now: 81_000 })).toEqual({ garden: 'g-test0001', reason: 'watchdog' })
  })

  it('a relaunch of another Garden does not carry the auto mark', () => {
    const storage = memStorage()
    markZenWidgetsRunning('g-a', { label: 'main', now: 1_000, storage, hidden: false })
    expect(takeZenPausedAtBoot(null, { label: 'main', storage, now: 2_000 })).toBeNull()
    markZenWidgetsRunning('g-b', { label: 'main', now: 2_100, storage, hidden: false })
    expect(readZenWidgetsRunning('main', storage)).toEqual({ garden: 'g-b', at: 2_100, beats: 0 })
  })

  it('hidden windows: visibility changes land on the marker, and a hidden window’s reload loop never pauses', () => {
    const storage = memStorage()
    takeZenPausedAtBoot(null, { label: 'main', storage })
    markZenWidgetsRunning('g-test0001', { label: 'main', now: 1_000, storage, hidden: false })
    noteZenWidgetsVisibility({ label: 'main', storage, hidden: true })
    expect(readZenWidgetsRunning('main', storage)?.hidden).toBe(true)
    // Reload after reload while hidden: every boot runs them, none is an auto relaunch.
    for (let i = 0; i < 4; i++) {
      resetZenPausedStartForTests()
      const at = 10_000 * (i + 1)
      expect(takeZenPausedAtBoot(at, { label: 'main', storage, now: at + 1 })).toBeNull()
      markZenWidgetsRunning('g-test0001', { label: 'main', now: at + 2, storage, hidden: true })
      expect(readZenWidgetsRunning('main', storage)).toEqual({ garden: 'g-test0001', at: at + 2, beats: 0, hidden: true })
    }
    // Shown again: the flag comes off.
    noteZenWidgetsVisibility({ label: 'main', storage, hidden: false })
    expect(readZenWidgetsRunning('main', storage)?.hidden).toBeUndefined()
    // No marker: nothing to note.
    clearZenWidgetsRunning({ label: 'main', storage })
    noteZenWidgetsVisibility({ label: 'main', storage, hidden: true })
    expect(readZenWidgetsRunning('main', storage)).toBeNull()
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
