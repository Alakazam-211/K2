// @vitest-environment jsdom
//
// Thread push §6.4 (S4): the desktop tells the daemon which Thread the
// person is reading, so that Thread's pushes stay quiet. Pinned here:
//   - the pure sender: true at once, repeated every 30 s, false on
//     blur/hide/unmount, re-said on a new socket;
//   - the hook wiring: frames go out on the Thread's own overlay socket,
//     only for a `viewing` surface, only to a `thread-push-v1` server, and
//     follow window focus/visibility and unmount.
import type { ServerScope } from '@/kessel/server-scope'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(daemonCliGet),
    daemonCliPost: primaryOnly(daemonCliPost),
  }
})

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: async (scope: ServerScope) => {
    const { expectPrimaryScope } = await import('@/test-utils/scope')
    expectPrimaryScope(scope)
    return { host: '127.0.0.1', port: 1, token: 't', secure: false }
  },
  daemonWsBase: () => 'ws://test',
}))

import { createViewingSender, viewingFrame, VIEWING_REPEAT_MS } from './threadViewing'
import { useOverlayThread } from './useOverlayThread'
import { primaryScope, noteServerVersion, __resetServerScopesForTests } from '@/kessel/server-scope'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'

const TRUE = viewingFrame(true)
const FALSE = viewingFrame(false)

describe('viewingFrame', () => {
  it('is the overlay socket text frame the PRD names', () => {
    expect(JSON.parse(TRUE)).toEqual({ t: 'viewing', visible: true })
    expect(JSON.parse(FALSE)).toEqual({ t: 'viewing', visible: false })
  })
})

describe('createViewingSender (fake timers)', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function harness(initial: boolean) {
    const sent: string[] = []
    let viewing = initial
    let socketOpen = true
    const sender = createViewingSender({
      send: (f) => {
        if (!socketOpen) return false
        sent.push(f)
        return true
      },
      isViewing: () => viewing,
    })
    return {
      sent,
      sender,
      set: (v: boolean) => {
        viewing = v
      },
      setSocket: (open: boolean) => {
        socketOpen = open
      },
    }
  }

  it('sends true at once on focus, repeats every 30 s, and false on blur', () => {
    const h = harness(true)
    h.sender.update()
    expect(h.sent).toEqual([TRUE])

    vi.advanceTimersByTime(VIEWING_REPEAT_MS - 1)
    expect(h.sent).toEqual([TRUE])
    vi.advanceTimersByTime(1)
    expect(h.sent).toEqual([TRUE, TRUE])
    vi.advanceTimersByTime(VIEWING_REPEAT_MS * 2)
    expect(h.sent).toEqual([TRUE, TRUE, TRUE, TRUE])

    h.set(false)
    h.sender.update()
    expect(h.sent).toEqual([TRUE, TRUE, TRUE, TRUE, FALSE])
    // The repeat stopped.
    vi.advanceTimersByTime(VIEWING_REPEAT_MS * 3)
    expect(h.sent).toEqual([TRUE, TRUE, TRUE, TRUE, FALSE])
  })

  it('does not repeat a state the daemon already has', () => {
    const h = harness(false)
    h.sender.update()
    h.sender.update()
    expect(h.sent).toEqual([])
    h.set(true)
    h.sender.update()
    h.sender.update()
    expect(h.sent).toEqual([TRUE])
  })

  it('the repeat tick notices a missed blur and sends false', () => {
    const h = harness(true)
    h.sender.update()
    h.set(false)
    vi.advanceTimersByTime(VIEWING_REPEAT_MS)
    expect(h.sent).toEqual([TRUE, FALSE])
    vi.advanceTimersByTime(VIEWING_REPEAT_MS * 2)
    expect(h.sent).toEqual([TRUE, FALSE])
  })

  it('dispose sends false once and stops the repeat; nothing after', () => {
    const h = harness(true)
    h.sender.update()
    h.sender.dispose()
    h.sender.dispose()
    expect(h.sent).toEqual([TRUE, FALSE])
    vi.advanceTimersByTime(VIEWING_REPEAT_MS * 3)
    h.sender.update()
    h.sender.socketOpened()
    expect(h.sent).toEqual([TRUE, FALSE])
  })

  it('dispose while not viewing sends nothing', () => {
    const h = harness(false)
    h.sender.update()
    h.sender.dispose()
    expect(h.sent).toEqual([])
  })

  it('a new socket hears true again (the daemon cleared the old socket’s state)', () => {
    const h = harness(true)
    h.setSocket(false)
    h.sender.update() // no socket yet: nothing went out
    expect(h.sent).toEqual([])
    h.setSocket(true)
    h.sender.socketOpened()
    expect(h.sent).toEqual([TRUE])
    // Reconnect while still viewing: said again on the new socket.
    h.sender.socketOpened()
    expect(h.sent).toEqual([TRUE, TRUE])
    // A socket that opens while not viewing hears nothing.
    h.set(false)
    h.sender.update()
    h.sender.socketOpened()
    expect(h.sent).toEqual([TRUE, TRUE, FALSE])
  })
})

