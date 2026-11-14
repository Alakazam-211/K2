// @vitest-environment jsdom
// People and channels on Home: the per-server cache (prd-home-people-
// channels-v1 §9, §15.1). Badges come from the daemon's summary: a DM's
// number (Sew's `mention_count`, or `unread_count` once flagged, the daemon
// decides), a channel's dot; a DM is found by the summary's `with`. Typing
// frames name who is typing for 10 s, and their message ends it at once.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
const { get } = vi.hoisted(() => ({ get: vi.fn(async (_s: unknown, _r: string, _p?: unknown): Promise<unknown> => ({})) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: unknown) => get(s, r, p),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import {
  TYPING_CLEAR_MS,
  applyHomeChatFrame,
  homeChatRowBadge,
  refreshHomeChatSummary,
  resetHomeChatForTests,
  typingMembers,
  useHomeChatStore,
} from './home-chat'
import type { HomeChatTarget } from '@/lib/home-chat/address'

const HOST = 'local'
const alex: HomeChatTarget = { kind: 'person', hostKey: HOST, ext: 'k2:connect:a', address: 'k2:connect:a::local' }
const sam: HomeChatTarget = { kind: 'person', hostKey: HOST, ext: 'k2:connect:s', address: 'k2:connect:s::local' }
const design: HomeChatTarget = { kind: 'channel', hostKey: HOST, conv: '600', address: '600::local' }
const quiet: HomeChatTarget = { kind: 'channel', hostKey: HOST, conv: '700', address: '700::local' }

beforeEach(() => {
  resetHomeChatForTests()
  get.mockReset()
})
afterEach(() => resetHomeChatForTests())

describe('Home chat badges (§15.1)', () => {
  it('a DM shows its number, a channel its dot, a read one nothing; a DM is found by `with`', async () => {
    get.mockImplementation(async (_s, route) => {
      expect(route).toBe('home-chat/summary')
      return {
        chat: 'live',
        convs: [
          { conv: '500', kind: 'dm', unread: true, badge: { kind: 'count', n: 3 }, with: 'k2:connect:a' },
          { conv: '600', kind: 'channel', unread: true, badge: { kind: 'dot' } },
          { conv: '700', kind: 'channel', unread: false, badge: { kind: 'none' } },
        ],
      }
    })
    await refreshHomeChatSummary(HOST)
    const host = useHomeChatStore.getState().byHost[HOST]
    expect(host.dms).toEqual({ 'k2:connect:a': '500' })
    expect(homeChatRowBadge(host, alex)).toEqual({ kind: 'count', n: 3 })
    expect(homeChatRowBadge(host, design)).toEqual({ kind: 'dot' })
    expect(homeChatRowBadge(host, quiet)).toEqual({ kind: 'none' })
    // A DM K2 hasn't seen yet: no badge (adding a person never makes the DM).
    expect(homeChatRowBadge(host, sam)).toEqual({ kind: 'none' })
    expect(homeChatRowBadge(undefined, design)).toEqual({ kind: 'none' })
  })

  it('a summary_changed frame reads the summary again', async () => {
    vi.useFakeTimers()
    get.mockResolvedValue({ chat: 'live', convs: [] })
    applyHomeChatFrame(HOST, { type: 'summary_changed', conv: '600' }, Date.now())
    expect(get).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(300)
    expect(get).toHaveBeenCalledWith(expect.anything(), 'home-chat/summary', undefined)
    vi.useRealTimers()
  })
})

describe('typing (§9)', () => {
  it('names who is typing for 10 s; their message clears them at once', () => {
    const now = 1_000_000
    applyHomeChatFrame(HOST, { type: 'typing', conv: '600', member: '100' }, now)
    applyHomeChatFrame(HOST, { type: 'typing', conv: '600', member: '101' }, now)
    const typing = () => useHomeChatStore.getState().typing
    expect(typingMembers(typing(), HOST, '600', now + 1).sort()).toEqual(['100', '101'])
    expect(typingMembers(typing(), HOST, '601', now + 1)).toEqual([])
    applyHomeChatFrame(HOST, { type: 'message', conv: '600', message: { id: '9', channel_id: '600', author_id: '100' } }, now + 2)
    expect(typingMembers(typing(), HOST, '600', now + 3)).toEqual(['101'])
    expect(typingMembers(typing(), HOST, '600', now + TYPING_CLEAR_MS + 1)).toEqual([])
    useHomeChatStore.getState().pruneTyping(now + TYPING_CLEAR_MS + 1)
    expect(typing()).toEqual({})
  })
})
