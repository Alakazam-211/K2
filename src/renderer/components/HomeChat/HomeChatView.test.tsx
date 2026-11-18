// @vitest-environment jsdom
// The Home chat view (prd-home-people-channels-v1 §8–§11; slice S5).
//
//   1. It draws from `/cli/home-chat/*` only: never a Thread route, never
//      the redb Thread store (agent Threads stay on redb, §2).
//   2. Reactions: a chip tap toggles the viewer's own, on the wire too; a
//      pick shows at once, and one the server refuses goes back quietly.
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
import { clearCallTileRest, setCallTileRest } from '@/stores/call-tile-place'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { threadComposeCapWidth } from '@/components/SessionView/threadTextWidth'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import type { HomeChatTarget } from '@/lib/home-chat/address'
import { rememberPeopleNames, resetKnownNamesForTests } from '@/lib/home-chat/known-names'

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
  it("keeps the agent Thread's text width: the column and the box cap at the same limit and centre", async () => {
    const was = useSettingsStore.getState().threadTextWidth
    useSettingsStore.setState({ threadTextWidth: { limited: true, px: 640 } })
    try {
      await open()
      const column = screen.getByTestId('home-chat-column')
      expect(column.style.maxWidth).toBe('640px')
      expect(column.style.marginInline).toBe('auto')
      const box = screen.getByTestId('home-chat-compose-cap')
      expect(box.style.maxWidth).toBe(threadComposeCapWidth(640))
      expect(box.contains(screen.getByTestId('home-chat-compose'))).toBe(true)
    } finally {
      useSettingsStore.setState({ threadTextWidth: was })
    }
  })

  it('Full width (no limit) leaves the column and the box uncapped', async () => {
    const was = useSettingsStore.getState().threadTextWidth
    useSettingsStore.setState({ threadTextWidth: { limited: false, px: 640 } })
    try {
      await open()
      expect(screen.getByTestId('home-chat-column').style.maxWidth).toBe('')
      expect(screen.getByTestId('home-chat-compose-cap').style.maxWidth).toBe('')
    } finally {
      useSettingsStore.setState({ threadTextWidth: was })
    }
  })

  it("the header's glyph matches the row's: a lock for an invite-only channel", async () => {
    await open()
    await waitFor(() => expect(screen.getByTestId('home-chat-header-glyph').querySelector('[data-channel-glyph]')?.getAttribute('data-channel-glyph')).toBe('lock'))
  })

  it('draws from the Home chat routes only, never a Thread route', async () => {
    await open()
    // The bubble (a reply's quote strip may show the same words).
    expect(screen.getAllByText('hi @alex, plain text')[0].closest('[data-zen-message="20"]')).toBeTruthy()
    const seen = routes()
    expect(seen.length).toBeGreaterThan(0)
    for (const r of seen) expect(r.startsWith('home-chat/')).toBe(true)
    expect(seen.some((r) => r.includes('thread'))).toBe(false)
    // Opening tells the daemon the chat is on screen (typing, join on open).
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/view', { conv: '600', open: true })
  })

  it('`@name` is plain text: no mention mark or link', async () => {
    await open()
    for (const el of screen.getAllByText('hi @alex, plain text')) expect(el.closest('a')).toBeNull()
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

  it('a picked emoji shows at once, before the server answers', async () => {
    let answer: (v: unknown) => void = () => undefined
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/react' ? new Promise((r) => (answer = r)) : { ok: true }))
    await open()
    const row = document.querySelector('[data-zen-message="20"]') as HTMLElement
    await act(async () => {
      fireEvent.click(row.querySelector('[data-testid="home-chat-react"]') as HTMLElement)
    })
    const quick = screen.getByTestId('home-chat-quick-reactions').querySelectorAll('button')
    await act(async () => {
      fireEvent.click(quick[3])
    })
    // The picker has closed, and the chip is there while the send is out.
    expect(screen.queryByTestId('home-chat-reaction-picker')).toBeNull()
    const chip = row.querySelector('[data-testid="home-chat-reaction"]') as HTMLElement
    expect(chip.getAttribute('data-emoji')).toBe(quick[3].getAttribute('data-emoji'))
    expect(chip.hasAttribute('data-me')).toBe(true)
    await act(async () => {
      answer({ ok: true })
    })
  })

  it('a reaction the server refuses goes back, with a quiet note (not the send error)', async () => {
    useToastStore.setState({ toasts: [] })
    h.post.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/react') throw new Error('offline')
      return { ok: true }
    })
    await open()
    const chip = () => document.querySelector('[data-zen-message="21"] [data-testid="home-chat-reaction"]') as HTMLElement
    await act(async () => {
      fireEvent.click(chip())
    })
    await waitFor(() => expect(chip().hasAttribute('data-me')).toBe(true))
    expect(chip().textContent).toContain('2')
    const toasts = useToastStore.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].message).toMatch(/^Couldn't remove your reaction/)
    expect(screen.queryByText(/Not sent/)).toBeNull()
  })

  it('Reply: the bar names who and the first line; the send carries replyTo; × clears it', async () => {
    await open()
    const row = document.querySelector('[data-zen-message="22"]') as HTMLElement
    await act(async () => {
      fireEvent.click(row.querySelector('[data-testid="home-chat-reply"]') as HTMLElement)
    })
    const chip = await screen.findByTestId('thread-reply-chip')
    expect(chip.textContent).toContain('Replying to Alex')
    expect(chip.querySelector('[data-reply-chip-text]')?.textContent).toBe('second')
    expect(chip.textContent).not.toContain('line two')
    // The agent Thread's reply bar, on the box's edge: the same component,
    // in the row over the typing line, never a note above the box.
    expect(chip.hasAttribute('data-thread-reply-bar')).toBe(true)
    expect(chip.closest('[data-thread-reply-row]')).toBeTruthy()
    const compose = screen.getByTestId('home-chat-compose')
    const box = compose.querySelector('[data-zen-compose-box]') as HTMLElement
    expect(chip.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Esc in the box cancels it.
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('home-chat-input'), { key: 'Escape' })
    })
    expect(screen.queryByTestId('thread-reply-chip')).toBeNull()
    await act(async () => {
      fireEvent.click(row.querySelector('[data-testid="home-chat-reply"]') as HTMLElement)
    })
    await screen.findByTestId('thread-reply-chip')
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
  it('a DM draws each call as a record line with Call back, updated in place, and has a Call button', async () => {
    const DM: HomeChatTarget = { kind: 'person', hostKey: 'local', ext: 'k2:connect:a', address: 'k2:connect:a::local' }
    const record = (summary: Record<string, unknown>) => m('40', ALEX, '', { call: { call_id: '88', started_by: ALEX, rung_ids: [ME], joined_ids: [ALEX], declined_ids: [], missed_ids: [], ...summary } })
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/dm' ? { ok: true, conv: '600' } : { ok: true }))
    h.get.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/messages') return { ok: true, conv: '600', page: { items: [m('39', ALEX, 'call me'), record({ ended_at: '2026-11-14T10:01:00Z', end_reason: 'no_answer', missed_ids: [ME] })], has_more: false } }
      if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      if (route === 'home-chat/people') return { ok: true, me: { ext: 'k2:connect:me', memberId: ME }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: ALEX }] }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<HomeChatView target={DM} />)
    await screen.findByText('call me')
    const line = await screen.findByTestId('home-chat-call-record')
    expect(line.dataset.kind).toBe('missed')
    expect(screen.getByTestId('home-chat-call-record-text').textContent).toBe('Missed call')
    expect(screen.getByTestId('home-chat-call-back')).toBeTruthy()
    // Not a bubble: no reaction or reply row on it.
    expect(line.closest('[data-zen-message="40"]')?.querySelector('[data-testid="home-chat-hover-row"]')).toBeFalsy()
    await waitFor(() => expect((screen.getByTestId('home-chat-call-button') as HTMLButtonElement).disabled).toBe(false))
    await waitFor(() => expect(h.frames).not.toBeNull())
    await act(async () => {
      h.frames?.({ type: 'message_updated', conv: '600', message: record({ ended_at: '2026-11-14T10:02:00Z', end_reason: 'empty', joined_ids: [ALEX, ME], duration_ms: 5_000 }) })
    })
    await waitFor(() => expect(screen.getByTestId('home-chat-call-record-text').textContent).toBe('Call · 0:05'))
  })

  it('a DM draws a recording card with Show transcript; the lines open beside it, never as bubbles (prd-home-call-recording-v1 §2, HR-10)', async () => {
    const DM: HomeChatTarget = { kind: 'person', hostKey: 'local', ext: 'k2:connect:a', address: 'k2:connect:a::local' }
    const card = m('50', ALEX, '', {
      recording: {
        recording_id: '42',
        started_by: ALEX,
        auto: false,
        stopped_at: '2026-11-14T10:12:04Z',
        stop_reason: 'stopped',
        duration_ms: 724_000,
        transcript: { state: 'done', turns: 2 },
        audio: { state: 'available', keep_days: 30, mixed: true },
      },
    })
    const line = (id: string, author: string, body: string) =>
      m(id, author, body, { thread_root_id: '50', transcript: { recording_id: '42', speaker_id: author, start_ms: 0, end_ms: 900 } })
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/dm' ? { ok: true, conv: '600' } : { ok: true }))
    h.get.mockImplementation(async (_s, route, p) => {
      if (route === 'home-chat/messages')
        // A transcript line in the page (a thread reply also sent to the channel) never draws.
        return { ok: true, conv: '600', page: { items: [m('49', ALEX, 'before the call'), card, line('51', ALEX, 'in the page')], has_more: false } }
      if (route === 'home-chat/replies') {
        expect(p).toEqual({ conv: '600', id: '50' })
        return { ok: true, conv: '600', id: '50', page: { items: [line('52', ME, 'and me'), line('51', ALEX, 'hello from Alex')], has_more: false } }
      }
      if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      if (route === 'home-chat/people') return { ok: true, me: { ext: 'k2:connect:me', memberId: ME }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: ALEX }] }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<HomeChatView target={DM} />)
    await screen.findByText('before the call')
    const text = await screen.findByTestId('home-chat-recording-card-title')
    await waitFor(() => expect(text.textContent).toBe('Recording · 12:04'))
    expect(screen.getByTestId('home-chat-recording-card-detail').textContent).toBe('Started by Alex · audio kept 30 days')
    expect(screen.getByTestId('home-chat-recording-card').getAttribute('data-state')).toBe('ready')
    // A stopped recording: no chip in the header.
    expect(screen.queryByTestId('home-chat-header-recording')).toBeNull()
    expect(screen.queryByText('in the page')).toBeNull()
    expect(screen.getByTestId('home-chat-recording-card').closest('[data-zen-message="50"]')?.querySelector('[data-testid="home-chat-hover-row"]')).toBeFalsy()

    fireEvent.click(screen.getByTestId('home-chat-show-transcript'))
    const panel = await screen.findByTestId('home-chat-transcript')
    await waitFor(() => expect(panel.querySelectorAll('[data-testid="home-chat-transcript-line"]').length).toBe(2))
    const turns = [...panel.querySelectorAll('[data-testid="home-chat-transcript-turn"]')].map((el) => el.textContent)
    expect(turns.map((t) => t?.replace(/^[A-Z]{1,2}(?=[A-Z][a-z])/, ''))).toEqual(['Alex0:00hello from Alex', 'You0:00and me'])
    expect(screen.getByTestId('home-chat-transcript-heading').textContent).toMatch(/^Transcript · 12:04 · /)
    // A live line goes to the open transcript, never the timeline.
    await waitFor(() => expect(h.frames).not.toBeNull())
    await act(async () => {
      h.frames?.({ type: 'message', conv: '600', message: line('53', ALEX, 'a live line') })
    })
    await waitFor(() => expect(panel.querySelectorAll('[data-testid="home-chat-transcript-line"]').length).toBe(3))
    expect(document.querySelector('[data-zen-message="53"]')).toBeNull()
    for (const el of screen.getAllByTestId('home-chat-transcript-line')) expect(el.closest('[data-thread-column]')).toBeNull()
    fireEvent.click(screen.getByTestId('home-chat-transcript-close'))
    expect(screen.queryByTestId('home-chat-transcript')).toBeNull()
    // Never audio: no player anywhere.
    expect(document.querySelector('audio')).toBeNull()
  })
})

