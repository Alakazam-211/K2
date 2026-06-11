// #672 — useActiveStore is the canonical Active mirror. The daemon owns
// the Active set and pushes the WHOLE set (snapshot on connect +
// active_changed deltas); the store applies both as a full-set replace
// (last-write-wins). This suite locks that contract.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// settings.ts (imported transitively for clampActiveWindowHours) calls
// invoke at module load — stub the Tauri boundary so the import is inert.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
// settings.ts kicks off a daemon fetch at module load (fetchSettings);
// stub the daemon-settings boundary so that init resolves quietly instead
// of producing an unhandled rejection during this suite.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))

import { useActiveStore, __resetActiveStoreForTests } from './active'

describe('useActiveStore — canonical Active mirror', () => {
  beforeEach(() => {
    __resetActiveStoreForTests()
  })

  it('setFromSnapshot replaces the whole set + window', () => {
    useActiveStore.getState().setFromSnapshot({
      projectIds: ['a', 'b', 'c'],
      activeWindowHours: 12,
    })
    const s = useActiveStore.getState()
    expect([...s.activeProjectIds].sort()).toEqual(['a', 'b', 'c'])
    expect(s.activeWindowHours).toBe(12)
  })

  it('applyActiveChanged is a FULL-SET replace (not a merge) — last-write-wins', () => {
    useActiveStore.getState().setFromSnapshot({ projectIds: ['a', 'b'], activeWindowHours: 24 })
    // A delta that drops 'a' and adds 'c' replaces the set entirely.
    useActiveStore.getState().applyActiveChanged({
      activeProjectIds: ['b', 'c'],
      activeWindowHours: 24,
    })
    const s = useActiveStore.getState()
    expect([...s.activeProjectIds].sort()).toEqual(['b', 'c'])
    expect(s.activeProjectIds.has('a')).toBe(false)
  })

  it('an EMPTY delta clears the set (e.g. daemon reaped everything)', () => {
    useActiveStore.getState().setFromSnapshot({ projectIds: ['x', 'y'], activeWindowHours: 24 })
    useActiveStore.getState().applyActiveChanged({ activeProjectIds: [], activeWindowHours: 24 })
    expect(useActiveStore.getState().activeProjectIds.size).toBe(0)
  })

  it('a later snapshot wins over an earlier delta (monotonic convergence)', () => {
    useActiveStore.getState().applyActiveChanged({ activeProjectIds: ['a'], activeWindowHours: 24 })
    useActiveStore.getState().setFromSnapshot({ projectIds: ['a', 'b', 'c'], activeWindowHours: 6 })
    const s = useActiveStore.getState()
    expect([...s.activeProjectIds].sort()).toEqual(['a', 'b', 'c'])
    expect(s.activeWindowHours).toBe(6)
  })

  it('the window is clamped (defensive — mirrors settings clamp)', () => {
    useActiveStore.getState().setFromSnapshot({ projectIds: [], activeWindowHours: 0 })
    // clampActiveWindowHours floors sub-1 values to its minimum (>=1).
    expect(useActiveStore.getState().activeWindowHours).toBeGreaterThanOrEqual(1)
  })

  it('echoActive / echoInactive optimistically toggle membership', () => {
    useActiveStore.getState().setFromSnapshot({ projectIds: ['a'], activeWindowHours: 24 })
    useActiveStore.getState().echoActive('b')
    expect(useActiveStore.getState().activeProjectIds.has('b')).toBe(true)
    useActiveStore.getState().echoInactive('a')
    expect(useActiveStore.getState().activeProjectIds.has('a')).toBe(false)
    // A subsequent daemon delta reconciles (full replace) over the echo.
    useActiveStore.getState().applyActiveChanged({ activeProjectIds: ['a'], activeWindowHours: 24 })
    expect([...useActiveStore.getState().activeProjectIds]).toEqual(['a'])
  })

  it('replaces the underlying Set reference so zustand selectors re-fire', () => {
    const before = useActiveStore.getState().activeProjectIds
    useActiveStore.getState().applyActiveChanged({ activeProjectIds: ['z'], activeWindowHours: 24 })
    const after = useActiveStore.getState().activeProjectIds
    expect(after).not.toBe(before)
  })
})
