// Which player draws a pane's stream (fix 4; E9, S6b; source-neutral): the
// worker player (`stream-player.ts`) where the webview allows it, else the
// main-thread player (`stream-main-player.ts`). The Mobile tab and the
// remote desktop share this one decision.
//
//  - `kind` starts `pending` where `canUseWorkerPlayer()` says yes, else
//    `main`. `start()` (once the pane shows a canvas) hands the canvas to a
//    worker and waits for its probe; the pane dials only once `kind` is
//    settled, offering what `workerDecode` says (the main thread finds out
//    lazily: video fails → `videoFailed`).
//  - Falls back to `main` on: no canvas yet, a canvas that won't transfer,
//    a probe that doesn't answer in `PROBE_TIMEOUT_MS`, a worker with no
//    OffscreenCanvas 2D context, and the worker failing mid-stream
//    (`lost`). The socket stays; the next frames draw here, and video
//    waits for its next keyframe. Every fallback calls `player('main', why)`:
//    the pane re-keys its <canvas>, since a canvas hands over control once.
//  - Per socket: `reset()` when one begins, `end()` when it closes.

import { blobDataUrl, canUseWorkerPlayer, PROBE_TIMEOUT_MS, StreamPlayer, startPlayerWorker, type PlayerEnv, type WorkerLike } from './stream-player'
import { canvasCentre, canvasThumb } from './stream-canvas'
import { MainThreadPlayer, type MainPlayerHandlers } from './stream-main-player'
import type { PixelRgb } from './stream-player.worker'
import type { WebCodecsLike } from './stream-video'
import type { DecodeSupport, StreamFrame } from './stream-wire'

export type PlayerKind = 'pending' | 'worker' | 'main'

export interface PlaybackHandlers extends MainPlayerHandlers {
  /** The player settled (`worker` after its probe) or fell back (`main`,
   *  with why). */
  player(kind: 'worker' | 'main', why: string | null): void
}

export interface PlaybackOptions {
  /** The pane's <canvas> now (it changes when the pane re-keys it). */
  canvas(): HTMLCanvasElement | null
  /** The view is held (the main path reads it per frame; the worker gets
   *  `hold()`). */
  held(): boolean
  /** The worker (default the real one; tests inject a fake). */
  startWorker?: () => WorkerLike
  /** The main path's WebCodecs (default the webview's). */
  codecs?: () => WebCodecsLike | null
  /** What `canUseWorkerPlayer` reads (default the globals). */
  env?: PlayerEnv
  /** The log prefix (`[mobile]`, `[desktop]`). */
  tag?: string
}

export class StreamPlayback {
  private playerKind: PlayerKind
  private worker: StreamPlayer | null = null
  private decode: DecodeSupport | null = null
  private readonly main: MainThreadPlayer
  private lostWarned = false
  private readonly tag: string

  constructor(
    private readonly h: PlaybackHandlers,
    private readonly opts: PlaybackOptions,
  ) {
    this.tag = `[${opts.tag ?? 'stream'}]`
    this.playerKind = canUseWorkerPlayer(opts.env) ? 'pending' : 'main'
    this.main = new MainThreadPlayer(h, { canvas: opts.canvas, held: opts.held, codecs: opts.codecs, tag: opts.tag })
  }

  /** Where frames are drawn; `pending` until the worker answers its probe. */
  get kind(): PlayerKind {
    return this.playerKind
  }

  /** What the worker decodes while it is the player; null on the main path. */
  get workerDecode(): DecodeSupport | null {
    return this.worker ? this.decode : null
  }

  /** Start the worker (while `pending`; a no-op otherwise). */
  start(opts: { sample?: boolean } = {}): void {
    if (this.playerKind !== 'pending' || this.worker) return
    const canvas = this.opts.canvas()
    if (!canvas) {
      this.toMain('no canvas yet')
      return
    }
    let p: StreamPlayer
    try {
      const off = canvas.transferControlToOffscreen()
      p = new StreamPlayer(
        (this.opts.startWorker ?? startPlayerWorker)(),
        off,
        {
          drawn: (seq, decodedAt, drawnAt, px) => this.h.drawn(seq, decodedAt, drawnAt, px),
          ack: (seq) => this.h.ack(seq),
          needKeyframe: () => this.h.needKeyframe(),
          videoFailed: (why) => {
            console.warn(`${this.tag} video failed in the player worker; switching to JPEG:`, why)
            this.h.videoFailed(why)
          },
          lost: (why) => this.toMain(why),
        },
        { sample: opts.sample === true },
      )
    } catch (err) {
      this.toMain(err instanceof Error ? err.message : String(err))
      return
    }
    this.worker = p
    let settled = false
    const timer = setTimeout(() => {
      if (!settled && this.worker === p) this.toMain('the worker did not answer its probe')
    }, PROBE_TIMEOUT_MS)
    void p.ready.then((probe) => {
      settled = true
      clearTimeout(timer)
      if (this.worker !== p) return
      if (!probe.canvas2d) {
        this.toMain('no OffscreenCanvas 2D context in the worker')
        return
      }
      this.decode = probe.decode
      this.playerKind = 'worker'
      this.h.player('worker', null)
    })
  }

  /** One frame off the socket: to the worker (its buffer is transferred),
   *  else drawn here. A frame the worker can't take (a stub's painted
   *  frame) is acked at once. */
  frame(f: StreamFrame): void {
    const p = this.worker
    if (p) {
      if (!p.frame(f)) this.h.ack(f.seq)
      return
    }
    this.main.frame(f)
  }

  /** A new socket begins. */
  reset(): void {
    this.worker?.reset()
    this.main.reset()
  }

  /** The socket closed: the main path stops (nothing more is acked). */
  end(): void {
    this.main.reset()
  }

  hold(on: boolean): void {
    this.worker?.hold(on)
  }

  sample(on: boolean): void {
    this.worker?.sample(on)
  }

  /** The daemon switched the socket to JPEG. */
  dropVideo(): void {
    this.main.dropVideo()
    this.worker?.dropVideo()
  }

  /** The canvas as a small JPEG data URL (the screenshot toast), or null. */
  async thumb(): Promise<string | null> {
    const p = this.worker
    if (p) {
      const b = await p.thumb()
      return b ? blobDataUrl(b) : null
    }
    return canvasThumb(this.opts.canvas())
  }

  /** The centre pixel of what is on screen (the photon probe), or null. */
  centrePixel(): PixelRgb | null {
    const p = this.worker
    return p ? p.lastPx : canvasCentre(this.opts.canvas())
  }

  /** The pane unmounts: the worker goes (a `pending` playback may `start`
   *  again), and the main path stops. */
  close(): void {
    this.worker?.close()
    this.worker = null
    this.main.reset()
  }

  private toMain(why: string): void {
    if (!this.lostWarned) {
      this.lostWarned = true
      console.warn(`${this.tag} drawing on the main thread; the worker player is off here:`, why)
    }
    this.worker?.close()
    this.worker = null
    this.decode = null
    this.playerKind = 'main'
    // The old canvas may have handed its control to the worker.
    this.h.player('main', why)
  }
}
