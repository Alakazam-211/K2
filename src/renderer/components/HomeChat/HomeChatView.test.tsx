// @vitest-environment jsdom
// The Home chat view (prd-home-people-channels-v1 §8–§11; slice S5).
//
//   1. It draws from `/cli/home-chat/*` only: never a Thread route, never
//      the redb Thread store (agent Threads stay on redb, §2).
//   2. Reactions: a chip tap toggles the viewer's own, on the wire too.
//   3. Reply: the hover row's Reply puts "Replying to X: first line" over
//      the box; the send carries `replyTo`; × clears it.
//   4. The quote strip jumps to the original, reading back a page when it
//      isn't loaded yet.
//   5. The typing bar names who is typing (not agent activity).
//   6. `@name` is plain text (no mentions in v1).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  frames: null as null | ((f: unknown) => void),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: (_scope: unknown, fn: (f: unknown) => void) => {
    h.frames = fn
    return () => {
      h.frames = null
    }
  },
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import HomeChatView from './HomeChatView'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import type { HomeChatTarget } from '@/lib/home-chat/address'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CHANNEL: HomeChatTarget = { kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' }
const ME = '1'
const ALEX = '100'

function m(id: string, author: string, body: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, channel_id: '600', author_id: author, body, seq: Number(id), created_at: '2026-11-14T10:00:00Z', reactions: [], ...extra }
}

const NEWEST = [
  m('20', ALEX, 'hi @alex, plain text'),
  m('21', ME, 'mine', { reactions: [{ emoji: { kind: 'unicode', value: '👍' }, count: 2, me: true }] }),
  m('22', ALEX, 'second\nline two', { reference: { kind: 'reply', message_id: '5', channel_id: '600' } }),
  m('23', ALEX, 'answers 20', { reference: { kind: 'reply', message_id: '20', channel_id: '600' } }),
]
const OLDER = [m('5', ME, 'the original question'), m('6', ALEX, 'six')]

function routes(): string[] {
  return [...h.get.mock.calls.map((c) => c[1]), ...h.post.mock.calls.map((c) => c[1])]
}

beforeEach(() => {
  resetHomeChatForTests()
  resetHomeChatViewCacheForTests()
  h.get.mockReset()
  h.post.mockReset()
  h.get.mockImplementation(async (_s, route, p) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return {
          ok: true,
          me: { ext: 'k2:connect:me', memberId: ME },
          people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: ALEX }],
        }
      case 'home-chat/channels/members':
        return { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: false, creator: 'k2:connect:me', people: [], agents: [] }
      case 'home-chat/messages':
        return { ok: true, conv: '600', page: p?.before ? { items: OLDER, has_more: false } : { items: NEWEST, has_more: true } }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
  h.post.mockImplementation(async (_s, route, b) => {
    if (route === 'home-chat/send') {
      const body = b as { text: string; replyTo?: string }
      return { ok: true, message: m('30', ME, body.text, body.replyTo ? { reference: { kind: 'reply', message_id: body.replyTo, channel_id: '600' } } : {}) }
    }
    return { ok: true }
  })
})

afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

async function open(): Promise<void> {
  render(<HomeChatView target={CHANNEL} />)
  await screen.findByText('mine')
}

