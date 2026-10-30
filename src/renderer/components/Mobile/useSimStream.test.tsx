// @vitest-environment jsdom
// useSimStream's frame paths, fail loud: the offer it makes, JPEG frames
// drawn newest-only with every seq acked, H.264 through a fake WebCodecs
// (drawn cropped, acked, held frames not drawn), a remote video stream
// raised from the JPEG default rate unless the person picked one, and the
// JPEG reconnect when video fails here.

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import type { SimFrame, SimServerMsg, SimStreamConnector, SimStreamHandlers, SimStreamOffer } from './sim-stream'
import { useSimStream } from './useSimStream'

/** One scope for every render (a new one would reconnect, as in the pane). */
const SCOPE = {} as never

interface Conn {
  handlers: SimStreamHandlers
  offer?: SimStreamOffer
  acks: number[]
  fps: number[]
  keyframes: number
  closed: boolean
}

function fakeConnector(): { connector: SimStreamConnector; conns: Conn[] } {
  const conns: Conn[] = []
  const connector: SimStreamConnector = (opts) => {
    const c: Conn = { handlers: opts.handlers, offer: opts.offer, acks: [], fps: [], keyframes: 0, closed: false }
    conns.push(c)
    return {
      send: () => {},
      ack: (seq) => c.acks.push(seq),
      setFps: (n) => c.fps.push(n),
      requestKeyframe: () => {
        c.keyframes += 1
      },
      close: () => {
        c.closed = true
      },
    }
  }
  return { connector, conns }
}

function fakeCanvas() {
  const drawImage = vi.fn()
  const canvas = { width: 0, height: 0, getContext: () => ({ drawImage }) }
  return { canvasRef: { current: canvas as unknown as HTMLCanvasElement }, drawImage, canvas }
}

const hello = (codec: 'jpeg' | 'h264', remote: boolean): SimServerMsg => ({
  t: 'hello',
  sim: 's1',
  lane: 'ios',
  state: 'running',
  fps: 8,
  remote,
  hold: { kind: 'free' },
  isolation: 'x',
  codec,
  window: 3,
})

const SC = [0, 0, 0, 1]
const KEY = new Uint8Array([...SC, 0x67, 0x64, 0x0c, 0x1f, 0xac, ...SC, 0x68, 0xee, ...SC, 0x65, 0x88])
const DELTA = new Uint8Array([...SC, 0x41, 0x9a])

const h264 = (seq: number, key: boolean): SimFrame => ({
  seq,
  widthPt: 411,
  heightPt: 915,
  source: { kind: 'h264', key, data: key ? KEY : DELTA, widthPx: 411, heightPx: 915 },
})

interface FakeDecoder {
  init: { output(f: unknown): void; error(e: unknown): void }
  configs: { codec: string }[]
  state: string
}

let decoders: FakeDecoder[] = []

function stubWebCodecs(): void {
  decoders = []
  class VideoDecoder {
    static isConfigSupported = async () => ({ supported: true })
    state = 'unconfigured'
    configs: { codec: string }[] = []
    constructor(readonly init: FakeDecoder['init']) {
      decoders.push(this as unknown as FakeDecoder)
    }
    configure(c: { codec: string }): void {
      this.state = 'configured'
      this.configs.push(c)
    }
    decode(): void {}
    close(): void {
      this.state = 'closed'
    }
  }
  class EncodedVideoChunk {
    constructor(readonly init: unknown) {}
  }
  vi.stubGlobal('VideoDecoder', VideoDecoder)
  vi.stubGlobal('EncodedVideoChunk', EncodedVideoChunk)
}

const videoFrame = (timestamp: number) => ({ timestamp, displayWidth: 412, displayHeight: 916, close: vi.fn() })

