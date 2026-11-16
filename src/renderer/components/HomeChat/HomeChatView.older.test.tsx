// @vitest-environment jsdom
//
// Home chat: "Earlier messages" keeps the message being read where it is
// (usePrependHold), including when the reader scrolls while the page is
// on its way. The view used to set scrollTop after the read from the
// height and scrollTop it kept when the read started, undoing that
// scrolling. The fake layout is the chats' (test-utils/chat-layout).
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  /** The older page's answer, when the test says. */
  older: null as null | (() => void),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: () => () => undefined,
}))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import HomeChatView from './HomeChatView'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import type { HomeChatTarget } from '@/lib/home-chat/address'
import {
  chatFrames,
  chatLayout,
  installChatLayout,
  readerScroll,
  rowOnScreen,
  rowTop,
  stubChatFrames,
  uninstallChatLayout,
} from '@/test-utils/chat-layout'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CHANNEL: HomeChatTarget = { kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' }

function m(n: number): Record<string, unknown> {
  return { id: String(n), channel_id: '600', author_id: n % 2 ? '100' : '1', body: `b${n}`, seq: n, created_at: '2026-11-14T10:00:00Z', reactions: [] }
}
/** 151–200 first, then 126–150. */
const NEWEST = Array.from({ length: 50 }, (_, i) => m(151 + i))
const OLDER = Array.from({ length: 25 }, (_, i) => m(126 + i))

beforeAll(() => installChatLayout({ view: 900, rowPx: (key) => 30 + (Number(key) % 4) * 5 }))
afterAll(uninstallChatLayout)

beforeEach(() => {
  resetHomeChatForTests()
  resetHomeChatViewCacheForTests()
  stubChatFrames()
  h.older = null
  h.get.mockReset()
  h.post.mockReset()
  h.get.mockImplementation(async (_s, route, p) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return { ok: true, me: { ext: 'k2:connect:me', memberId: '1' }, people: [] }
      case 'home-chat/channels/members':
        return { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: true, creator: 'k2:connect:me', people: [], agents: [] }
      case 'home-chat/messages':
        if (!p?.before) return { ok: true, conv: '600', page: { items: NEWEST, has_more: true } }
        return new Promise((resolve) => {
          h.older = () => resolve({ ok: true, conv: '600', page: { items: OLDER, has_more: true } })
        })
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
  h.post.mockImplementation(async () => ({ ok: true }))
})

afterEach(() => {
  cleanup()
  resetHomeChatForTests()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('Home chat: Earlier messages', () => {
  it('scrolling while the page loads is kept, and the row being read stays put when it lands', async () => {
    render(<HomeChatView target={CHANNEL} />)
    await screen.findByText('b200')
    vi.useFakeTimers()
    chatFrames(5)
    // Up to the first loaded message (memory first), then Earlier messages.
    for (let i = 0; i < 200 && (chatLayout.scrollTop > 0 || !screen.queryByTestId('home-chat-load-older')); i++) readerScroll(-300)
    expect(chatLayout.scrollTop).toBe(0)
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-load-older'))
    })
    expect(h.older).not.toBeNull()
    // The reader reads on down while the page is on its way.
    readerScroll(300)
    chatFrames(3)
    const before = rowOnScreen()
    expect(before).not.toBeNull()
    await act(async () => {
      h.older?.()
    })
    chatFrames(3)
    expect(rowTop(before?.key ?? '')).toBe(before?.top)
    expect(document.querySelector('[data-thread-column] > [data-zen-message="126"]')).not.toBeNull()
  })
})
