// The real stream adapter (`wsSimStream`, prd-mobile-dev-v1 M-A4) against a
// fake socket, fail loud: it dials `/cli/sim/stream` through the per-server
// queue with the sim id, fps cap and token; binary messages are frames
// numbered per connection; text JSON is meta / control / error; input typed
// before OPEN goes out in order after it; acks name the frame; close stops
// every callback.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  url: string
  readyState = 0
  binaryType = 'blob'
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: ((ev: { code: number; reason?: string }) => void) | null = null
  private listeners = new Map<string, Set<() => void>>()
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  addEventListener(type: string, fn: () => void): void {
    const set = this.listeners.get(type) ?? new Set()
    set.add(fn)
    this.listeners.set(type, set)
  }
  removeEventListener(type: string, fn: () => void): void {
    this.listeners.get(type)?.delete(fn)
  }
  open(): void {
    this.readyState = 1
    for (const fn of [...(this.listeners.get('open') ?? [])]) fn()
    this.onopen?.()
  }
  send(text: string): void {
    if (this.readyState !== 1) throw new Error('send before open')
    this.sent.push(text)
  }
  close(): void {
    this.readyState = 3
  }
}

import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import type { ServerScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { wsSimStream, type SimStreamHandlers } from './sim-stream'

async function tick(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

beforeEach(() => {
  FakeWebSocket.instances = []
  resetGridDialQueueForTests()
  vi.stubGlobal('WebSocket', FakeWebSocket)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('wsSimStream', () => {
  it('dials, decodes frames and text, queues early input, acks, and goes quiet on close', async () => {
    const scope = {
      ...fakeScope('box.test'),
      creds: async () => ({ host: 'box.test', port: 443, secure: true, token: 'tok-1' }),
    } as unknown as ServerScope
    const seen: string[] = []
    const handlers: SimStreamHandlers = {
      onFrame: (f) => seen.push(`frame:${f.seq}:${f.source.kind}`),
      onMeta: (m) => seen.push(`meta:${m.width}x${m.height}:${m.orientation}`),
      onControl: (c) => seen.push(`control:${c.heldBy}:${c.by ?? ''}`),
      onState: (s, why) => seen.push(`state:${s}${why ? `:${why}` : ''}`),
    }
    const conn = wsSimStream({ scope, simId: 's1', fps: 8, handlers })
    conn.send({ action: 'tap', x: 10, y: 20 })
    await tick()
    expect(FakeWebSocket.instances).toHaveLength(1)
    const sock = FakeWebSocket.instances[0]
    const url = new URL(sock.url)
    expect(url.pathname).toBe('/cli/sim/stream')
    expect(url.searchParams.get('sim')).toBe('s1')
    expect(url.searchParams.get('fps')).toBe('8')
    expect(url.protocol).toBe('wss:')
    expect(url.host).toBe('box.test')
    expect(url.searchParams.get('token')).toBe('tok-1')

    sock.open()
    expect(sock.sent).toEqual([JSON.stringify({ action: 'tap', x: 10, y: 20 })])
    sock.onmessage?.({ data: JSON.stringify({ type: 'meta', width: 412, height: 915, orientation: 'portrait' }) })
    sock.onmessage?.({ data: new Blob([new Uint8Array([0xff, 0xd8, 0xff])], { type: 'image/jpeg' }) })
    sock.onmessage?.({ data: new Uint8Array([0xff, 0xd8]).buffer })
    sock.onmessage?.({ data: JSON.stringify({ type: 'control', heldBy: 'takeover', by: 'rosson' }) })
    conn.ack(2)
    expect(sock.sent[1]).toBe(JSON.stringify({ action: 'ack', seq: 2 }))
    sock.onmessage?.({ data: JSON.stringify({ type: 'error', message: 'sim stopped' }) })
    conn.close()
    sock.onmessage?.({ data: JSON.stringify({ type: 'meta', width: 1, height: 1 }) })
    sock.onclose?.({ code: 1000 })
    expect(sock.readyState).toBe(3)
    expect(seen).toEqual([
      'state:connecting',
      'state:open',
      'meta:412x915:portrait',
      'frame:1:jpeg',
      'frame:2:jpeg',
      'control:takeover:rosson',
      'state:error:sim stopped',
    ])
  })
})
