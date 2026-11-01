import { describe, it, expect, vi, beforeEach } from 'vitest'

const h = vi.hoisted(() => ({
  prefetchThread: vi.fn(async (..._a: unknown[]) => true),
  scopeForHost: vi.fn((host: string) => ({ hostKey: host, isRemote: host !== 'local' })),
}))
vi.mock('@/components/SessionView/threadRead', () => ({ prefetchThread: h.prefetchThread }))
vi.mock('@/kessel/server-scope', () => ({ scopeForHost: h.scopeForHost }))

import { createHoverPrefetch, HOVER_PREFETCH_DELAY_MS, prefetchHomeRow } from './home-row-prefetch'

beforeEach(() => {
  h.prefetchThread.mockClear()
  h.scopeForHost.mockClear()
})

describe('Home row hover prefetch', () => {
  it('a remote row reads its pinned Chat Thread (the row handle) on its server', () => {
    prefetchHomeRow('sales::rosson.k2.dev')
    expect(h.scopeForHost).toHaveBeenCalledWith('rosson.k2.dev')
    expect(h.prefetchThread).toHaveBeenCalledWith({ hostKey: 'rosson.k2.dev', isRemote: true }, 'sales')
  })

  it('a row on this computer, a bad address, or a server no longer saved reads nothing', () => {
    prefetchHomeRow('sales::local')
    prefetchHomeRow('not-an-address')
    h.scopeForHost.mockImplementationOnce(() => {
      throw new Error('removed')
    })
    prefetchHomeRow('sales::gone.k2.dev')
    expect(h.prefetchThread).not.toHaveBeenCalled()
  })

  it('is debounced: only a row the pointer rests on is read; leaving cancels', () => {
    vi.useFakeTimers()
    const prefetch = vi.fn()
    const hover = createHoverPrefetch({ prefetch, setTimeout, clearTimeout })
    hover.enter('a::x.k2.dev')
    vi.advanceTimersByTime(HOVER_PREFETCH_DELAY_MS - 10)
    hover.enter('b::x.k2.dev')
    vi.advanceTimersByTime(HOVER_PREFETCH_DELAY_MS - 10)
    hover.leave()
    vi.advanceTimersByTime(HOVER_PREFETCH_DELAY_MS * 2)
    expect(prefetch).not.toHaveBeenCalled()
    hover.enter('c::x.k2.dev')
    vi.advanceTimersByTime(HOVER_PREFETCH_DELAY_MS)
    expect(prefetch).toHaveBeenCalledTimes(1)
    expect(prefetch).toHaveBeenCalledWith('c::x.k2.dev')
    vi.useRealTimers()
  })
})