describe('Home chat view', () => {
  it('draws from the Home chat routes only, never a Thread route', async () => {
    await open()
    expect(screen.getByText('hi @alex, plain text')).toBeTruthy()
    const seen = routes()
    expect(seen.length).toBeGreaterThan(0)
    for (const r of seen) expect(r.startsWith('home-chat/')).toBe(true)
    expect(seen.some((r) => r.includes('thread'))).toBe(false)
    // Opening tells the daemon the chat is on screen (typing, join on open).
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/view', { conv: '600', open: true })
  })

  it('`@name` is plain text: no mention mark or link', async () => {
    await open()
    const el = screen.getByText('hi @alex, plain text')
    expect(el.closest('a')).toBeNull()
    expect(document.querySelector('[data-mention]')).toBeNull()
  })

  it('a chip tap toggles the viewer’s own reaction, on the wire too', async () => {
    await open()
    const chip = document.querySelector('[data-zen-message="21"] [data-testid="home-chat-reaction"]') as HTMLButtonElement
    expect(chip.getAttribute('data-me')).toBe('')
    expect(chip.textContent).toContain('2')
    await act(async () => {
      fireEvent.click(chip)
    })
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/react', { conv: '600', msg: '21', emoji: '👍', on: false })
    const after = document.querySelector('[data-zen-message="21"] [data-testid="home-chat-reaction"]') as HTMLButtonElement
    expect(after.getAttribute('data-me')).toBeNull()
    expect(after.textContent).toContain('1')
  })

  it('Reply: the bar names who and the first line; the send carries replyTo; × clears it', async () => {
    await open()
    const row = document.querySelector('[data-zen-message="22"]') as HTMLElement
    await act(async () => {
      fireEvent.click(row.querySelector('[data-testid="home-chat-reply"]') as HTMLElement)
    })
    const chip = await screen.findByTestId('thread-reply-chip')
    expect(chip.textContent).toContain('Replying to Alex: second')
    expect(chip.textContent).not.toContain('line two')
    const input = screen.getByTestId('home-chat-input') as HTMLTextAreaElement
    await act(async () => {
      fireEvent.change(input, { target: { value: 'agreed' } })
    })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
    const send = h.post.mock.calls.find((c) => c[1] === 'home-chat/send')
    expect(send?.[2]).toMatchObject({ conv: '600', text: 'agreed', replyTo: '22' })
    expect(typeof (send?.[2] as { nonce?: unknown }).nonce).toBe('string')
    await waitFor(() => expect(screen.queryByTestId('thread-reply-chip')).toBeNull())
    // Reply, then ×.
    await act(async () => {
      fireEvent.click(row.querySelector('[data-testid="home-chat-reply"]') as HTMLElement)
    })
    await act(async () => {
      fireEvent.click(await screen.findByTestId('thread-reply-cancel'))
    })
    expect(screen.queryByTestId('thread-reply-chip')).toBeNull()
  })

  it('the quote strip jumps to a loaded original', async () => {
    await open()
    const strip = document.querySelector('[data-zen-message="23"] [data-reply-jump]') as HTMLElement
    expect(strip.textContent).toContain('Alex: hi @alex')
    await act(async () => {
      fireEvent.click(strip)
    })
    await waitFor(() => expect(document.querySelector('[data-home-chat-message][data-zen-message="20"]')?.hasAttribute('data-flash')).toBe(true))
  })

  it('the quote strip reads back a page for an original not loaded yet', async () => {
    await open()
    const strip = document.querySelector('[data-zen-message="22"] [data-reply-jump]') as HTMLElement
    expect(strip.textContent).toContain('Original not loaded')
    await act(async () => {
      fireEvent.click(strip)
    })
    expect(h.get).toHaveBeenCalledWith(expect.anything(), 'home-chat/messages', expect.objectContaining({ before: '20' }))
    await waitFor(() => expect(document.querySelector('[data-home-chat-message][data-zen-message="5"]')?.hasAttribute('data-flash')).toBe(true))
    // The strip now names the original.
    expect((document.querySelector('[data-zen-message="22"] [data-reply-jump]') as HTMLElement).textContent).toContain('You: the original question')
  })

  it('the typing bar names who is typing', async () => {
    await open()
    await waitFor(() => expect(h.frames).not.toBeNull())
    await act(async () => {
      h.frames?.({ type: 'typing', conv: '600', member: ALEX })
    })
    await waitFor(() => expect(screen.getByTestId('home-chat-typing').textContent).toBe('Alex is typing'))
    expect(screen.getByTestId('home-chat-typing').hasAttribute('data-on')).toBe(true)
    // A live message from them clears it.
    await act(async () => {
      h.frames?.({ type: 'message', conv: '600', message: m('24', ALEX, 'done typing') })
    })
    await screen.findByText('done typing')
    // The line goes quiet; its slot stays (no height), so the box never moves.
    expect(screen.getByTestId('home-chat-typing').hasAttribute('data-on')).toBe(false)
    expect(screen.getByTestId('home-chat-typing').textContent).toBe('')
  })
})