beforeEach(() => {
  vi.stubGlobal('createImageBitmap', async () => ({ width: 411, height: 915, close: () => {} }))
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('useSimStream', () => {
  it('offers video and a window, draws only the newest JPEG and acks every seq', async () => {
    const { connector, conns } = fakeConnector()
    const { canvasRef, drawImage } = fakeCanvas()
    const { result } = renderHook(() =>
      useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 30, connector, canvasRef, hold: false }),
    )
    expect(conns).toHaveLength(1)
    expect(conns[0].offer).toEqual({ video: true, window: 3 })
    const c = conns[0]
    act(() => {
      c.handlers.onConn('open')
      c.handlers.onServer(hello('jpeg', false))
      for (const seq of [1, 2, 3]) {
        c.handlers.onFrame({ seq, widthPt: 411, heightPt: 915, source: { kind: 'jpeg', data: new Blob([new Uint8Array([0xff, 0xd8])]) } })
      }
    })
    await waitFor(() => expect(c.acks).toEqual([1, 2, 3]))
    expect(drawImage).toHaveBeenCalledTimes(1)
    expect(result.current.hasFrame).toBe(true)
    expect(result.current.codec).toBe('jpeg')
    // A JPEG stream keeps the rate it asked for.
    expect(c.fps).toEqual([])
  })

  it('decodes H.264 with WebCodecs: drawn cropped to the header size, acked, not drawn while held', () => {
    stubWebCodecs()
    const { connector, conns } = fakeConnector()
    const { canvasRef, drawImage, canvas } = fakeCanvas()
    let hold = false
    const { result, rerender } = renderHook(() =>
      useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold }),
    )
    const c = conns[0]
    act(() => {
      c.handlers.onConn('open')
      c.handlers.onServer(hello('h264', true))
    })
    // A remote video stream at the JPEG default goes to 24 fps.
    expect(c.fps).toEqual([24])
    expect(result.current.codec).toBe('h264')
    act(() => c.handlers.onFrame(h264(1, true)))
    expect(decoders).toHaveLength(1)
    expect(decoders[0].configs).toEqual([{ codec: 'avc1.640C1F', optimizeForLatency: true }])
    const f1 = videoFrame(1)
    act(() => decoders[0].init.output(f1))
    expect(drawImage).toHaveBeenCalledWith(f1, 0, 0, 411, 915, 0, 0, 411, 915)
    expect([canvas.width, canvas.height]).toEqual([411, 915])
    expect(f1.close).toHaveBeenCalled()
    expect(c.acks).toEqual([1])
    expect(result.current.hasFrame).toBe(true)
    expect(result.current.screen).toEqual({ widthPt: 411, heightPt: 915 })
    hold = true
    rerender()
    act(() => c.handlers.onFrame(h264(2, false)))
    act(() => decoders[0].init.output(videoFrame(2)))
    expect(drawImage).toHaveBeenCalledTimes(1)
    expect(c.acks).toEqual([1, 2])
    // A delta after the decoder lost its place asks for a keyframe.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    act(() => decoders[0].init.error(new Error('glitch')))
    expect(c.keyframes).toBe(1)
    warn.mockRestore()
  })

  it('keeps the rate the person picked', () => {
    stubWebCodecs()
    const { connector, conns } = fakeConnector()
    const { canvasRef } = fakeCanvas()
    const { result } = renderHook(() =>
      useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false }),
    )
    act(() => result.current.setFps(8))
    act(() => conns[0].handlers.onServer(hello('h264', true)))
    expect(conns[0].fps).toEqual([8])
  })

  it('reconnects JPEG-only when video fails here, and drops video when the daemon falls back', () => {
    stubWebCodecs()
    const { connector, conns } = fakeConnector()
    const { canvasRef } = fakeCanvas()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    renderHook(() => useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 30, connector, canvasRef, hold: false }))
    const c = conns[0]
    act(() => {
      c.handlers.onConn('open')
      c.handlers.onServer(hello('h264', false))
    })
    let seq = 0
    for (let i = 0; i < 3; i++) {
      act(() => c.handlers.onFrame(h264(++seq, true)))
      act(() => decoders[decoders.length - 1].init.error(new Error(`bad ${i}`)))
    }
    expect(conns).toHaveLength(2)
    expect(c.closed).toBe(true)
    expect(conns[1].offer).toEqual({ video: false, window: 3 })
    // The daemon's own fallback (`codec: jpeg`) closes the decoder.
    const c2 = conns[1]
    act(() => {
      c2.handlers.onConn('open')
      c2.handlers.onServer(hello('h264', false))
      c2.handlers.onFrame(h264(50, true))
    })
    const d = decoders[decoders.length - 1]
    expect(d.state).toBe('configured')
    act(() => c2.handlers.onServer({ t: 'codec', codec: 'jpeg', message: 'encoder stopped' }))
    expect(d.state).toBe('closed')
    // What the closed decoder held is acked (the socket stays open).
    expect(c2.acks).toEqual([50])
    warn.mockRestore()
  })
})
