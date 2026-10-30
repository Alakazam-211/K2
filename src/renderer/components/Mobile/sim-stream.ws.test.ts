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
import { encodeFrame, encodeVideoFrame, wsSimStream, type SimFrame, type SimStreamHandlers } from './sim-stream'
import { resetVideoSupportForTests } from './sim-video'

async function tick(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

/** Until the adapter dialed (creds, the decoder probe, the dial queue). */
async function untilDialed(): Promise<void> {
  for (let i = 0; i < 200 && FakeWebSocket.instances.length === 0; i++) await Promise.resolve()
  if (FakeWebSocket.instances.length === 0) throw new Error('the adapter never dialed')
}

beforeEach(() => {
  FakeWebSocket.instances = []
  resetGridDialQueueForTests()
  vi.stubGlobal('WebSocket', FakeWebSocket)
})

afterEach(() => {
  vi.unstubAllGlobals()
  resetVideoSupportForTests()
})

const scopeFor = (): ServerScope =>
  ({
    ...fakeScope('box.test'),
    creds: async () => ({ host: 'box.test', port: 443, secure: true, token: 'tok-1' }),
  }) as unknown as ServerScope

/** A WebCodecs that says yes to H.264 (the probe only). */
function stubWebCodecs(supported: boolean): void {
  class VideoDecoder {
    static isConfigSupported = async () => ({ supported })
  }
  class EncodedVideoChunk {}
  vi.stubGlobal('VideoDecoder', VideoDecoder)
  vi.stubGlobal('EncodedVideoChunk', EncodedVideoChunk)
}

describe('wsSimStream offer (0.46.7)', () => {
  it('offers H.264 and a window when this webview decodes it, reads K2SV frames, and asks for keyframes', async () => {
    stubWebCodecs(true)
    const frames: SimFrame[] = []
    const handlers: SimStreamHandlers = { onFrame: (f) => frames.push(f), onServer: () => {}, onConn: () => {} }
    const conn = wsSimStream({ scope: scopeFor(), workspace: 'p-docs', fps: 30, handlers, offer: { video: true, window: 3 } })
    await tick()
    const sock = FakeWebSocket.instances[0]
    const url = new URL(sock.url)
    expect(url.searchParams.get('codecs')).toBe('h264,jpeg')
    expect(url.searchParams.get('win')).toBe('3')
    sock.open()
    const au = new Uint8Array([0, 0, 0, 1, 0x65, 0x88])
    sock.onmessage?.({ data: encodeVideoFrame({ seq: 9, widthPx: 412, heightPx: 916, widthPt: 411, heightPt: 915, key: true, tsMs: 5 }, au) })
    // A JPEG frame on the same socket still reads (the daemon may fall back).
    sock.onmessage?.({ data: encodeFrame({ seq: 10, widthPx: 411, heightPx: 915, widthPt: 411, heightPt: 915 }, new Uint8Array([0xff, 0xd8])) })
    // An unknown video codec id is dropped, loudly.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    sock.onmessage?.({ data: encodeVideoFrame({ seq: 11, widthPx: 1, heightPx: 1, widthPt: 1, heightPt: 1, key: false, codec: 9 }, au) })
    expect(warn).toHaveBeenCalledWith('[mobile] stream: a K2SV message this tab cannot read')
    warn.mockRestore()
    expect(frames).toHaveLength(2)
    const v = frames[0]
    expect(v.seq).toBe(9)
    expect(v.widthPt).toBe(411)
    if (v.source.kind !== 'h264') throw new Error(`expected h264, got ${v.source.kind}`)
    expect({ key: v.source.key, w: v.source.widthPx, h: v.source.heightPx, data: Array.from(v.source.data) }).toEqual({
      key: true,
      w: 412,
      h: 916,
      data: [0, 0, 0, 1, 0x65, 0x88],
    })
    expect(frames[1].source.kind).toBe('jpeg')
    conn.requestKeyframe()
    conn.ack(9)
    expect(sock.sent).toEqual(['{"t":"keyframe"}', '{"t":"ack","seq":9}'])
    conn.close()
  })

  it('offers JPEG only (still with a window) when this webview has no H.264 decoder', async () => {
    stubWebCodecs(false)
    const handlers: SimStreamHandlers = { onFrame: () => {}, onServer: () => {}, onConn: () => {} }
    const conn = wsSimStream({ scope: scopeFor(), workspace: 'p-docs', fps: 8, handlers, offer: { video: true, window: 3 } })
    await untilDialed()
    const url = new URL(FakeWebSocket.instances[0].url)
    expect(url.searchParams.get('codecs')).toBe('jpeg')
    expect(url.searchParams.get('win')).toBe('3')
    conn.close()
  })
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
    await untilDialed()
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
