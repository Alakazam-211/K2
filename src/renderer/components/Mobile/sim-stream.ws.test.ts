// The real stream adapter (`wsSimStream`, prd-mobile-dev-v1 M-A4) against a
// fake socket, fail loud: it dials `/cli/sim/stream` through the per-server
// queue with the workspace, fps cap and token; binary messages are K2SF
// frames acked by their seq; text JSON is tagged by "t"; input typed
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
import { encodeFrame, wsSimStream, type SimStreamHandlers } from './sim-stream'

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

describe('wsSimStream (k2-core::sim::stream)', () => {
  it('dials, decodes K2SF frames and t-tagged text, queues early input, acks by seq, and goes quiet on close', async () => {
    const scope = {
      ...fakeScope('box.test'),
      creds: async () => ({ host: 'box.test', port: 443, secure: true, token: 'tok-1' }),
    } as unknown as ServerScope
    const seen: string[] = []
    const handlers: SimStreamHandlers = {
      onFrame: (f) => seen.push(`frame:${f.seq}:${f.widthPt}x${f.heightPt}:${f.source.kind}`),
      onServer: (m) => seen.push(`server:${m.t}${m.t === 'hold' ? `:${m.hold.kind}` : ''}${m.t === 'error' ? `:${m.code}` : ''}`),
      onConn: (s, why) => seen.push(`conn:${s}${why ? `:${why}` : ''}`),
    }
    const conn = wsSimStream({ scope, workspace: 'p-docs', fps: 8, handlers })
    conn.send({ t: 'tap', x: 10, y: 20.5 })
    await tick()
    expect(FakeWebSocket.instances).toHaveLength(1)
    const sock = FakeWebSocket.instances[0]
    expect(sock.binaryType).toBe('arraybuffer')
    const url = new URL(sock.url)
    expect(url.pathname).toBe('/cli/sim/stream')
    expect(url.searchParams.get('workspace')).toBe('p-docs')
    expect(url.searchParams.get('fps')).toBe('8')
    expect(url.protocol).toBe('wss:')
    expect(url.host).toBe('box.test')
    expect(url.searchParams.get('token')).toBe('tok-1')

    sock.open()
    expect(sock.sent).toEqual(['{"t":"tap","x":10,"y":20.5}'])
    sock.onmessage?.({
      data: '{"t":"hello","sim":"s1","lane":"android","state":"running","fps":8,"remote":true,"hold":{"kind":"free"},"isolation":"x"}',
    })
    sock.onmessage?.({ data: encodeFrame({ seq: 41, widthPx: 824, heightPx: 1830, widthPt: 412, heightPt: 915 }, new Uint8Array([0xff, 0xd8])) })
    sock.onmessage?.({ data: new Uint8Array([0xff, 0xd8]).buffer }) // no header: dropped
    sock.onmessage?.({ data: '{"t":"hold","hold":{"kind":"takeover","by":"rosson","idle_ms":600000}}' })
    conn.ack(41)
    conn.setFps(4)
    conn.send({ t: 'take-over' })
    expect(sock.sent.slice(1)).toEqual(['{"t":"ack","seq":41}', '{"t":"fps","fps":4}', '{"t":"take-over"}'])
    sock.onmessage?.({ data: '{"t":"error","code":"sim_held","message":"held"}' })
    sock.onclose?.({ code: 4410, reason: '' })
    conn.close()
    sock.onmessage?.({ data: '{"t":"fps","fps":2}' })
    expect(sock.readyState).toBe(3)
    expect(seen).toEqual([
      'conn:connecting',
      'conn:open',
      'server:hello',
      'frame:41:412x915:jpeg',
      'server:hold:takeover',
      'server:error:sim_held',
      'conn:closed:The device stopped.',
    ])
  })
})
