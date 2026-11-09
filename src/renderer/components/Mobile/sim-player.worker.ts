// The device stream's decode and draw, off the main thread (fix 4).
//
// The main side (`sim-player.ts`) keeps the WebSocket (it dials through the
// per-server queue with the scope's credentials, M-10) and transfers each
// binary message's ArrayBuffer here, zero copy, with its kind and header
// fields. This worker owns the OffscreenCanvas the pane's <canvas> handed
// over and draws on it:
//  - JPEG: `createImageBitmap` → `drawImage`, with the main path's stale
//    rule (a frame with a newer one already here is acked, not drawn);
//  - H.264: the same `H264Sink` the main path uses, with this worker's own
//    `VideoDecoder`.
// It answers with `drawn` (for the latency readout; times absolute, so the
// main side can put them on its own clock), `ack`, `keyframe`, `failed` and
// `thumb` (the screenshot toast's thumbnail, M-9: the main thread can no
// longer read the canvas). While the latency readout is on it samples the
// centre pixel of each drawn frame (the photon probe).
//
// `WorkerPlayerCore` is the logic with its seams injected (tests); the
// bottom of the file wires it to the worker's `self`.

import { H264Sink, PROBE_CODECS, browserWebCodecs, type VideoFrameLike, type WebCodecsLike } from './sim-video'

/** Main → worker. */
export type ToWorker =
  | { t: 'init'; canvas: unknown; sample: boolean }
  | { t: 'probe' }
  | { t: 'frame'; kind: 'jpeg' | 'h264'; seq: number; buf: ArrayBuffer; offset: number; widthPx: number; heightPx: number; key: boolean }
  | { t: 'hold'; on: boolean }
  | { t: 'sample'; on: boolean }
  /** A new socket: drop the decoder; nothing to ack on the old one. */
  | { t: 'reset' }
  /** The daemon switched this socket to JPEG: free what the decoder held. */
  | { t: 'drop-video' }
  | { t: 'thumb'; id: number }
  | { t: 'close' }

export type PixelRgb = [number, number, number]

/** Worker → main. `decodedAt` / `drawnAt` are absolute ms
 *  (`performance.timeOrigin + performance.now()`). */
export type FromWorker =
  | { t: 'probe'; canvas2d: boolean; video: boolean }
  | { t: 'drawn'; seq: number; decodedAt: number; drawnAt: number; px: PixelRgb | null }
  | { t: 'ack'; seq: number }
  | { t: 'keyframe' }
  | { t: 'failed'; why: string }
  | { t: 'thumb'; id: number; blob: Blob | null }

export interface BitmapLike {
  readonly width: number
  readonly height: number
  close(): void
}

export interface Ctx2dLike {
  drawImage(...args: unknown[]): void
  getImageData(x: number, y: number, w: number, h: number): { data: ArrayLike<number> }
}

export interface OffscreenLike {
  width: number
  height: number
  getContext(type: '2d', opts?: Record<string, unknown>): Ctx2dLike | null
  convertToBlob?(opts: { type: string; quality: number }): Promise<Blob>
}

export interface WorkerDeps {
  post(msg: FromWorker): void
  codecs: WebCodecsLike | null
  decodeJpeg(blob: Blob): Promise<BitmapLike>
  /** Absolute ms. */
  now(): number
}

export class WorkerPlayerCore {
  private canvas: OffscreenLike | null = null
  private ctx: Ctx2dLike | null = null
  private held = false
  private sample = false
  private sink: H264Sink | null = null
  private newestJpeg = -1
  private drawing: Promise<void> = Promise.resolve()
  private warned = false

  constructor(private readonly deps: WorkerDeps) {}

  handle(m: ToWorker): void {
    switch (m.t) {
      case 'init':
        this.canvas = m.canvas as OffscreenLike
        // `desynchronized` where the webview honours it (lower latency).
        this.ctx = this.canvas.getContext('2d', { desynchronized: true }) ?? this.canvas.getContext('2d')
        this.sample = m.sample
        return
      case 'probe':
        void this.probe()
        return
      case 'frame':
        if (m.kind === 'h264') this.video(m)
        else this.jpeg(m)
        return
      case 'hold':
        this.held = m.on
        return
      case 'sample':
        this.sample = m.on
        return
      case 'reset':
        this.sink?.close(false)
        this.sink = null
        this.newestJpeg = -1
        return
      case 'drop-video':
        this.sink?.close(true)
        this.sink = null
        return
      case 'thumb':
        void this.thumb(m.id)
        return
      case 'close':
        this.sink?.close(false)
        this.sink = null
        return
    }
  }

  private async probe(): Promise<void> {
    let video = false
    const codecs = this.deps.codecs
    if (codecs) {
      for (const codec of PROBE_CODECS) {
        if (await codecs.isConfigSupported({ codec, optimizeForLatency: true })) {
          video = true
          break
        }
      }
    }
    this.deps.post({ t: 'probe', canvas2d: this.ctx !== null, video })
  }

