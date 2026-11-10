// The stream engine's player (fix 4; E9, source-neutral: the Mobile tab's
// device and the remote desktop): decode and draw in a Worker on an
// OffscreenCanvas where the webview allows it, so React work on the main
// thread never delays a frame. The socket stays with the pane (M-10); each
// binary message's ArrayBuffer is transferred to the worker
// (`stream-player.worker.ts`) with its header fields.
//
// Where it runs: `canUseWorkerPlayer()` needs `Worker`,
// `HTMLCanvasElement.transferControlToOffscreen` and `createImageBitmap`;
// then the worker's own probe says whether it has an OffscreenCanvas 2D
// context (else: the main-thread path) and which codecs its `VideoDecoder`
// takes (`ready` → `decode`: H.264, VP8, JPEG; the pane offers only those).
// A worker that errors falls back to the main-thread path on the same
// socket (`lost`). The manual switch `localStorage['k2.stream.player'] =
// 'main'` forces the main thread (the Mobile tab's older
// `k2.mobile.streamPlayer` still does too).
//
// A canvas can hand over control once: after a fallback the pane re-keys
// its <canvas> so the main path draws on a fresh element.

import { FRAME_HEADER_LEN, VIDEO_HEADER_LEN, isVideoSource, type DecodeSupport, type StreamFrame } from './stream-wire'
import type { FromWorker, PixelRgb, ToWorker } from './stream-player.worker'

/** Set to "main" to keep this webview's players on the main thread. */
export const PLAYER_PREF_KEY = 'k2.stream.player'
/** The Mobile tab's switch from before the engine module; still read. */
export const LEGACY_PLAYER_PREF_KEY = 'k2.mobile.streamPlayer'

/** How long the worker has to answer its probe or a thumbnail ask. */
export const PROBE_TIMEOUT_MS = 3000
export const THUMB_TIMEOUT_MS = 500

export interface WorkerLike {
  postMessage(msg: unknown, transfer?: Transferable[]): void
  onmessage: ((ev: { data: unknown }) => void) | null
  onerror: ((ev: unknown) => void) | null
  terminate(): void
}

interface PlayerEnv {
  Worker?: unknown
  HTMLCanvasElement?: { prototype?: { transferControlToOffscreen?: unknown } }
  createImageBitmap?: unknown
  localStorage?: { getItem(k: string): string | null }
}

function pinnedMain(env: PlayerEnv): boolean {
  try {
    const ls = env.localStorage
    return ls?.getItem(PLAYER_PREF_KEY) === 'main' || ls?.getItem(LEGACY_PLAYER_PREF_KEY) === 'main'
  } catch {
    return false
  }
}

/** The first check (the worker's own probe is the second). */
export function canUseWorkerPlayer(env: PlayerEnv = globalThis as unknown as PlayerEnv): boolean {
  if (pinnedMain(env)) return false
  return (
    typeof env.Worker === 'function' &&
    typeof env.HTMLCanvasElement?.prototype?.transferControlToOffscreen === 'function' &&
    typeof env.createImageBitmap === 'function'
  )
}

/** The real worker (Vite bundles it; the Tauri CSP allows `worker-src 'self' blob:`). */
export function startPlayerWorker(): WorkerLike {
  return new Worker(new URL('./stream-player.worker.ts', import.meta.url), { type: 'module', name: 'k2-stream-player' }) as unknown as WorkerLike
}

export interface WorkerProbe {
  canvas2d: boolean
  /** What the worker decodes (`offerCodecs` turns it into `codecs=`). */
  decode: DecodeSupport
}

export interface PlayerHandlers {
  /** Frame `seq` is on the canvas; times on this thread's clock
   *  (`performance.now()`); the centre pixel while sampling. */
  drawn(seq: number, decodedAt: number, drawnAt: number, px: PixelRgb | null): void
  ack(seq: number): void
  /** A delta came with no keyframe before it, or the decoder failed: the
   *  frames are dropped until a keyframe; the pane sends `{"t":"keyframe"}`. */
  needKeyframe(): void
  /** Video can't go on in the worker: switch the stream to JPEG. */
  videoFailed(why: string): void
  /** The worker itself failed: use the main-thread path. */
  lost(why: string): void
}

export class StreamPlayer {
  readonly ready: Promise<WorkerProbe>
  private resolveReady: (p: WorkerProbe) => void = () => {}
  private readonly thumbs = new Map<number, (b: Blob | null) => void>()
  private nextThumb = 1
  private dead = false
  private drawnAny = false
  /** `performance.timeOrigin` here: the worker's times are absolute. */
  private readonly timeOrigin: number
  /** The centre pixel of the newest drawn frame (while sampling). */
  lastPx: PixelRgb | null = null

