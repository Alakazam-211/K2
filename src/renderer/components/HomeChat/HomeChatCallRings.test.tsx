// @vitest-environment jsdom
// Home DM call rings for the callee (prd-home-people-channels-v1 §11a.3).
// The bug: the callee's window held its Home chat events socket only while
// a chat row or the chat view was on screen, so a callee on any other page
// never heard `call_ring` and never saw a card. The fix holds the socket for
// every server a ring can come from, from a layer beside the App:
//
//   1. With nothing but the ring layer mounted (no chat row, no chat view:
//      "any screen"), `call_ring` shows the card; Answer joins the DM call
//      (`voice.join` with the room `dm:<conv>`); `ringing:false` clears it.
//   2. Decline posts `voice/call/decline {dm, callId}`.
//   3. The servers watched: the window's own, every server that says Home
//      chat, every chat row on ANY Home, every Voice & Chat only server.
//   4. A server whose chat isn't on yet is asked again, and its socket
//      starts once it says on (no reload).
//   5. The events socket never gives up: after a few dead tries it slows to
//      one a minute.
//   6. The ring layer looks for the chat header every 500 ms (a ring shows on
//      every page, so the header comes and goes under it) and sits under it,
//      over it in a narrow chat, or under the top bar with no header.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  frames: new Map<string, (f: unknown) => void>(),
  join: vi.fn(async (_hostKey: string, _room: unknown): Promise<void> => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: (scope: { hostKey: string }, fn: (f: unknown) => void) => {
    h.frames.set(scope.hostKey, fn)
    return () => {
      h.frames.delete(scope.hostKey)
    }
  },
}))
vi.mock('@/stores/voice', () => ({
  voice: {
    join: (hostKey: string, room: unknown) => h.join(hostKey, room),
    leave: vi.fn(async () => undefined),
    setMuted: vi.fn(),
    store: { getState: () => ({ call: null }) },
  },
  useVoice: <T,>(sel: (s: { call: null }) => T): T => sel({ call: null }),
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { HomeChatCallRings, ringWatchHosts } from './HomeChatCallControls'
import { resetHomeChatForTests, retainHomeChatHost, useHomeChatStore, HOME_CHAT_STATUS_RETRY_MS } from '@/stores/home-chat'
import { useHomeChatCallsStore } from '@/stores/home-chat-calls'
import {
  HOME_CHAT_EVENTS_RETRY_MS,
  HOME_CHAT_EVENTS_SLOW_RETRY_MS,
  parseHomeChatFrame,
  type HomeChatEventsDeps,
} from '@/lib/home-chat/events-socket'
import { HOME_CHAT_FEATURE } from '@/lib/home-chat/api'
import type { ServerScope } from '@/kessel/server-scope'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CALLER = '42'
const EXPIRES = '2099-01-01T00:00:00Z'

function statusOn(): void {
  h.get.mockImplementation(async (_s, route) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return {
          ok: true,
          me: { ext: 'k2:connect:me', memberId: '41' },
          people: [{ ext: 'k2:connect:alex', username: 'alex', displayName: 'Alex Doe', ready: true, memberId: CALLER }],
        }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
}

/** Send `raw` as the daemon would, through the real frame parser. */
function frame(hostKey: string, raw: Record<string, unknown>): void {
  const f = parseHomeChatFrame(JSON.stringify(raw))
  if (!f) throw new Error(`the parser dropped ${JSON.stringify(raw)}`)
  const fn = h.frames.get(hostKey)
  if (!fn) throw new Error(`no events socket for ${hostKey}`)
  act(() => fn(f))
}

function ring(ringing: boolean, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: 'call_ring',
    conv: '900',
    d: { call_id: '77', channel_id: '900', started_by: CALLER, ringing, silent: false, expires_at: EXPIRES, ...extra },
  }
}

beforeEach(() => {
  resetHomeChatForTests()
  h.get.mockReset()
  h.post.mockReset()
  h.post.mockImplementation(async () => ({ ok: true }))
  h.join.mockReset()
  h.join.mockImplementation(async () => undefined)
  h.frames.clear()
  statusOn()
})

afterEach(() => {
  cleanup()
  resetHomeChatForTests()
  vi.useRealTimers()
})

describe('the incoming call card for the callee', () => {
  it('shows on call_ring with no chat row or chat view mounted; Answer joins; ringing:false clears it', async () => {
    render(<HomeChatCallRings />)
    // The layer alone opens this window's own server's socket.
    await waitFor(() => expect(h.frames.has('local')).toBe(true))
    expect(screen.queryByTestId('home-chat-ring-card')).toBeNull()

    frame('local', ring(true))
    const card = await screen.findByTestId('home-chat-ring-card')
    expect(card.getAttribute('role')).toBe('alertdialog')
    expect(screen.getByTestId('home-chat-ring-accept').textContent).toBe('Answer')
    expect(screen.getByTestId('home-chat-ring-decline').textContent).toBe('Decline')
    // The caller's name comes from the server's people list.
    await waitFor(() => expect(screen.getByTestId('home-chat-ring-name').textContent).toBe('Alex Doe'))

    fireEvent.click(screen.getByTestId('home-chat-ring-accept'))
    await waitFor(() => expect(h.join).toHaveBeenCalledTimes(1))
    const [hostKey, room] = h.join.mock.calls[0]
    expect(hostKey).toBe('local')
    expect((room as { id: string }).id).toBe('dm:900')

    // Answered (here or on another device): the daemon stops the ring and
    // the card goes.
    frame('local', ring(false, { reason: 'accepted' }))
    await waitFor(() => expect(screen.queryByTestId('home-chat-ring-card')).toBeNull())
    expect(useHomeChatCallsStore.getState().rings).toEqual({})
  })

  it('Decline posts voice/call/decline; a ring stopped elsewhere clears the card', async () => {
    render(<HomeChatCallRings />)
    await waitFor(() => expect(h.frames.has('local')).toBe(true))
    frame('local', ring(true))
    await screen.findByTestId('home-chat-ring-card')
    fireEvent.click(screen.getByTestId('home-chat-ring-decline'))
    await waitFor(() => expect(h.post).toHaveBeenCalledWith(expect.anything(), 'voice/call/decline', { dm: '900', callId: '77' }))
    expect(h.join).not.toHaveBeenCalled()
    frame('local', ring(false, { reason: 'declined' }))
    await waitFor(() => expect(screen.queryByTestId('home-chat-ring-card')).toBeNull())

    // Another call rings, then the caller cancels: gone without a gesture here.
    frame('local', ring(true, { call_id: '78' }))
    await screen.findByTestId('home-chat-ring-card')
    frame('local', ring(false, { call_id: '78', reason: 'cancelled' }))
    await waitFor(() => expect(screen.queryByTestId('home-chat-ring-card')).toBeNull())
  })

  it('a window opened mid-ring shows the ring from hello', async () => {
    render(<HomeChatCallRings />)
    await waitFor(() => expect(h.frames.has('local')).toBe(true))
    frame('local', {
      type: 'hello',
      chat: 'live',
      calls: [{ callId: '77', conv: '900', startedBy: CALLER, ringing: true, silent: false, expiresAt: EXPIRES, call: null }],
    })
    await screen.findByTestId('home-chat-ring-card')
  })
})

describe('the ring layer follows the chat header', () => {
  /** A chat header on screen at `rect` (CSS px, as the app zoom leaves it). */
  function header(rect: { top: number; left: number; width: number; height: number }): HTMLElement {
    const el = document.createElement('div')
    el.dataset.testid = 'home-chat-header'
    el.getBoundingClientRect = () => ({ ...rect, right: rect.left + rect.width, bottom: rect.top + rect.height, x: rect.left, y: rect.top, toJSON: () => ({}) })
    document.body.appendChild(el)
    return el
  }

  it('looks again every 500 ms: a header that appears or moves is followed, and without one the layer is under the top bar', async () => {
    vi.useFakeTimers()
    render(<HomeChatCallRings />)
    await vi.advanceTimersByTimeAsync(0)
    expect(h.frames.has('local')).toBe(true)
    frame('local', ring(true))
    const layer = screen.getByTestId('home-chat-call-layer')
    // No chat header on this page: just under the top bar.
    expect(layer.dataset.place).toBe('float')
    expect(layer.style.top).toBe('56px')

    // A chat opens under the ring: nothing moves until the layer looks again.
    const el = header({ top: 40, left: 300, width: 900, height: 48 })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(499)
    })
    expect(layer.style.top).toBe('56px')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
    // Under its header (bottom 88, plus the 8px gap).
    expect(layer.style.top).toBe('96px')

    // The header moves down (a banner above it): followed within 500 ms.
    el.getBoundingClientRect = () => ({ top: 100, left: 300, width: 900, height: 48, right: 1200, bottom: 148, x: 300, y: 100, toJSON: () => ({}) })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500)
    })
    expect(layer.style.top).toBe('156px')

    // The chat narrows: the card docks over the header itself.
    el.getBoundingClientRect = () => ({ top: 100, left: 300, width: 500, height: 48, right: 800, bottom: 148, x: 300, y: 100, toJSON: () => ({}) })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500)
    })
    expect(layer.dataset.place).toBe('dock')
    expect(layer.style.top).toBe('100px')
    expect(layer.style.left).toBe('300px')
    expect(layer.style.width).toBe('500px')

    // The chat closes: back under the top bar.
    el.remove()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500)
    })
    expect(layer.dataset.place).toBe('float')
    expect(layer.style.top).toBe('56px')
  })
})

