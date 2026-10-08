// prd-zen-user-widgets-v2 TUW3.2 (protocol), TUW3.6 (ready, ping, unload),
// UW33 (chord gating) — the frame host, with a fake channel and fake timers.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ZenFrameHello } from './zen-custom-types'
import type { ZenCustomLayer } from './zen-custom-bridge'
import {
  startZenFrameHost,
  ZEN_FRAME_PING_MS,
  ZEN_FRAME_READY_MS,
  type ZenFrameHostOptions,
} from './zen-custom-host'

class FakePort {
  sent: unknown[] = []
  closed = false
  onmessage: ((e: MessageEvent) => void) | null = null
  postMessage(m: unknown): void {
    this.sent.push(m)
  }
  close(): void {
    this.closed = true
  }
  /** The frame sends `m` to this (host) end. */
  deliver(m: unknown): void {
    this.onmessage?.({ data: m } as MessageEvent)
  }
}

function fakeChannel(): { ch: MessageChannel; host: FakePort; frameEnd: object } {
  const host = new FakePort()
  const frameEnd = { frame: true }
  return { ch: { port1: host, port2: frameEnd } as unknown as MessageChannel, host, frameEnd }
}

const HELLO: ZenFrameHello = {
  k2: 'hello',
  v: 1,
  caps: ['agents:read'],
  features: ['zen-widgets-v1'],
  widget: { id: 'arcade', name: 'Agent Arcade', garden: 'g-test0001' },
  config: {},
  motion: { reduced: false },
}

interface Setup {
  host: FakePort
  posted: Array<{ msg: unknown; target: string; transfer: unknown[] }>
  stops: string[]
  handled: unknown[]
  disposed: { layer: boolean }
  focused: { v: boolean }
  chords: Array<[unknown, boolean]>
  frameEnd: object
  dispose(): void
}

function setup(over: Partial<ZenFrameHostOptions> = {}, label = 'arcade'): Setup {
  const { ch, host, frameEnd } = fakeChannel()
  const posted: Setup['posted'] = []
  const stops: string[] = []
  const handled: unknown[] = []
  const disposed = { layer: false }
  const focused = { v: true }
  const chords: Array<[unknown, boolean]> = []
  const h = startZenFrameHost(
    { postMessage: (msg, target, transfer) => posted.push({ msg, target, transfer }) },
    {
      hello: { ...HELLO, widget: { ...HELLO.widget, id: label } },
      label,
      channel: () => ch,
      stop: (r) => stops.push(r),
      focused: () => focused.v,
      onChord: (c, f) => void chords.push([c, f]),
      now: () => Date.now(),
      makeLayer: (push): ZenCustomLayer => ({
        handle: async (m) => {
          handled.push(m)
          const id = (m as { id?: number }).id
          if ((m as { sub?: number }).sub !== undefined) push((m as { sub: number }).sub, { pushed: label })
          return typeof id === 'number' ? { id, ok: true, value: label } : null
        },
        subscriptions: 0,
        dispose: () => void (disposed.layer = true),
      }),
      ...over,
    },
  )
  return { host, posted, stops, handled, disposed, focused, chords, frameEnd, dispose: h.dispose }
}

beforeEach(() => {
  vi.useFakeTimers()
})
afterEach(() => {
  vi.useRealTimers()
})

describe('TUW3.2: the private port', () => {
  it('the hello goes to the frame window once, with the frame’s end of the channel', () => {
    const s = setup()
    expect(s.posted).toEqual([{ msg: HELLO, target: '*', transfer: [s.frameEnd] }])
  })

  it('two widgets’ ports can’t act for each other: each port only reaches its own layer', async () => {
    const a = setup({}, 'a')
    const b = setup({}, 'b')
    a.host.deliver({ id: 1, verb: 'gardens.list', args: [] })
    b.host.deliver({ sub: 4, verb: 'agents.subscribe', args: [] })
    await vi.runOnlyPendingTimersAsync()
    expect(a.handled).toEqual([{ id: 1, verb: 'gardens.list', args: [] }])
    expect(b.handled).toEqual([{ sub: 4, verb: 'agents.subscribe', args: [] }])
    expect(a.host.sent).toEqual([{ id: 1, ok: true, value: 'a' }])
    expect(b.host.sent).toEqual([{ sub: 4, value: { pushed: 'b' } }])
  })
})

describe('TUW3.6: ready, ping, unload', () => {
  it('no ready within 10 s → not-started; the port closes', () => {
    const s = setup()
    vi.advanceTimersByTime(ZEN_FRAME_READY_MS - 1)
    expect(s.stops).toEqual([])
    vi.advanceTimersByTime(1)
    expect(s.stops).toEqual(['not-started'])
    expect(s.host.closed).toBe(true)
    expect(s.disposed.layer).toBe(true)
  })

  it('after ready, pings every 5 s; answered pings keep it; three missed → not-responding', () => {
    const s = setup()
    s.host.deliver({ ready: true })
    vi.advanceTimersByTime(ZEN_FRAME_READY_MS)
    expect(s.stops).toEqual([])
    const pings = (): number[] => s.host.sent.flatMap((m) => (typeof (m as { ping?: number }).ping === 'number' ? [(m as { ping: number }).ping] : []))
    // Answer every ping for a while.
    for (let i = 0; i < 4; i++) {
      vi.advanceTimersByTime(ZEN_FRAME_PING_MS)
      s.host.deliver({ pong: pings()[pings().length - 1] })
    }
    expect(s.stops).toEqual([])
    // Then silence: the 1st, 2nd, 3rd ticks after the last answered ping
    // see unanswered pings; the third miss stops it.
    vi.advanceTimersByTime(ZEN_FRAME_PING_MS * 3)
    expect(s.stops).toEqual([])
    vi.advanceTimersByTime(ZEN_FRAME_PING_MS)
    expect(s.stops).toEqual(['not-responding'])
  })

  it('20 errors in a minute → failing', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const s = setup()
    s.host.deliver({ ready: true })
    for (let i = 0; i < 19; i++) s.host.deliver({ error: { message: `e${i}` } })
    expect(s.stops).toEqual([])
    // A minute passes with every ping answered.
    for (let i = 0; i < 13; i++) {
      vi.advanceTimersByTime(ZEN_FRAME_PING_MS)
      const last = s.host.sent.filter((m) => typeof (m as { ping?: number }).ping === 'number').pop() as { ping: number }
      s.host.deliver({ pong: last.ping })
    }
    for (let i = 0; i < 19; i++) s.host.deliver({ error: { message: 'again' } })
    expect(s.stops).toEqual([])
    s.host.deliver({ error: { message: 'last' } })
    expect(s.stops).toEqual(['failing'])
    warn.mockRestore()
  })
})

describe('UW33: forwarded chords', () => {
  it('go to the page’s gate with this frame’s focus, never to the layer', () => {
    const s = setup()
    s.host.deliver({ chord: 'garden-2' })
    s.focused.v = false
    s.host.deliver({ chord: 'zen-exit' })
    expect(s.chords).toEqual([
      ['garden-2', true],
      ['zen-exit', false],
    ])
    expect(s.handled).toEqual([])
  })
})
