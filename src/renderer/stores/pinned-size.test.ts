// S7b — sessionId→pinnedSize mirror store. Pins the tab-menu contract:
// TerminalPane registers terminalId→sessionId at spawn-resolve and
// mirrors pin frames + measured dims; PaneTabBar reads all three maps
// with no polling. Every setter is idempotent (same value ⇒ SAME state
// object) so frame-rate writers cause zero re-render churn.

import { describe, it, expect, beforeEach } from 'vitest'
import { usePinnedSizeStore } from './pinned-size'

beforeEach(() => {
  usePinnedSizeStore.setState({ pins: {}, sessions: {}, dims: {} })
})

describe('pinned-size store — pins', () => {
  it('setPin stores, updates and clears a pin by sessionId', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin('sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    expect(usePinnedSizeStore.getState().pins['sess-1']).toEqual({
      cols: 100,
      rows: 30,
      setBy: 'owner',
    })

    s.setPin('sess-1', { cols: 80, rows: 24, setBy: null })
    expect(usePinnedSizeStore.getState().pins['sess-1']).toEqual({
      cols: 80,
      rows: 24,
      setBy: null,
    })

    s.setPin('sess-1', null)
    expect(usePinnedSizeStore.getState().pins['sess-1']).toBeUndefined()
  })

  it('is idempotent: re-setting the same pin (or clearing an absent one) keeps the same state object', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin('sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    const before = usePinnedSizeStore.getState()
    s.setPin('sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    expect(usePinnedSizeStore.getState()).toBe(before)

    s.setPin('never-pinned', null)
    expect(usePinnedSizeStore.getState()).toBe(before)
  })

  it('keeps pins independent per session', () => {
    const s = usePinnedSizeStore.getState()
    s.setPin('sess-1', { cols: 100, rows: 30, setBy: 'owner' })
    s.setPin('sess-2', { cols: 80, rows: 24, setBy: 'alice' })
    s.setPin('sess-1', null)
    expect(usePinnedSizeStore.getState().pins['sess-1']).toBeUndefined()
    expect(usePinnedSizeStore.getState().pins['sess-2']).toEqual({
      cols: 80,
      rows: 24,
      setBy: 'alice',
    })
  })
})

describe('pinned-size store — session registration (tab↔pane join)', () => {
  it('register/unregister maps a terminalId to its daemon sessionId', () => {
    const s = usePinnedSizeStore.getState()
    s.registerSession('term-1', 'sess-1')
    expect(usePinnedSizeStore.getState().sessions['term-1']).toBe('sess-1')

    s.unregisterSession('term-1')
    expect(usePinnedSizeStore.getState().sessions['term-1']).toBeUndefined()
  })

  it('re-registering the same mapping is a state no-op; a reconnect can re-point it', () => {
    const s = usePinnedSizeStore.getState()
    s.registerSession('term-1', 'sess-1')
    const before = usePinnedSizeStore.getState()
    s.registerSession('term-1', 'sess-1')
    expect(usePinnedSizeStore.getState()).toBe(before)

    s.registerSession('term-1', 'sess-2')
    expect(usePinnedSizeStore.getState().sessions['term-1']).toBe('sess-2')
  })

  it('unregistering an unknown terminalId is a state no-op', () => {
    const before = usePinnedSizeStore.getState()
    before.unregisterSession('ghost')
    expect(usePinnedSizeStore.getState()).toBe(before)
  })
})

describe('pinned-size store — measured dims (Match my window now)', () => {
  it('records and overwrites the latest measured dims per session', () => {
    const s = usePinnedSizeStore.getState()
    s.setDims('sess-1', 120, 36)
    expect(usePinnedSizeStore.getState().dims['sess-1']).toEqual({
      cols: 120,
      rows: 36,
    })
    s.setDims('sess-1', 90, 28)
    expect(usePinnedSizeStore.getState().dims['sess-1']).toEqual({
      cols: 90,
      rows: 28,
    })
  })

  it('same dims are a state no-op (ResizeObserver-rate writer)', () => {
    const s = usePinnedSizeStore.getState()
    s.setDims('sess-1', 120, 36)
    const before = usePinnedSizeStore.getState()
    s.setDims('sess-1', 120, 36)
    expect(usePinnedSizeStore.getState()).toBe(before)
  })
})