  private centre(): PixelRgb | null {
    const c = this.canvas
    if (!this.sample || !this.ctx || !c || c.width === 0 || c.height === 0) return null
    const d = this.ctx.getImageData(c.width >> 1, c.height >> 1, 1, 1).data
    return [d[0], d[1], d[2]]
  }

  private noContext(): void {
    if (this.warned) return
    this.warned = true
    console.warn('[mobile] the player worker has no 2d context; frames are acked but not drawn')
  }

  private jpeg(m: Extract<ToWorker, { t: 'frame' }>): void {
    this.newestJpeg = m.seq
    this.drawing = this.drawing.then(async () => {
      try {
        // A newer frame is already here: this one is stale.
        if (this.held || m.seq !== this.newestJpeg) return
        const c = this.canvas
        const ctx = this.ctx
        if (!c || !ctx) {
          this.noContext()
          return
        }
        const bmp = await this.deps.decodeJpeg(new Blob([new Uint8Array(m.buf, m.offset)], { type: 'image/jpeg' }))
        const decodedAt = this.deps.now()
        try {
          if (c.width !== bmp.width) c.width = bmp.width
          if (c.height !== bmp.height) c.height = bmp.height
          ctx.drawImage(bmp, 0, 0)
        } finally {
          bmp.close()
        }
        this.deps.post({ t: 'drawn', seq: m.seq, decodedAt, drawnAt: this.deps.now(), px: this.centre() })
      } catch (err) {
        console.warn('[mobile] worker frame draw failed:', err)
      } finally {
        // Ack even a skipped or failed frame, or the stream stalls.
        this.deps.post({ t: 'ack', seq: m.seq })
      }
    })
  }

  private video(m: Extract<ToWorker, { t: 'frame' }>): void {
    if (!this.sink) {
      const codecs = this.deps.codecs
      if (!codecs) {
        this.deps.post({ t: 'ack', seq: m.seq })
        this.deps.post({ t: 'failed', why: 'H.264 frames but no WebCodecs in the player worker' })
        return
      }
      this.sink = new H264Sink({
        codecs,
        held: () => this.held,
        ack: (seq) => this.deps.post({ t: 'ack', seq }),
        needKeyframe: () => this.deps.post({ t: 'keyframe' }),
        failed: (why) => this.deps.post({ t: 'failed', why }),
        draw: (frame, widthPx, heightPx) => this.drawVideo(frame, widthPx, heightPx),
      })
    }
    this.sink.push({ seq: m.seq, key: m.key, data: new Uint8Array(m.buf, m.offset), widthPx: m.widthPx, heightPx: m.heightPx })
  }

  /** A decoded frame, cropped to the header's size (H.264 pads odd sizes). */
  private drawVideo(frame: VideoFrameLike, widthPx: number, heightPx: number): void {
    const decodedAt = this.deps.now()
    const c = this.canvas
    const ctx = this.ctx
    if (!c || !ctx) {
      this.noContext()
      return
    }
    const w = Math.min(widthPx, frame.displayWidth)
    const h = Math.min(heightPx, frame.displayHeight)
    if (c.width !== w) c.width = w
    if (c.height !== h) c.height = h
    ctx.drawImage(frame, 0, 0, w, h, 0, 0, w, h)
    // The decoder's timestamp is the frame's seq (H264Sink).
    this.deps.post({ t: 'drawn', seq: frame.timestamp, decodedAt, drawnAt: this.deps.now(), px: this.centre() })
  }

  private async thumb(id: number): Promise<void> {
    const c = this.canvas
    let blob: Blob | null = null
    if (c?.convertToBlob && c.width > 0 && c.height > 0) {
      try {
        blob = await c.convertToBlob({ type: 'image/jpeg', quality: 0.6 })
      } catch (err) {
        console.warn('[mobile] worker thumbnail failed:', err)
      }
    }
    this.deps.post({ t: 'thumb', id, blob })
  }
}

// ── The worker entry ────────────────────────────────────────────────────

interface WorkerScope {
  postMessage(msg: unknown): void
  onmessage: ((ev: { data: ToWorker }) => void) | null
  importScripts?: unknown
}

const scope = globalThis as unknown as WorkerScope
// Only inside a worker (tests import the core).
if (typeof scope.importScripts === 'function' && typeof (globalThis as { document?: unknown }).document === 'undefined') {
  const core = new WorkerPlayerCore({
    post: (msg) => scope.postMessage(msg),
    codecs: browserWebCodecs(),
    decodeJpeg: (blob) => createImageBitmap(blob),
    now: () => performance.timeOrigin + performance.now(),
  })
  scope.onmessage = (ev) => core.handle(ev.data)
}
