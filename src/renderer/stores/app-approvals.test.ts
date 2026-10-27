// 0.46.4 field report: a held `k2 app install` was listed by `k2 app
// approvals` but the owner's App Approvals showed nothing (loaded once,
// never on the daemon's push; no badge). The store keeps the badge count
// from the daemon's `app_approvals_changed` push and its own refresh, and
// bumps `revision` so an open panel reloads.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const bus = vi.hoisted(() => ({ handler: null as null | ((pending: number) => void) }))
vi.mock('@/stores/session-events', () => ({
  onAppApprovalsChanged: vi.fn((_scope: unknown, fn: (pending: number) => void) => {
    bus.handler = fn
    return () => {}
  }),
}))

const api = vi.hoisted(() => ({ fetchAppApprovals: vi.fn() }))
vi.mock('@/components/Settings/sections/app-approvals-api', () => ({
  fetchAppApprovals: api.fetchAppApprovals,
}))

const PRIMARY = vi.hoisted(() => ({ id: 'primary', isPrimary: true, hostKey: 'local', label: 'This computer' }))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => PRIMARY,
  scopeForHost: (hostKey: string) => ({ id: `host:${hostKey}`, isPrimary: false, hostKey, label: hostKey }),
}))

// 0.46.5: the Home pool says which other servers this window owns.
const pool = vi.hoisted(() => {
  let state = { entries: {} as Record<string, unknown> }
  const subs: Array<() => void> = []
  return {
    hostPool: {
      store: {
        getState: () => state,
        subscribe: (fn: () => void) => {
          subs.push(fn)
          return () => {}
        },
      },
    },
    set(entries: Record<string, unknown>) {
      state = { entries }
      for (const fn of subs) fn()
    },
  }
})
vi.mock('@/lib/host-pool-instance', () => ({ hostPool: pool.hostPool }))

function entry(hostKey: string, over: Record<string, unknown> = {}) {
  return { hostKey, saved: true, auth: 'ok', role: 'owner', ...over }
}

const host = vi.hoisted(() => {
  let state = { activeHost: null as unknown }
  const subs: Array<(s: typeof state, p: typeof state) => void> = []
  return {
    store: {
      getState: () => state,
      subscribe: (fn: (s: typeof state, p: typeof state) => void) => {
        subs.push(fn)
        return () => {}
      },
    },
    switchTo(h: unknown) {
      const prev = state
      state = { activeHost: h }
      for (const fn of subs) fn(state, prev)
    },
  }
})
vi.mock('@/stores/connect-host', () => ({ useConnectHostStore: host.store }))

import { useAppApprovalsStore, initAppApprovalsEvents } from '@/stores/app-approvals'

function rows(statuses: string[]) {
  return { requests: statuses.map((status, i) => ({ id: `w_${i}`, status })), ownerConfirm: { available: true, methods: [] } }
}

describe('App Approvals badge store', () => {
  beforeEach(() => {
    api.fetchAppApprovals.mockReset()
  })

  it('counts the pending requests on init, follows the push, and resets on a server switch', async () => {
    api.fetchAppApprovals.mockResolvedValue(rows(['pending', 'pending']))
    initAppApprovalsEvents()
    await vi.waitFor(() => expect(useAppApprovalsStore.getState().pending).toBe(2))
    expect(api.fetchAppApprovals).toHaveBeenCalledWith('pending', PRIMARY)

    // The daemon's push carries the count and bumps the panel's revision.
    const before = useAppApprovalsStore.getState().revision
    expect(bus.handler).not.toBeNull()
    bus.handler!(3)
    expect(useAppApprovalsStore.getState().pending).toBe(3)
    expect(useAppApprovalsStore.getState().revision).toBe(before + 1)

    // A server switch: 0 until the new server answers, then its count.
    api.fetchAppApprovals.mockResolvedValue(rows(['pending']))
    host.switchTo({ hostKey: 'other.k2.dev' })
    expect(useAppApprovalsStore.getState().pending).toBe(0)
    await vi.waitFor(() => expect(useAppApprovalsStore.getState().pending).toBe(1))
  })

  it('keeps the last count when the daemon can not be read', async () => {
    useAppApprovalsStore.setState({ pending: 4, windowPending: 4, otherPending: 0 })
    api.fetchAppApprovals.mockRejectedValue(new Error('daemon unreachable'))
    await useAppApprovalsStore.getState().refresh()
    expect(useAppApprovalsStore.getState().pending).toBe(4)
  })

  it('0.46.5: counts every server the window owns, not only its own', async () => {
    // Back on this computer (the first test switched servers).
    host.switchTo(null)
    host.switchTo({ hostKey: 'local' })
    api.fetchAppApprovals.mockImplementation(async (_state: string, scope: { hostKey: string }) => {
      if (scope.hostKey === 'local') return rows(['pending'])
      if (scope.hostKey === 'box.k2.dev') return rows(['pending', 'pending', 'approved'])
      if (scope.hostKey === 'down.k2.dev') throw new Error('offline')
      throw new Error(`never read ${scope.hostKey}`)
    })
    // Owned and signed in: read. An Admin, a server not signed in, an
    // unsaved one: never read.
    pool.set({
      'box.k2.dev': entry('box.k2.dev'),
      'down.k2.dev': entry('down.k2.dev'),
      'admin.k2.dev': entry('admin.k2.dev', { role: 'admin' }),
      'out.k2.dev': entry('out.k2.dev', { auth: 'signin-required' }),
      'gone.k2.dev': entry('gone.k2.dev', { saved: false }),
    })
    await vi.waitFor(() => expect(useAppApprovalsStore.getState().pending).toBe(3))
    const read = api.fetchAppApprovals.mock.calls.map((c) => (c[1] as { hostKey: string }).hostKey)
    expect(new Set(read)).toEqual(new Set(['local', 'box.k2.dev', 'down.k2.dev']))

    // The window's push replaces only its own share.
    const rev = useAppApprovalsStore.getState().revision
    bus.handler!(0)
    expect(useAppApprovalsStore.getState().pending).toBe(2)
    expect(useAppApprovalsStore.getState().revision).toBe(rev + 1)

    // An owned server leaving the pool's list re-reads and drops its share.
    pool.set({})
    await vi.waitFor(() => expect(useAppApprovalsStore.getState().pending).toBe(1))
  })
})