describe('which servers a ring can come from', () => {
  it('the window’s own, Home chat servers, chat rows on any Home, Voice & Chat only servers', () => {
    const pool = {
      'chat.k2.dev': { boot: { features: ['x', HOME_CHAT_FEATURE] } },
      'old.k2.dev': { boot: { features: ['x'] } },
      'down.k2.dev': { boot: null },
    }
    const homes = [
      { rows: [{ address: 'agent::local' }] },
      // A Home that isn't on screen still counts.
      { rows: [{ kind: 'person', address: 'k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e::rows.k2.dev' }, { kind: 'channel', address: '901::chan.k2.dev' }] },
    ]
    const saved = [
      { hostname: 'z3.k2.dev', port: 443, secure: true, access: 'voice_chat' },
      { hostname: 'member.k2.dev', port: 443, secure: true },
    ]
    expect(ringWatchHosts('local', pool, homes, saved)).toEqual(['chan.k2.dev', 'chat.k2.dev', 'local', 'rows.k2.dev', 'z3.k2.dev'])
  })

  it('a server whose chat is off is asked again, and its socket starts once it says on', async () => {
    vi.useFakeTimers()
    let on = false
    h.get.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/status') return { ok: true, on, reason: on ? null : 'off', flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    const release = retainHomeChatHost('local')
    await vi.advanceTimersByTimeAsync(0)
    expect(useHomeChatStore.getState().byHost.local?.status?.on).toBe(false)
    expect(h.frames.has('local')).toBe(false)
    on = true
    await vi.advanceTimersByTimeAsync(HOME_CHAT_STATUS_RETRY_MS[0])
    expect(h.frames.has('local')).toBe(true)
    release()
    expect(h.frames.has('local')).toBe(false)
  })
})

