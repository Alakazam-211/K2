// S7b — sessionId→pinnedSize mirror store. Pins the tab-menu contract:
// TerminalPane registers terminalId→sessionId at spawn-resolve and
// mirrors pin frames + measured dims; PaneTabBar reads all three maps
// with no polling. Every setter is idempotent (same value ⇒ SAME state
// object) so frame-rate writers cause zero re-render churn.

import { describe, it, expect, beforeEach } from 'vitest'
import { dimsOf, pinOf, sessionsOf, usePinnedSizeStore } from './pinned-size'
import { primaryScope } from '@/kessel/server-scope'

beforeEach(() => {
  usePinnedSizeStore.setState({ pins: {}, sessions: {}, dims: {} })
})

describe('pinned-size store — pins', () => {
  it('setPin stores, updates and clears a pin by sessionId', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin(primaryScope(), 'sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    expect(pinOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toEqual({
      cols: 100,
      rows: 30,
      setBy: 'owner',
    })

    s.setPin(primaryScope(), 'sess-1', { cols: 80, rows: 24, setBy: null })
    expect(pinOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toEqual({
      cols: 80,
      rows: 24,
      setBy: null,
    })

    s.setPin(primaryScope(), 'sess-1', null)
    expect(pinOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toBeUndefined()
  })

  it('is idempotent: re-setting the same pin (or clearing an absent one) keeps the same state object', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin(primaryScope(), 'sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    const before = usePinnedSizeStore.getState()
    s.setPin(primaryScope(), 'sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    expect(usePinnedSizeStore.getState()).toBe(before)

    s.setPin(primaryScope(), 'never-pinned', null)
    expect(usePinnedSizeStore.getState()).toBe(before)
  })

  it('keeps pins independent per session', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin(primaryScope(), 'sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    s.setPin(primaryScope(), 'sess-2', { cols: 80, rows: 24, setBy: 'alice' })
    s.setPin(primaryScope(), 'sess-1', null)
    expect(pinOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toBeUndefined()
    expect(pinOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-2')).toEqual({
      cols: 80,
      rows: 24,
      setBy: 'alice',
    })
  })
})

describe('pinned-size store — session registration (tab↔pane join)', () => {
  it('register/unregister maps a terminalId to its daemon sessionId', () => {
    const s = usePinnedSizeStore.getState()
    s.registerSession(primaryScope(), 'term-1', 'sess-1')
    expect(sessionsOf(usePinnedSizeStore.getState().sessions, primaryScope())['term-1']).toBe('sess-1')

    s.unregisterSession(primaryScope(), 'term-1')
    expect(sessionsOf(usePinnedSizeStore.getState().sessions, primaryScope())['term-1']).toBeUndefined()
  })

  it('re-registering the same mapping is a state no-op; a reconnect can re-point it', () => {
    const s = usePinnedSizeStore.getState()
    s.registerSession(primaryScope(), 'term-1', 'sess-1')
    const before = usePinnedSizeStore.getState()
    s.registerSession(primaryScope(), 'term-1', 'sess-1')
    expect(usePinnedSizeStore.getState()).toBe(before)

    s.registerSession(primaryScope(), 'term-1', 'sess-2')
    expect(sessionsOf(usePinnedSizeStore.getState().sessions, primaryScope())['term-1']).toBe('sess-2')
  })

  it('unregistering an unknown terminalId is a state no-op', () => {
    const before = usePinnedSizeStore.getState()
    before.unregisterSession(primaryScope(), 'ghost')
    expect(usePinnedSizeStore.getState()).toBe(before)
  })
})

describe('pinned-size store — measured dims (Match my window now)', () => {
  it('records and overwrites the latest measured dims per session', () => {
    const s = usePinnedSizeStore.getState()
    s.setDims(primaryScope(), 'sess-1', 120, 36)
    expect(dimsOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toEqual({
      cols: 120,
      rows: 36,
    })
    s.setDims(primaryScope(), 'sess-1', 90, 28)
    expect(dimsOf(usePinnedSizeStore.getState(), primaryScope(), 'sess-1')).toEqual({
      cols: 90,
      rows: 28,
    })
  })

  it('same dims are a state no-op (ResizeObserver-rate writer)', () => {
    const s = usePinnedSizeStore.getState()
    s.setDims(primaryScope(), 'sess-1', 120, 36)
    const before = usePinnedSizeStore.getState()
    s.setDims(primaryScope(), 'sess-1', 120, 36)
    expect(usePinnedSizeStore.getState()).toBe(before)
  })
})