describe('a chat that fails to open', () => {
  it('says why at full contrast, in sentence case, and Try again reads it again', async () => {
    const ok = h.get.getMockImplementation()!
    let fail = true
    h.get.mockImplementation(async (s, route, p) => {
      if (route === 'home-chat/messages' && fail) throw new Error('sew is not answering')
      return ok(s, route, p)
    })
    render(<HomeChatView target={CHANNEL} />)
    const state = await screen.findByTestId('home-chat-error')
    const reason = screen.getByTestId('home-chat-error-reason')
    expect(reason.textContent).toBe('Sew is not answering.')
    // No fade on the reason or anything round it inside the state.
    for (let el: HTMLElement | null = reason; el && el !== state.parentElement; el = el.parentElement) {
      expect(el.className).not.toMatch(/opacity-/)
      expect(el.style.opacity === '' || el.style.opacity === '1').toBe(true)
    }
    fail = false
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await screen.findByText('mine')
    expect(screen.queryByTestId('home-chat-error')).toBeNull()
  })
})

describe('a DM whose person has left', () => {
  const GONE: HomeChatTarget = { kind: 'person', hostKey: 'local', ext: 'k2:connect:gone', address: 'k2:connect:gone::local' }
  function serve(): void {
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/dm' ? { ok: true, conv: '600' } : { ok: true }))
    h.get.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/messages') return { ok: true, conv: '600', page: { items: [m('50', '300', 'bye for now')], has_more: false } }
      if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      if (route === 'home-chat/people') return { ok: true, me: { ext: 'k2:connect:me', memberId: ME }, people: [] }
      throw new Error(`unexpected GET ${route}`)
    })
  }

  it('is headed by their last known name', async () => {
    resetKnownNamesForTests()
    rememberPeopleNames('local', {
      me: { ext: 'k2:connect:me', memberId: ME },
      people: [{ ext: 'k2:connect:gone', username: 'ana', displayName: 'Ana Ruiz', picture: null, role: null, ready: true, memberId: '300' }],
    })
    serve()
    render(<HomeChatView target={GONE} />)
    await screen.findByText('bye for now')
    await screen.findByTestId('home-chat-readonly')
    expect(screen.getByTestId('home-chat-title').textContent).toBe('Ana Ruiz')
  })

  it('is headed "Former member" when K2 never saw their name', async () => {
    resetKnownNamesForTests()
    serve()
    render(<HomeChatView target={GONE} />)
    await screen.findByTestId('home-chat-readonly')
    expect(screen.getByTestId('home-chat-title').textContent).toBe('Former member')
    expect(document.body.textContent).not.toMatch(/this person|Someone/)
  })
})