describe('the events socket never gives up', () => {
  it('slows to one try a minute after the dead tries, and keeps trying', async () => {
    const waits: number[] = []
    let next: (() => void) | null = null
    const deps: HomeChatEventsDeps = {
      open: () => Promise.reject(new Error('refused')),
      setTimer: (fn, ms) => {
        waits.push(ms)
        next = fn
        return waits.length
      },
      clearTimer: () => {},
    }
    const scope = { hostKey: 'gone.k2.dev', wsBase: async () => 'ws://gone.k2.dev', creds: async () => ({ token: 't' }) } as unknown as ServerScope
    // The real socket (the module mock above only replaces the default export's use in stores).
    const { subscribeHomeChatEvents } = await vi.importActual<typeof import('@/lib/home-chat/events-socket')>('@/lib/home-chat/events-socket')
    const stop = subscribeHomeChatEvents(scope, () => {}, () => {}, deps)
    for (let i = 0; i < 6; i++) {
      await vi.waitFor(() => expect(waits.length).toBe(i + 1))
      const fn = next as (() => void) | null
      if (!fn) throw new Error('no retry scheduled')
      next = null
      fn()
    }
    await vi.waitFor(() => expect(waits.length).toBe(7))
    expect(waits).toEqual([
      HOME_CHAT_EVENTS_RETRY_MS[0],
      HOME_CHAT_EVENTS_RETRY_MS[1],
      HOME_CHAT_EVENTS_RETRY_MS[2],
      HOME_CHAT_EVENTS_SLOW_RETRY_MS,
      HOME_CHAT_EVENTS_SLOW_RETRY_MS,
      HOME_CHAT_EVENTS_SLOW_RETRY_MS,
      HOME_CHAT_EVENTS_SLOW_RETRY_MS,
    ])
    stop()
  })
})
