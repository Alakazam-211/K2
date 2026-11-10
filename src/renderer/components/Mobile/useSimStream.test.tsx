// @vitest-environment jsdom
// useSimStream's frame paths, fail loud: the offer it makes, JPEG frames
// drawn newest-only with every seq acked, H.264 through a fake WebCodecs
// (drawn cropped, acked, held frames not drawn), a remote video stream
// raised from the JPEG default rate unless the person picked one, and the
// JPEG reconnect when video fails here.

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { encodeFrame, type DecodeSupport } from '@/components/Stream/stream-wire'
import { fakePlayerWorker, fakeWebCodecs, H264_DELTA as DELTA, H264_KEY as KEY, webCodecsGlobals, type FakeVideoDecoder } from '@/components/Stream/testing'
import type { SimFrame, SimServerMsg, SimStreamConnector, SimStreamHandlers, SimStreamOffer, SimViewerPath } from './sim-stream'
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

const hello = (codec: 'jpeg' | 'h264', remote: boolean, path?: SimViewerPath): SimServerMsg => ({
  t: 'hello',
  sim: 's1',
  lane: 'ios',
  state: 'running',
  fps: 8,
  remote,
  ...(path ? { path } : {}),
  hold: { kind: 'free' },
  isolation: 'x',
  codec,
  window: 3,
})

const h264 = (seq: number, key: boolean): SimFrame => ({
  seq,
  widthPt: 411,
  heightPt: 915,
  source: { kind: 'h264', key, data: key ? KEY : DELTA, widthPx: 411, heightPx: 915 },
})

let decoders: FakeVideoDecoder[] = []

