// The device stream's player (fix 4): decode and draw in a Worker on an
// OffscreenCanvas where the webview allows it, so React work on the main
// thread never delays a frame. The socket stays here (M-10); each binary
// message's ArrayBuffer is transferred to the worker (`sim-player.worker.ts`)
// with its header fields.
//
// Where it runs: `canUseWorkerPlayer()` needs `Worker`,
// `HTMLCanvasElement.transferControlToOffscreen` and `createImageBitmap`;
// then the worker's own probe says whether it has an OffscreenCanvas 2D
// context (else: the main-thread path) and a `VideoDecoder` for the codecs
// `PROBE_CODECS` lists (else: JPEG only). A worker that errors falls back to
// the main-thread path on the same socket (`lost`). The manual switch
// `localStorage['k2.mobile.streamPlayer'] = 'main'` forces the main thread.
//
// A canvas can hand over control once: after a fallback the pane re-keys
// its <canvas> so the main path draws on a fresh element.

import { FRAME_HEADER_LEN, VIDEO_HEADER_LEN, type SimFrame } from './sim-stream'
import type { FromWorker, PixelRgb, ToWorker } from './sim-player.worker'

/** Set to "main" to keep this tab's player on the main thread. */
export const PLAYER_PREF_KEY = 'k2.mobile.streamPlayer'

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
    return env.localStorage?.getItem(PLAYER_PREF_KEY) === 'main'
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
  return new Worker(new URL('./sim-player.worker.ts', import.meta.url), { type: 'module', name: 'k2-sim-player' }) as unknown as WorkerLike
}

export interface WorkerProbe {
  canvas2d: boolean
  video: boolean
}

export interface PlayerHandlers {
  /** Frame `seq` is on the canvas; times on this thread's clock
   *  (`performance.now()`); the centre pixel while sampling. */
  drawn(seq: number, decodedAt: number, drawnAt: number, px: PixelRgb | null): void
  ack(seq: number): void
  keyframe(): void
  /** Video can't go on in the worker: switch the stream to JPEG. */
  videoFailed(why: string): void
  /** The worker itself failed: use the main-thread path. */
  lost(why: string): void
}

export class SimPlayer {
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
  frame(f: SimFrame): boolean {
    if (this.dead || !f.raw) return false
    const s = f.source
    if (s.kind === 'h264') {
      this.post({ t: 'frame', kind: 'h264', seq: f.seq, buf: f.raw, offset: VIDEO_HEADER_LEN, widthPx: s.widthPx, heightPx: s.heightPx, key: s.key }, [f.raw])
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
        this.resolveReady({ canvas2d: m.canvas2d, video: m.video })
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
        this.h.keyframe()
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
