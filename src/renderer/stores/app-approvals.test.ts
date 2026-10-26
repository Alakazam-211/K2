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

vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => ({ id: 'primary', isPrimary: true, hostKey: 'local' }) }))

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
    expect(api.fetchAppApprovals).toHaveBeenCalledWith('pending')

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
    useAppApprovalsStore.setState({ pending: 4 })
    api.fetchAppApprovals.mockRejectedValue(new Error('daemon unreachable'))
    await useAppApprovalsStore.getState().refresh()
    expect(useAppApprovalsStore.getState().pending).toBe(4)
  })
})
