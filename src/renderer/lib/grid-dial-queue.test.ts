import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  BACKOFF_MS,
  HANDSHAKE_TIMEOUT_MS,
  gridDialBackoffRemainingMs,
  noteGridDialFailure,
  openQueuedGridWebSocket,
  resetGridDialQueueForTests,
  setGridDialMaxForTests,
} from './grid-dial-queue'

import { primaryScope } from '@/kessel/server-scope'
class FakeWebSocket {
  static pending: FakeWebSocket[] = []
  static throwOnConstruct = false
  readyState = 0
  binaryType = ''
  onopen: ((ev?: Event) => void) | null = null
  onerror: ((ev?: Event) => void) | null = null
  onclose: ((ev?: CloseEvent) => void) | null = null

  constructor(public url: string) {
    if (FakeWebSocket.throwOnConstruct) {
      throw new Error('Insufficient resources')
    }
    FakeWebSocket.pending.push(this)
  }

  open(): void {
    this.readyState = 1
    this.onopen?.(new Event('open'))
  }

  fail(): void {
    this.readyState = 3
    this.onerror?.(new Event('error'))
  }

  close(): void {
    this.readyState = 3
  }
}

describe('grid-dial-queue', () => {
  beforeEach(() => {
    resetGridDialQueueForTests()
    FakeWebSocket.pending = []
    FakeWebSocket.throwOnConstruct = false
  })

  it('does not back off after one or two failures', () => {
    const t0 = 1_000_000
    noteGridDialFailure(primaryScope(), t0)
    noteGridDialFailure(primaryScope(), t0 + 100)
    expect(gridDialBackoffRemainingMs(primaryScope(), t0 + 200)).toBe(0)
  })

  it('backs off all panes after a burst of failed dials', () => {
    const t0 = 2_000_000
    noteGridDialFailure(primaryScope(), t0)
    noteGridDialFailure(primaryScope(), t0 + 50)
    noteGridDialFailure(primaryScope(), t0 + 100)
    const remain = gridDialBackoffRemainingMs(primaryScope(), t0 + 200)
    expect(remain).toBeGreaterThan(5_000)
    expect(remain).toBeLessThanOrEqual(BACKOFF_MS)
  })

  it('does not count failures outside the burst window', () => {
    const t0 = 3_000_000
    noteGridDialFailure(primaryScope(), t0)
    noteGridDialFailure(primaryScope(), t0 + 50)
    noteGridDialFailure(primaryScope(), t0 + 3_100)
    expect(gridDialBackoffRemainingMs(primaryScope(), t0 + 3_200)).toBe(0)
  })

  it('constructs at most one WebSocket until a handshake settles', async () => {
    setGridDialMaxForTests(1)
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const p1 = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')
      const p2 = openQueuedGridWebSocket(primaryScope(), 'wss://b/grid')
      const p3 = openQueuedGridWebSocket(primaryScope(), 'wss://c/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)

      FakeWebSocket.pending[0]!.open()
      await expect(p1).resolves.toBe(FakeWebSocket.pending[0])
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(2)

      FakeWebSocket.pending[1]!.open()
      await expect(p2).resolves.toBe(FakeWebSocket.pending[1])
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(3)

      FakeWebSocket.pending[2]!.open()
      await expect(p3).resolves.toBe(FakeWebSocket.pending[2])
    } finally {
      globalThis.WebSocket = prev
    }
  })

  it('remote cap of 2 starts two handshakes at once', async () => {
    setGridDialMaxForTests(2)
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const p1 = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')
      const p2 = openQueuedGridWebSocket(primaryScope(), 'wss://b/grid')
      const p3 = openQueuedGridWebSocket(primaryScope(), 'wss://c/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(2)
      FakeWebSocket.pending[0]!.open()
      await expect(p1).resolves.toBe(FakeWebSocket.pending[0])
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(3)
      FakeWebSocket.pending[1]!.open()
      FakeWebSocket.pending[2]!.open()
      await expect(p2).resolves.toBe(FakeWebSocket.pending[1])
      await expect(p3).resolves.toBe(FakeWebSocket.pending[2])
    } finally {
      globalThis.WebSocket = prev
    }
  })

  it('counts a failed handshake toward the global backoff', async () => {
    vi.useFakeTimers()
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const now = 4_000_000
      vi.setSystemTime(now)

      const first = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)
      FakeWebSocket.pending[0]!.fail()
      await expect(first).rejects.toThrow('grid-dial-failed')

      const second = openQueuedGridWebSocket(primaryScope(), 'wss://b/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(2)
      FakeWebSocket.pending[1]!.fail()
      await expect(second).rejects.toThrow('grid-dial-failed')

      const third = openQueuedGridWebSocket(primaryScope(), 'wss://c/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(3)
      FakeWebSocket.pending[2]!.fail()
      await expect(third).rejects.toThrow('grid-dial-failed')
      expect(gridDialBackoffRemainingMs(primaryScope(), now)).toBe(BACKOFF_MS)
    } finally {
      globalThis.WebSocket = prev
      vi.useRealTimers()
    }
  })

  it('counts a constructor throw toward the global backoff', async () => {
    vi.useFakeTimers()
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    FakeWebSocket.throwOnConstruct = true
    try {
      const now = 5_000_000
      vi.setSystemTime(now)
      await expect(openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')).rejects.toThrow(
        'grid-dial-failed',
      )
      await expect(openQueuedGridWebSocket(primaryScope(), 'wss://b/grid')).rejects.toThrow(
        'grid-dial-failed',
      )
      await expect(openQueuedGridWebSocket(primaryScope(), 'wss://c/grid')).rejects.toThrow(
        'grid-dial-failed',
      )
      expect(gridDialBackoffRemainingMs(primaryScope(), now)).toBe(BACKOFF_MS)
      expect(FakeWebSocket.pending).toHaveLength(0)
    } finally {
      globalThis.WebSocket = prev
      vi.useRealTimers()
    }
  })

  it('times out a hung handshake and releases the slot', async () => {
    setGridDialMaxForTests(1)
    vi.useFakeTimers()
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const hung = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)

      const waiter = openQueuedGridWebSocket(primaryScope(), 'wss://b/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)

      // Attach before advancing — the timeout rejects `hung` synchronously
      // with fake timers, which Vitest otherwise flags as unhandled.
      const hungAssert = expect(hung).rejects.toThrow('grid-dial-failed')
      await vi.advanceTimersByTimeAsync(HANDSHAKE_TIMEOUT_MS)
      await hungAssert
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(2)

      FakeWebSocket.pending[1]!.open()
      await expect(waiter).resolves.toBe(FakeWebSocket.pending[1])
    } finally {
      globalThis.WebSocket = prev
      vi.useRealTimers()
    }
  })

  it('does not construct a waiter while global backoff is armed', async () => {
    vi.useFakeTimers()
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const now = 6_000_000
      vi.setSystemTime(now)
      for (let i = 0; i < 3; i++) {
        const p = openQueuedGridWebSocket(primaryScope(), `wss://${i}/grid`)
        await Promise.resolve()
        FakeWebSocket.pending[i]!.fail()
        await expect(p).rejects.toThrow('grid-dial-failed')
      }
      expect(gridDialBackoffRemainingMs(primaryScope(), now)).toBe(BACKOFF_MS)

      const blocked = openQueuedGridWebSocket(primaryScope(), 'wss://blocked/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(3)

      await vi.advanceTimersByTimeAsync(BACKOFF_MS)
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(4)
      FakeWebSocket.pending[3]!.open()
      await expect(blocked).resolves.toBe(FakeWebSocket.pending[3])
    } finally {
      globalThis.WebSocket = prev
      vi.useRealTimers()
    }
  })

  it('skips construct when cancelled after the slot is granted', async () => {
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      await expect(
        openQueuedGridWebSocket(primaryScope(), 'wss://a/grid', { isCancelled: () => true }),
      ).rejects.toThrow('grid-dial-aborted')
      expect(FakeWebSocket.pending).toHaveLength(0)
      expect(gridDialBackoffRemainingMs(primaryScope())).toBe(0)
    } finally {
      globalThis.WebSocket = prev
    }
  })

  it('aborts a queued waiter without constructing or counting a failure', async () => {
    setGridDialMaxForTests(1)
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const held = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid')
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)

      const ac = new AbortController()
      const queued = openQueuedGridWebSocket(primaryScope(), 'wss://b/grid', { signal: ac.signal })
      await Promise.resolve()
      expect(FakeWebSocket.pending).toHaveLength(1)

      ac.abort()
      await expect(queued).rejects.toThrow('grid-dial-aborted')
      expect(FakeWebSocket.pending).toHaveLength(1)
      expect(gridDialBackoffRemainingMs(primaryScope())).toBe(0)

      FakeWebSocket.pending[0]!.open()
      await expect(held).resolves.toBe(FakeWebSocket.pending[0])
    } finally {
      globalThis.WebSocket = prev
    }
  })

  it('runs beforeDial after the slot and before construct', async () => {
    const prev = globalThis.WebSocket
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket
    try {
      const order: string[] = []
      const p = openQueuedGridWebSocket(primaryScope(), 'wss://a/grid', {
        beforeDial: () => {
          order.push('before')
          expect(FakeWebSocket.pending).toHaveLength(0)
        },
      })
      await Promise.resolve()
      expect(order).toEqual(['before'])
      expect(FakeWebSocket.pending).toHaveLength(1)
      FakeWebSocket.pending[0]!.open()
      await expect(p).resolves.toBe(FakeWebSocket.pending[0])
    } finally {
      globalThis.WebSocket = prev
    }
  })
})
