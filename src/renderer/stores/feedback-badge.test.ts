// prd-tickets-badge-orphans — the Tickets badge's freshness, against the REAL
// connect-host store, server-scope feature registry and soft-resync bus.
//   T6:  a failed refresh keeps the number and marks it stale; the next good
//        one clears it.
//   T9:  the connection leaving `connected` marks it stale with no fetch; the
//        heal's soft resync re-reads it (one fetch) and clears it (TB15).
//   T10: a server switch resets the count to 0 before the new server answers,
//        and a late answer from the old server never lands (TB16).
//   A3/TB14: a server that does not report `tickets-list-all` gets a dimmed
//        `?` and no fetch.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

// The feedback event bus is not under test here; record and stay inert.
vi.mock('@/stores/session-events', () => ({
  onFeedbackChanged: vi.fn(() => () => {}),
}))

const api = vi.hoisted(() => ({ fetchWaitingCount: vi.fn() }))
vi.mock('@/components/Feedback/feedback-api', () => ({
  fetchWaitingCount: api.fetchWaitingCount,
}))

import { useFeedbackStore, initFeedbackEvents } from '@/stores/feedback'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'
import { noteServerVersion, __resetServerScopesForTests } from '@/kessel/server-scope'
import { homeHostKey } from '@/lib/host-key'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'h1',
    label: 'Box',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'tok-rosson',
    remember: false,
    lastConnectedAt: null,
    ...over,
  }
}

const A = host({ id: 'hA', label: 'A', hostname: '192.168.1.20', port: 38471, secure: false, token: 'tok-a' })
const B = host({ id: 'hB', label: 'B', hostname: 'baden.k2.dev' })

function badge(): { count: number; stale: boolean; unsupported: boolean } {
  const s = useFeedbackStore.getState()
  return { count: s.waitingCount, stale: s.waitingStale, unsupported: s.waitingUnsupported }
}

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void
  const promise = new Promise<T>((r) => (resolve = r))
  return { promise, resolve }
}

beforeEach(() => {
  mem.clear()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  useConnectHostStore.getState().addHost(A)
  useConnectHostStore.getState().addHost(B)
  initFeedbackEvents(false)
  useFeedbackStore.setState({ waitingCount: 0, waitingStale: false, waitingUnsupported: false })
  api.fetchWaitingCount.mockReset()
})

describe('Tickets badge freshness', () => {
  it('T6: a failed refresh keeps the number and marks it stale; the next success clears it', async () => {
    api.fetchWaitingCount.mockResolvedValueOnce(3)
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 3, stale: false, unsupported: false })

    api.fetchWaitingCount.mockRejectedValueOnce(new Error('host unreachable'))
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 3, stale: true, unsupported: false })

    api.fetchWaitingCount.mockResolvedValueOnce(1)
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 1, stale: false, unsupported: false })
  })

  it('A3/TB14: a server without tickets-list-all shows ? (no fetch); once it reports the key it counts', async () => {
    useConnectHostStore.getState().selectHost(A)
    noteServerVersion(homeHostKey(A), '0.41.6', [])
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 0, stale: false, unsupported: true })
    expect(api.fetchWaitingCount).not.toHaveBeenCalled()

    noteServerVersion(homeHostKey(A), '0.41.7', ['spawn-attach-only', 'tickets-list-all'])
    api.fetchWaitingCount.mockResolvedValueOnce(2)
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 2, stale: false, unsupported: false })
    expect(api.fetchWaitingCount).toHaveBeenCalledTimes(1)
  })

  it('T9: leaving connected marks the badge stale with no fetch; the heal re-reads it once', async () => {
    useConnectHostStore.getState().selectHost(A)
    noteServerVersion(homeHostKey(A), '0.41.7', ['tickets-list-all'])
    api.fetchWaitingCount.mockResolvedValueOnce(2)
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 2, stale: false, unsupported: false })
    api.fetchWaitingCount.mockClear()

    useConnectHostStore.getState().setRecovery({ kind: 'reconnecting', bootPhase: null })
    expect(badge()).toEqual({ count: 2, stale: true, unsupported: false })
    expect(api.fetchWaitingCount).not.toHaveBeenCalled()

    api.fetchWaitingCount.mockResolvedValueOnce(2)
    useConnectHostStore.getState().setRecovery({ kind: 'connected' })
    await vi.waitFor(() => expect(api.fetchWaitingCount).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(badge()).toEqual({ count: 2, stale: false, unsupported: false }))
  })

  it('T10: a server switch resets the count; the new server failing shows no badge, not the old number', async () => {
    useConnectHostStore.getState().selectHost(A)
    noteServerVersion(homeHostKey(A), '0.41.7', ['tickets-list-all'])
    noteServerVersion(homeHostKey(B), '0.41.7', ['tickets-list-all'])
    api.fetchWaitingCount.mockResolvedValueOnce(3)
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(badge()).toEqual({ count: 3, stale: false, unsupported: false })

    useConnectHostStore.getState().selectHost(B)
    expect(badge()).toEqual({ count: 0, stale: false, unsupported: false })

    api.fetchWaitingCount.mockRejectedValueOnce(new Error('B is down'))
    await useFeedbackStore.getState().refreshWaitingCount()
    expect(useFeedbackStore.getState().waitingCount).toBe(0)
  })

  it("T10: server A's late answer never lands after the switch to B", async () => {
    useConnectHostStore.getState().selectHost(A)
    noteServerVersion(homeHostKey(A), '0.41.7', ['tickets-list-all'])
    const late = deferred<number>()
    api.fetchWaitingCount.mockReturnValueOnce(late.promise)
    const inFlight = useFeedbackStore.getState().refreshWaitingCount()

    useConnectHostStore.getState().selectHost(B)
    late.resolve(7)
    await inFlight
    expect(badge()).toEqual({ count: 0, stale: false, unsupported: false })
  })
})