/** The engine's fake WebCodecs as this webview's globals (H.264 yes). */
function stubWebCodecs(): void {
  const codecs = fakeWebCodecs({ h264: true, vp8: true })
  decoders = codecs.decoders
  for (const [k, v] of Object.entries(webCodecsGlobals(codecs))) vi.stubGlobal(k, v)
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

  it('raises a LAN video stream left at the remote default to 60 (fix 2), and keeps a picked rate', () => {
    stubWebCodecs()
    const { connector, conns } = fakeConnector()
    const { canvasRef } = fakeCanvas()
    const { result } = renderHook(() =>
      useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false }),
    )
    act(() => conns[0].handlers.onServer(hello('h264', true, 'lan')))
    expect(conns[0].fps).toEqual([60])
    expect(result.current.path).toBe('lan')
    // A JPEG stream on the LAN stays where it is (the daemon caps it at 15).
    const j = fakeConnector()
    renderHook(() => useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector: j.connector, canvasRef, hold: false }))
    act(() => j.conns[0].handlers.onServer(hello('jpeg', true, 'lan')))
    expect(j.conns[0].fps).toEqual([])
    // A rate the person picked is kept.
    const p = fakeConnector()
    const picked = renderHook(() => useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector: p.connector, canvasRef, hold: false }))
    act(() => picked.result.current.setFps(8))
    act(() => p.conns[0].handlers.onServer(hello('h264', true, 'lan')))
    expect(p.conns[0].fps).toEqual([8])
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

  // ── Fix 4: which player, and the fallbacks ──────────────────────────

  it('jsdom has no transferControlToOffscreen: frames draw on this thread', () => {
    const { connector } = fakeConnector()
    const { canvasRef } = fakeCanvas()
    const { result } = renderHook(() => useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false }))
    expect(result.current.player).toBe('main')
    expect(result.current.canvasKey).toBe(0)
  })

  describe('with a worker player', () => {
    let restore: (() => void) | null = null
    beforeEach(() => {
      const proto = HTMLCanvasElement.prototype as unknown as { transferControlToOffscreen?: unknown }
      const had = Object.prototype.hasOwnProperty.call(proto, 'transferControlToOffscreen')
      proto.transferControlToOffscreen = () => ({})
      vi.stubGlobal('Worker', function Worker() {})
      restore = () => {
        if (!had) delete proto.transferControlToOffscreen
      }
    })
    afterEach(() => {
      restore?.()
    })

    const H264_ONLY: DecodeSupport = { h264: true, vp8: false, jpeg: true }
    const NO_VIDEO: DecodeSupport = { h264: false, vp8: false, jpeg: true }

    function fakeWorker(probe: { canvas2d: boolean; decode: DecodeSupport }) {
      const w = fakePlayerWorker(probe)
      return { w, posted: w.posted }
    }

    function workerCanvas() {
      const c = fakeCanvas()
      ;(c.canvas as unknown as { transferControlToOffscreen: () => unknown }).transferControlToOffscreen = () => ({ offscreen: true })
      return c
    }

    it('waits for the probe; a worker without VideoDecoder means JPEG only; frames go to it transferred', async () => {
      const { connector, conns } = fakeConnector()
      const { canvasRef, drawImage } = workerCanvas()
      const fw = fakeWorker({ canvas2d: true, decode: NO_VIDEO })
      const { result } = renderHook(() =>
        useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false, startWorker: () => fw.w }),
      )
      await waitFor(() => expect(conns).toHaveLength(1))
      expect(result.current.player).toBe('worker')
      expect(conns[0].offer).toEqual({ video: false, window: 3 })
      expect(fw.posted[0].msg).toMatchObject({ t: 'init', canvas: { offscreen: true } })
      const raw = encodeFrame({ seq: 1, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2 }, new Uint8Array([0xff, 0xd8]))
      act(() => conns[0].handlers.onFrame({ seq: 1, widthPt: 4, heightPt: 2, source: { kind: 'jpeg', data: new Blob([]) }, raw }))
      const sent = fw.posted.find((p) => p.msg.t === 'frame')
      expect(sent?.msg).toMatchObject({ t: 'frame', kind: 'jpeg', seq: 1, offset: 16 })
      expect(sent?.transfer).toEqual([raw])
      expect(drawImage).not.toHaveBeenCalled()
      act(() => fw.w.onmessage?.({ data: { t: 'drawn', seq: 1, decodedAt: 0, drawnAt: 0, px: null } }))
      act(() => fw.w.onmessage?.({ data: { t: 'ack', seq: 1 } }))
      expect(conns[0].acks).toEqual([1])
      expect(result.current.hasFrame).toBe(true)
      // The worker's decoder lost its place: the socket asks for a keyframe.
      act(() => fw.w.reply({ t: 'keyframe' }))
      expect(conns[0].keyframes).toBe(1)
    })

    it('a worker that fails falls back to this thread on the same socket, with a fresh canvas key', async () => {
      const { connector, conns } = fakeConnector()
      const { canvasRef, drawImage } = workerCanvas()
      const fw = fakeWorker({ canvas2d: true, decode: H264_ONLY })
      const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
      const { result } = renderHook(() =>
        useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false, startWorker: () => fw.w }),
      )
      await waitFor(() => expect(conns).toHaveLength(1))
      expect(conns[0].offer).toEqual({ video: true, window: 3 })
      act(() => fw.w.onerror?.({ message: 'worker crashed' }))
      expect(result.current.player).toBe('main')
      expect(result.current.canvasKey).toBe(1)
      expect(conns).toHaveLength(1)
      const raw = encodeFrame({ seq: 2, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2 }, new Uint8Array([0xff, 0xd8]))
      await act(async () => {
        conns[0].handlers.onFrame({ seq: 2, widthPt: 4, heightPt: 2, source: { kind: 'jpeg', data: new Blob([]) }, raw })
        await new Promise((r) => setTimeout(r, 0))
      })
      expect(drawImage).toHaveBeenCalledTimes(1)
      expect(conns[0].acks).toEqual([2])
      expect(warn).toHaveBeenCalledTimes(1)
      warn.mockRestore()
    })

    it('a worker with no OffscreenCanvas 2D context: the main thread from the start', async () => {
      const { connector, conns } = fakeConnector()
      const { canvasRef } = workerCanvas()
      const fw = fakeWorker({ canvas2d: false, decode: H264_ONLY })
      const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
      const { result } = renderHook(() =>
        useSimStream({ scope: SCOPE, workspace: 'p-docs', visible: true, fps: 8, connector, canvasRef, hold: false, startWorker: () => fw.w }),
      )
      await waitFor(() => expect(conns).toHaveLength(1))
      expect(result.current.player).toBe('main')
      expect(result.current.canvasKey).toBe(1)
      expect(conns[0].offer).toEqual({ video: true, window: 3 })
      warn.mockRestore()
    })
  })
})
