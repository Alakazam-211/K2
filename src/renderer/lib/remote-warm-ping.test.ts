// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook } from '@testing-library/react'
import { createWarmPing, useRemoteWarmPing, WARM_PING_INTERVAL_MS } from './remote-warm-ping'
import { noteHostActivity, resetCliTimingsForTests } from './cli-timing'
import type { ServerScope } from '@/kessel/server-scope'

function fakeTimers() {
  let next = 1
  const live = new Map<number, () => void>()
  return {
    setInterval: (fn: () => void) => {
      const id = next++
      live.set(id, fn)
      return id
    },
    clearInterval: (h: unknown) => void live.delete(h as number),
    fire: () => [...live.values()].forEach((fn) => fn()),
    count: () => live.size,
  }
}

describe('createWarmPing', () => {
  it('pings only while active, and not when the connection was used recently', async () => {
    const t = fakeTimers()
    let now = 100_000
    let last: number | null = null
    const ping = vi.fn(async () => {})
    const w = createWarmPing({ ping, lastActivity: () => last, now: () => now, ...t })
    t.fire()
    expect(ping).not.toHaveBeenCalled()
    w.setActive(true)
    w.setActive(true)
    expect(t.count()).toBe(1)
    t.fire()
    expect(ping).toHaveBeenCalledTimes(1)
    await new Promise((r) => setTimeout(r, 0))
    // Another request just went there: no ping.
    last = now - 1_000
    t.fire()
    expect(ping).toHaveBeenCalledTimes(1)
    now += WARM_PING_INTERVAL_MS
    t.fire()
    expect(ping).toHaveBeenCalledTimes(2)
    w.setActive(false)
    expect(t.count()).toBe(0)
    w.dispose()
    w.setActive(true)
    expect(t.count()).toBe(0)
  })

  it('one ping in flight at a time; a failed ping is ignored', async () => {
    const t = fakeTimers()
    let fail: (e: Error) => void = () => {}
    const ping = vi.fn(() => new Promise<void>((_r, reject) => (fail = reject)))
    const w = createWarmPing({ ping, lastActivity: () => null, now: () => 0, ...t })
    w.setActive(true)
    t.fire()
    t.fire()
    expect(ping).toHaveBeenCalledTimes(1)
    fail(new Error('offline'))
    await new Promise((r) => setTimeout(r, 0))
    t.fire()
    expect(ping).toHaveBeenCalledTimes(2)
  })
})

describe('useRemoteWarmPing', () => {
  const fetchMock = vi.fn(async () => ({ text: async () => '{}' }) as unknown as Response)
  let visibility: 'visible' | 'hidden' = 'visible'

  beforeEach(() => {
    vi.useFakeTimers()
    resetCliTimingsForTests()
    fetchMock.mockClear()
    vi.stubGlobal('fetch', fetchMock)
    visibility = 'visible'
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  function scope(remote: boolean): ServerScope {
    return {
      hostKey: remote ? 'rosson.k2.dev' : 'local',
      isRemote: remote,
      httpBase: async () => (remote ? 'https://rosson.k2.dev' : 'http://127.0.0.1:1'),
    } as unknown as ServerScope
  }

  it('a shown remote room pings /boot-status under 60 s; hidden or a hidden window stops it', async () => {
    const s = scope(true)
    const { rerender, unmount } = renderHook(({ shown }) => useRemoteWarmPing(s, shown), { initialProps: { shown: true } })
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS)
    expect(WARM_PING_INTERVAL_MS).toBeLessThan(60_000)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledWith('https://rosson.k2.dev/boot-status', { method: 'GET', cache: 'no-store' })
    // Traffic of its own keeps it warm: no ping.
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS - 5_000)
    noteHostActivity('rosson.k2.dev', Date.now())
    await vi.advanceTimersByTimeAsync(5_000)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    // The window hides: nothing.
    visibility = 'hidden'
    document.dispatchEvent(new Event('visibilitychange'))
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS * 3)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    visibility = 'visible'
    document.dispatchEvent(new Event('visibilitychange'))
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS)
    expect(fetchMock).toHaveBeenCalledTimes(2)
    // The room is not shown: nothing.
    rerender({ shown: false })
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS * 3)
    expect(fetchMock).toHaveBeenCalledTimes(2)
    unmount()
  })

  it('a local room never pings', async () => {
    const s = scope(false)
    renderHook(() => useRemoteWarmPing(s, true))
    await vi.advanceTimersByTimeAsync(WARM_PING_INTERVAL_MS * 3)
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
