// @vitest-environment jsdom
//
// Members read usage from `account.usage` (they can't read
// `/cli/usage/limits`), so the accounts list follows `subscription_usage_changed`
// too, debounced 2 s and only while the page is visible (U28).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  usageHandlers: [] as Array<(ev: unknown) => void>,
  acctHandlers: [] as Array<(tool: string | null) => void>,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly(vi.fn()),
  }
})

vi.mock('@/stores/session-events', () => ({
  onLlmAccountsChanged: (_scope: unknown, fn: (tool: string | null) => void) => {
    h.acctHandlers.push(fn)
    return () => {
      h.acctHandlers.splice(h.acctHandlers.indexOf(fn), 1)
    }
  },
  onSubscriptionUsageChanged: (_scope: unknown, fn: (ev: unknown) => void) => {
    h.usageHandlers.push(fn)
    return () => {
      h.usageHandlers.splice(h.usageHandlers.indexOf(fn), 1)
    }
  },
}))

import { primaryScope } from '@/kessel/server-scope'
import { resetLlmAccountsForTests, USAGE_EVENT_DEBOUNCE_MS, watchLlmAccounts } from './llm-accounts'

const emitUsage = () => h.usageHandlers.forEach((fn) => fn({ kind: 'subscription_usage_changed' }))

describe('watchLlmAccounts and usage events', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    h.daemonCliGet.mockReset()
    h.daemonCliGet.mockResolvedValue({ tools: [] })
    resetLlmAccountsForTests()
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' })
  })

  afterEach(() => {
    vi.useRealTimers()
    h.usageHandlers.length = 0
    h.acctHandlers.length = 0
  })

  it('re-reads the list once, debounced, after usage events', async () => {
    const stop = watchLlmAccounts(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)

    emitUsage()
    emitUsage()
    emitUsage()
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(USAGE_EVENT_DEBOUNCE_MS)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
    stop()
  })

  it('does not re-read while the page is hidden', async () => {
    const stop = watchLlmAccounts(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' })
    emitUsage()
    await vi.advanceTimersByTimeAsync(USAGE_EVENT_DEBOUNCE_MS * 2)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    stop()
  })

  it('stops following when the last watcher leaves', async () => {
    const stop = watchLlmAccounts(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    emitUsage()
    stop()
    await vi.advanceTimersByTimeAsync(USAGE_EVENT_DEBOUNCE_MS * 2)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(h.usageHandlers).toHaveLength(0)
  })
})
