// @vitest-environment jsdom
//
// The usage chip re-reads its server's cache (GET, never the refresh POST)
// about once a minute, so the daemon's background probe shows up without a
// click. Hidden pages do not poll; focus or visibility reads once.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ scope: unknown; route: string }>,
  posts: [] as Array<{ scope: unknown; route: string }>,
  body: null as unknown,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (scope: unknown, route: string) => {
    h.gets.push({ scope, route })
    return h.body
  },
  daemonCliPost: async (scope: unknown, route: string) => {
    h.posts.push({ scope, route })
    return h.body
  },
}))

vi.mock('@/stores/connect-host', () => ({ onActiveHostChange: () => () => {} }))

import {
  USAGE_CACHE_POLL_MS,
  resetSubscriptionUsageForTests,
  startUsageCachePoll,
  usageEntryFor,
  type UsageTarget,
} from './subscription-usage'
import { useWindowFocusStore } from './window-focus'

function doc(used: number): unknown {
  return {
    harnesses: [
      {
        harness: 'claude',
        plan: 'Max',
        windows: [{ label: 'Weekly', used, resetsAt: '2099-01-01T00:00:00Z' }],
        checkedAt: new Date().toISOString(),
        status: '',
      },
    ],
  }
}

let visibility: DocumentVisibilityState = 'visible'
function setVisibility(v: DocumentVisibilityState): void {
  visibility = v
  document.dispatchEvent(new Event('visibilitychange'))
}

const target: UsageTarget = { key: 'win', scope: { id: 'win' } as unknown as ServerScope }

function usedOf(key: string): number {
  const entry = usageEntryFor(key)
  if (!entry?.doc) throw new Error(`no doc for ${key}`)
  return entry.doc.harnesses[0].windows[0].used
}

/** Let the in-flight GET land (the store awaits it, then writes). */
async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0)
}

let stop: (() => void) | null = null

beforeEach(() => {
  vi.useFakeTimers()
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
  visibility = 'visible'
  h.gets.length = 0
  h.posts.length = 0
  h.body = doc(0.1)
  useWindowFocusStore.setState({ isFocused: true })
  resetSubscriptionUsageForTests()
})

afterEach(() => {
  stop?.()
  stop = null
  vi.useRealTimers()
})

describe('startUsageCachePoll', () => {
  it('polls about every 60 seconds', () => {
    expect(USAGE_CACHE_POLL_MS).toBe(60_000)
  })

  it('re-reads the cache with a GET on each tick and never POSTs', async () => {
    stop = startUsageCachePoll(target)
    expect(h.gets).toHaveLength(0)

    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS)
    expect(h.gets.map((c) => c.route)).toEqual(['usage/subscriptions'])
    expect(h.gets[0].scope).toBe(target.scope)
    expect(usedOf('win')).toBe(0.1)

    // The daemon's probe moved the cache; the next tick shows it.
    h.body = doc(0.55)
    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS)
    expect(h.gets).toHaveLength(2)
    expect(usedOf('win')).toBe(0.55)
    expect(h.posts).toHaveLength(0)
  })

  it('keeps the parse guard: a bad body becomes an empty doc, never the raw body', async () => {
    stop = startUsageCachePoll(target)
    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS)
    expect(usedOf('win')).toBe(0.1)
    h.body = { harnesses: 'nope', extra: 1 }
    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS)
    const entry = usageEntryFor('win')
    if (!entry) throw new Error('no entry')
    expect(entry.doc).toEqual({ harnesses: [] })
    expect(entry.error).toBe(null)
  })

  it('stops while the page is hidden and reads once when it is shown again', async () => {
    stop = startUsageCachePoll(target)
    setVisibility('hidden')
    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS * 5)
    expect(h.gets).toHaveLength(0)

    setVisibility('visible')
    await settle()
    expect(h.gets).toHaveLength(1)
  })

  it('reads once when this window regains focus', async () => {
    stop = startUsageCachePoll(target)
    useWindowFocusStore.setState({ isFocused: false })
    await settle()
    expect(h.gets).toHaveLength(0)
    useWindowFocusStore.setState({ isFocused: true })
    await settle()
    expect(h.gets).toHaveLength(1)
  })

  it('does nothing after stop', async () => {
    stop = startUsageCachePoll(target)
    stop()
    await vi.advanceTimersByTimeAsync(USAGE_CACHE_POLL_MS * 3)
    setVisibility('hidden')
    setVisibility('visible')
    useWindowFocusStore.setState({ isFocused: false })
    useWindowFocusStore.setState({ isFocused: true })
    await settle()
    expect(h.gets).toHaveLength(0)
  })
})