  constructor(
    private readonly worker: WorkerLike,
    canvas: unknown,
    private readonly h: PlayerHandlers,
    opts: { sample?: boolean; transferCanvas?: boolean; timeOrigin?: number } = {},
  ) {
    this.timeOrigin = opts.timeOrigin ?? performance.timeOrigin
    this.ready = new Promise((res) => {
      this.resolveReady = res
    })
    worker.onmessage = (ev) => this.onMessage(ev.data as FromWorker)
    worker.onerror = (ev) => this.lose(`the player worker failed: ${String((ev as { message?: unknown })?.message ?? ev)}`)
    const init: ToWorker = { t: 'init', canvas, sample: opts.sample === true }
    worker.postMessage(init, opts.transferCanvas === false ? [] : [canvas as Transferable])
    this.post({ t: 'probe' })
  }

  /** Has the worker drawn a frame yet? */
  get started(): boolean {
    return this.drawnAny
  }

  get alive(): boolean {
    return !this.dead
  }

  private post(m: ToWorker, transfer: Transferable[] = []): void {
    if (this.dead) return
    this.worker.postMessage(m, transfer)
  }

  /** Hand a frame to the worker (its buffer is transferred). False when it
   *  can't go (no raw buffer, e.g. the stub's painted frames). */
  frame(f: StreamFrame): boolean {
    if (this.dead || !f.raw) return false
    const s = f.source
    if (isVideoSource(s)) {
      this.post({ t: 'frame', kind: s.kind, seq: f.seq, buf: f.raw, offset: VIDEO_HEADER_LEN, widthPx: s.widthPx, heightPx: s.heightPx, key: s.key }, [f.raw])
      return true
    }
    if (s.kind === 'jpeg') {
      this.post({ t: 'frame', kind: 'jpeg', seq: f.seq, buf: f.raw, offset: FRAME_HEADER_LEN, widthPx: 0, heightPx: 0, key: false }, [f.raw])
      return true
    }
    return false
  }

  hold(on: boolean): void {
    this.post({ t: 'hold', on })
  }

  sample(on: boolean): void {
    if (!on) this.lastPx = null
    this.post({ t: 'sample', on })
  }

  /** A new socket begins. */
  reset(): void {
    this.post({ t: 'reset' })
  }

  /** The daemon switched the socket to JPEG. */
  dropVideo(): void {
    this.post({ t: 'drop-video' })
  }

  /** The canvas as a JPEG (the screenshot toast), or null if the worker
   *  doesn't answer in time. */
  thumb(timeoutMs = THUMB_TIMEOUT_MS): Promise<Blob | null> {
    if (this.dead) return Promise.resolve(null)
    const id = this.nextThumb++
    return new Promise((res) => {
      const timer = setTimeout(() => {
        this.thumbs.delete(id)
        res(null)
      }, timeoutMs)
      this.thumbs.set(id, (b) => {
        clearTimeout(timer)
        res(b)
      })
      this.post({ t: 'thumb', id })
    })
  }

  close(): void {
    if (this.dead) return
    this.post({ t: 'close' })
    this.dead = true
    this.worker.onmessage = null
    this.worker.onerror = null
    this.worker.terminate()
    for (const r of this.thumbs.values()) r(null)
    this.thumbs.clear()
  }

  private lose(why: string): void {
    if (this.dead) return
    this.close()
    this.h.lost(why)
  }

  private onMessage(m: FromWorker): void {
    if (this.dead) return
    switch (m.t) {
      case 'probe':
        this.resolveReady({ canvas2d: m.canvas2d, decode: m.decode })
        return
      case 'drawn':
        this.drawnAny = true
        if (m.px) this.lastPx = m.px
        this.h.drawn(m.seq, m.decodedAt - this.timeOrigin, m.drawnAt - this.timeOrigin, m.px)
        return
      case 'ack':
        this.h.ack(m.seq)
        return
      case 'keyframe':
        this.h.needKeyframe()
        return
      case 'failed':
        this.h.videoFailed(m.why)
        return
      case 'thumb': {
        const r = this.thumbs.get(m.id)
        this.thumbs.delete(m.id)
        r?.(m.blob)
        return
      }
    }
  }
}

/** A Blob as a data: URL (the toast's thumbnail). */
export function blobDataUrl(b: Blob): Promise<string | null> {
  return new Promise((res) => {
    const r = new FileReader()
    r.onload = () => res(typeof r.result === 'string' ? r.result : null)
    r.onerror = () => res(null)
    r.readAsDataURL(b)
  })
}