// ── Hook wiring ───────────────────────────────────────────────────────────

class FakeSocket {
  static all: FakeSocket[] = []
  url: string
  readyState = 0
  sent: string[] = []
  onmessage: ((ev: { data: string }) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onopen: ((ev: Event) => void) | null = null
  onclose: ((ev: Event) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeSocket.all.push(this)
  }
  open(): void {
    this.readyState = 1
    this.onopen?.(new Event('open'))
  }
  send(frame: string): void {
    if (this.readyState !== 1) throw new Error('send on a socket that is not open')
    this.sent.push(frame)
  }
  close(): void {
    this.readyState = 3
  }
}

let hasFocus = true
let visibility: DocumentVisibilityState = 'visible'

function setWindow(focus: boolean, vis: DocumentVisibilityState): void {
  hasFocus = focus
  visibility = vis
}

describe('useOverlayThread viewing frames', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    FakeSocket.all = []
    resetGridDialQueueForTests()
    __resetServerScopesForTests()
    noteServerVersion('local', null, ['thread-push-v1'])
    vi.stubGlobal('WebSocket', FakeSocket)
    setWindow(true, 'visible')
    vi.spyOn(document, 'hasFocus').mockImplementation(() => hasFocus)
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
    daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'thread') return { conversation_id: 'conv-1', has_more: false, items: [] }
      if (route === 'thread/activity') return { ok: true, turn: null }
      throw new Error(`unexpected GET ${route}`)
    })
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
    daemonCliGet.mockReset()
    __resetServerScopesForTests()
  })

  async function mount(viewing: boolean) {
    const hook = renderHook(
      (p: { viewing: boolean }) =>
        useOverlayThread({ scope: primaryScope(), addr: 'k2', conversationId: null, enabled: true, viewing: p.viewing }),
      { initialProps: { viewing } },
    )
    await vi.waitFor(() => expect(FakeSocket.all.length).toBe(1))
    const sock = FakeSocket.all[0]
    act(() => sock.open())
    return { hook, sock }
  }

  it('a viewing pane says true on open, every 30 s, false on blur and on unmount', async () => {
    const { hook, sock } = await mount(true)
    expect(sock.url).toContain('/cli/overlay/events?conversation=conv-1')
    expect(sock.sent).toEqual([TRUE])

    await act(async () => {
      await vi.advanceTimersByTimeAsync(VIEWING_REPEAT_MS)
    })
    expect(sock.sent).toEqual([TRUE, TRUE])

    setWindow(false, 'visible')
    act(() => window.dispatchEvent(new Event('blur')))
    expect(sock.sent).toEqual([TRUE, TRUE, FALSE])
    await act(async () => {
      await vi.advanceTimersByTimeAsync(VIEWING_REPEAT_MS * 2)
    })
    expect(sock.sent).toEqual([TRUE, TRUE, FALSE])

    setWindow(true, 'visible')
    act(() => window.dispatchEvent(new Event('focus')))
    expect(sock.sent).toEqual([TRUE, TRUE, FALSE, TRUE])

    hook.unmount()
    expect(sock.sent).toEqual([TRUE, TRUE, FALSE, TRUE, FALSE])
  })

  it('the tab going hidden sends false; coming back sends true', async () => {
    const { sock } = await mount(true)
    expect(sock.sent).toEqual([TRUE])
    setWindow(true, 'hidden')
    act(() => document.dispatchEvent(new Event('visibilitychange')))
    expect(sock.sent).toEqual([TRUE, FALSE])
    setWindow(true, 'visible')
    act(() => document.dispatchEvent(new Event('visibilitychange')))
    expect(sock.sent).toEqual([TRUE, FALSE, TRUE])
  })

  it('the pane being hidden (viewing prop false) sends false at once', async () => {
    const { hook, sock } = await mount(true)
    hook.rerender({ viewing: false })
    expect(sock.sent).toEqual([TRUE, FALSE])
    hook.rerender({ viewing: true })
    expect(sock.sent).toEqual([TRUE, FALSE, TRUE])
  })

  it('a surface that does not pass viewing never sends a frame', async () => {
    const { hook, sock } = await mount(false)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(VIEWING_REPEAT_MS * 2)
    })
    act(() => window.dispatchEvent(new Event('focus')))
    hook.unmount()
    expect(sock.sent).toEqual([])
  })

  it('a server without thread-push-v1 gets no viewing frames', async () => {
    noteServerVersion('local', null, [])
    const { hook, sock } = await mount(true)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(VIEWING_REPEAT_MS * 2)
    })
    hook.unmount()
    expect(sock.sent).toEqual([])
  })

  it('an unfocused window does not claim the Thread', async () => {
    setWindow(false, 'visible')
    const { sock } = await mount(true)
    expect(sock.sent).toEqual([])
  })
})