// The box grows (the reply bar, the typing line, a longer draft) and the
// list above it gets shorter. When the list was at its newest message, it
// keeps that message in view instead of letting the box cover it; when you
// had scrolled up to read, it leaves you where you were.
describe('the newest message stays in view when the box grows', () => {
  const observed = new Map<Element, Array<() => void>>()
  const RealRO = globalThis.ResizeObserver
  beforeEach(() => {
    observed.clear()
    globalThis.ResizeObserver = class {
      cb: () => void
      constructor(cb: () => void) {
        this.cb = cb
      }
      observe(el: Element): void {
        observed.set(el, [...(observed.get(el) ?? []), this.cb])
      }
      unobserve(): void {}
      disconnect(): void {}
    } as unknown as typeof ResizeObserver
  })
  afterEach(() => {
    globalThis.ResizeObserver = RealRO
  })

  function sized(el: HTMLElement, box: { clientHeight: number; scrollHeight: number }): void {
    Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => box.clientHeight })
    Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => box.scrollHeight })
  }

  it('follows the bottom when the list was at it', async () => {
    await open()
    const list = document.querySelector<HTMLElement>('[data-zen-conversation-body]')!
    const box = { clientHeight: 600, scrollHeight: 2000 }
    sized(list, box)
    list.scrollTop = 1400
    fireEvent.scroll(list)
    expect(observed.has(list)).toBe(true)
    // The reply bar appears: the list is 60px shorter.
    box.clientHeight = 540
    act(() => observed.get(list)!.forEach((cb) => cb()))
    expect(list.scrollTop).toBe(2000)
  })

  it('a list that fit (nothing to scroll) keeps its newest message when the call stage takes its space', async () => {
    await open()
    const list = document.querySelector<HTMLElement>('[data-zen-conversation-body]')!
    const box = { clientHeight: 757, scrollHeight: 2000 }
    sized(list, box)
    // Scrolled up earlier (a jump to an original), so not following.
    list.scrollTop = 0
    fireEvent.scroll(list)
    // Now everything fits: no scroll event can say the list is at its end.
    box.scrollHeight = 757
    act(() => observed.get(list)!.forEach((cb) => cb()))
    // The stage opens above: the list is 405px shorter.
    box.clientHeight = 352
    box.scrollHeight = 704
    act(() => observed.get(list)!.forEach((cb) => cb()))
    expect(list.scrollTop).toBe(704)
  })

  // The floating call tile in a bottom corner of a narrow window, over the
  // message column: the list makes room under its newest message, so the
  // tile never covers it.
  describe('the floating call tile never covers the newest message', () => {
    afterEach(() => act(() => clearCallTileRest()))

    /** An 800 x 800 window: the list from under the header (100) to the box
     *  (660), the column across most of it. */
    function laidOut(): { list: HTMLElement; box: { clientHeight: number; scrollHeight: number } } {
      const list = document.querySelector<HTMLElement>('[data-zen-conversation-body]')!
      const column = list.querySelector<HTMLElement>('[data-thread-column]')!
      const rect = (r: { left: number; top: number; width: number; height: number }) => () =>
        ({ ...r, right: r.left + r.width, bottom: r.top + r.height, x: r.left, y: r.top, toJSON: () => ({}) }) as DOMRect
      list.getBoundingClientRect = rect({ left: 0, top: 100, width: 800, height: 560 })
      column.getBoundingClientRect = rect({ left: 40, top: 100, width: 720, height: 560 })
      const box = { clientHeight: 560, scrollHeight: 2000 }
      sized(list, box)
      return { list, box }
    }
    const TILE = { left: 468, top: 392, width: 320, height: 260, bottom: true }
    const intersects = (a: { top: number; bottom: number; left: number; right: number }, b: typeof a): boolean =>
      a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom

    it('a list at its newest message gets the room under it and stays at it: the newest message clears the tile', async () => {
      await open()
      const { list, box } = laidOut()
      list.scrollTop = 1440
      fireEvent.scroll(list)
      const inset = 660 - TILE.top + 8
      // The list is taller by the room it makes (as the browser measures it
      // once the room is there), and it stays at its end.
      box.scrollHeight = 2000 + inset - 8
      act(() => setCallTileRest(TILE))
      expect(list.style.paddingBottom).toBe(`${inset}px`)
      expect(list.scrollTop).toBe(box.scrollHeight)
      // Scrolled to its end, the newest message's bottom is the list's bottom
      // less its bottom room: just above the tile, with the gap.
      const newest = { left: 40, right: 760, bottom: 660 - inset, top: 660 - inset - 48 }
      expect(intersects(newest, { left: TILE.left, right: TILE.left + TILE.width, top: TILE.top, bottom: TILE.top + TILE.height })).toBe(false)
      expect(TILE.top - newest.bottom).toBe(8)
    })

    it('a tile in a top corner adds nothing, and the room goes when the tile does', async () => {
      await open()
      const { list } = laidOut()
      act(() => setCallTileRest({ ...TILE, top: 120, bottom: false }))
      expect(list.style.paddingBottom).toBe('8px')
      act(() => setCallTileRest(TILE))
      expect(list.style.paddingBottom).toBe(`${660 - TILE.top + 8}px`)
      act(() => clearCallTileRest())
      expect(list.style.paddingBottom).toBe('8px')
    })

    it('a reader who scrolled up stays where they are', async () => {
      await open()
      const { list } = laidOut()
      list.scrollTop = 300
      fireEvent.scroll(list)
      act(() => setCallTileRest(TILE))
      expect(list.style.paddingBottom).toBe(`${660 - TILE.top + 8}px`)
      expect(list.scrollTop).toBe(300)
    })
  })

  it('leaves a reader who scrolled up where they are', async () => {
    await open()
    const list = document.querySelector<HTMLElement>('[data-zen-conversation-body]')!
    const box = { clientHeight: 600, scrollHeight: 2000 }
    sized(list, box)
    list.scrollTop = 300
    fireEvent.scroll(list)
    box.clientHeight = 540
    act(() => observed.get(list)!.forEach((cb) => cb()))
    expect(list.scrollTop).toBe(300)
  })
})

