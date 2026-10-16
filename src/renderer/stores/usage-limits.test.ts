// @vitest-environment jsdom
//
// Usage v2 limits store: one watcher per server, push refetches, a
// Member's 403 kept as a status, and a per-login refresh.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const h = vi.hoisted(() => ({
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: unknown }>,
  body: null as unknown,
  fail: null as Error | null,
  usageHandlers: new Set<() => void>(),
  accountHandlers: new Set<() => void>(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (_scope: unknown, route: string) => {
    h.gets.push(route)
    if (h.fail) throw h.fail
    return h.body
  },
  daemonCliPost: async (_scope: unknown, route: string, body: unknown) => {
    h.posts.push({ route, body })
    if (h.fail) throw h.fail
    return h.body
  },
}))

vi.mock('@/stores/connect-host', () => ({ onActiveHostChange: () => () => {} }))

vi.mock('@/stores/session-events', () => ({
  onSubscriptionUsageChanged: (_scope: unknown, fn: () => void) => {
    h.usageHandlers.add(fn)
    return () => h.usageHandlers.delete(fn)
  },
  onLlmAccountsChanged: (_scope: unknown, fn: () => void) => {
    h.accountHandlers.add(fn)
    return () => h.accountHandlers.delete(fn)
  },
}))

import {
  LIMITS_POLL_MS,
  resetUsageLimitsForTests,
  useUsageLimitsStore,
  usageLimitsWatchHolds,
  watchUsageLimits,
} from './usage-limits'

const target = { key: 'srv', scope: { id: 'srv' } as unknown as ServerScope }

function doc(used: number): unknown {
  return {
    generatedAt: '',
    airgap: false,
    tools: [{ tool: 'claude', display: 'Claude', supported: true, logins: [{ accountId: 'acc_a', label: 'a', isDefault: true, windows: [{ label: 'Weekly', used, resetsAt: '' }] }] }],
  }
}

function usedNow(): number {
  const entry = useUsageLimitsStore.getState().entries.srv
  if (!entry?.doc) throw new Error('no limits doc')
  return entry.doc.tools[0].logins[0].windows[0].used
}

beforeEach(() => {
  vi.useFakeTimers()
  h.gets.length = 0
  h.posts.length = 0
  h.body = doc(0.1)
  h.fail = null
  h.usageHandlers.clear()
  h.accountHandlers.clear()
  resetUsageLimitsForTests()
})

afterEach(() => {
  resetUsageLimitsForTests()
  vi.useRealTimers()
})

describe('watchUsageLimits', () => {
  it('loads once for every holder of the same server and stops at the last release (B32)', async () => {
    const a = watchUsageLimits(target)
    const b = watchUsageLimits(target)
    await vi.advanceTimersByTimeAsync(0)
    expect(h.gets).toEqual(['usage/limits'])
    expect(usageLimitsWatchHolds('srv')).toBe(2)
    expect(h.usageHandlers.size).toBe(1)
    await vi.advanceTimersByTimeAsync(LIMITS_POLL_MS)
    expect(h.gets).toHaveLength(2)
    a()
    b()
    expect(h.usageHandlers.size).toBe(0)
    expect(h.accountHandlers.size).toBe(0)
    await vi.advanceTimersByTimeAsync(LIMITS_POLL_MS * 3)
    expect(h.gets).toHaveLength(2)
  })

  it('re-reads on subscription_usage_changed and llm_accounts_changed', async () => {
    const stop = watchUsageLimits(target)
    await vi.advanceTimersByTimeAsync(0)
    expect(usedNow()).toBe(0.1)
    h.body = doc(0.5)
    for (const fn of h.usageHandlers) fn()
    await vi.advanceTimersByTimeAsync(0)
    expect(usedNow()).toBe(0.5)
    h.body = doc(0.6)
    for (const fn of h.accountHandlers) fn()
    await vi.advanceTimersByTimeAsync(0)
    expect(usedNow()).toBe(0.6)
    expect(h.gets).toHaveLength(3)
    stop()
  })

  it("keeps a Member's 403 as a status, not a guess from the text (B25)", async () => {
    h.fail = new DaemonHttpError('Usage is visible to the server Owner and Admins', 403, 'admin_only')
    const stop = watchUsageLimits(target)
    await vi.advanceTimersByTimeAsync(0)
    const entry = useUsageLimitsStore.getState().entries.srv
    expect(entry).toEqual({ doc: null, error: 'Usage is visible to the server Owner and Admins', status: 403 })
    stop()
  })
})

describe('refresh', () => {
  it('POSTs the login to re-check and keeps the answer', async () => {
    h.body = doc(0.7)
    await useUsageLimitsStore.getState().refresh(target, { accountId: 'acc_a' })
    expect(h.posts).toEqual([{ route: 'usage/limits/refresh', body: { accountId: 'acc_a' } }])
    expect(usedNow()).toBe(0.7)
  })

  it('rethrows a failure and records it', async () => {
    h.fail = new DaemonHttpError('Unknown account', 404, null)
    await expect(useUsageLimitsStore.getState().refresh(target, { accountId: 'x' })).rejects.toThrow('Unknown account')
    expect(useUsageLimitsStore.getState().entries.srv).toEqual({ doc: null, error: 'Unknown account', status: 404 })
  })
})
