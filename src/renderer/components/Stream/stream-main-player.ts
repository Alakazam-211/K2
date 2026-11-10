// The stream engine's main-thread player (E9, S6b; source-neutral): the
// frame loop a pane runs when the worker player (`stream-player.ts`) can't
// run here or was lost. `StreamPlayback` (`stream-playback.ts`) picks it;
// the Mobile tab and the remote desktop share this one copy.
//
//  - JPEG: draws each frame on the pane's <canvas>, strictly in order, then
//    acks its seq. A frame with a newer one already waiting is acked
//    without drawing (stale), as is every frame while the view is held.
//  - H.264 / VP8: the engine's `VideoSink` (`stream-video.ts`) decodes
//    every frame in order (WebCodecs), draws each output, acks it; it drops
//    frames and asks for a keyframe when it lost its place. A fresh sink
//    (after `reset`, or the worker lost mid-stream) waits for a keyframe.
//    No WebCodecs here, or video failing, calls `videoFailed` (the pane
//    reconnects JPEG-only); the canvas keeps the last frame meanwhile.
//  - A stub's painted frame is drawn at 2× its points.
//  - `reset()` ends one socket: what is still in flight is not acked (the
//    socket is gone) and nothing more is drawn for it.
//
// The canvas is read on each frame (`canvas()`): after a fallback the pane
// re-keys its <canvas>, so the element changes under the same ref.

import { drawJpeg, drawVideoFrame } from './stream-canvas'
import type { PixelRgb } from './stream-player.worker'
import { browserWebCodecs, VideoSink, type WebCodecsLike } from './stream-video'
import { isVideoSource, type StreamFrame } from './stream-wire'

export interface MainPlayerHandlers {
  /** Frame `seq` is on the canvas; times on `performance.now()`'s clock
   *  (`decodedAt` is null for a painted stub frame; `px` is the worker's
   *  sample, so always null here: read the canvas instead). */
  drawn(seq: number, decodedAt: number | null, drawnAt: number, px: PixelRgb | null): void
  ack(seq: number): void
  /** The decoder lost its place: the pane sends `{"t":"keyframe"}`. */
  needKeyframe(): void
  /** Video can't go on here: switch the stream to JPEG. */
  videoFailed(why: string): void
}

export interface MainPlayerOptions {
  /** The pane's <canvas> now, or null (unmounted). */
  canvas(): HTMLCanvasElement | null
  /** The view is held (frozen): frames are acked, not drawn. */
  held(): boolean
  /** WebCodecs (default the webview's; read when a video frame first comes). */
  codecs?: () => WebCodecsLike | null
  /** The log prefix (`[mobile]`, `[desktop]`). */
  tag?: string
}

/** Painted size for a stub frame: 2× points so text stays crisp. */
const PAINT_SCALE = 2

async function drawStill(canvas: HTMLCanvasElement, frame: StreamFrame, onDecoded?: () => void): Promise<boolean> {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  if (frame.source.kind === 'paint') {
    const w = frame.widthPt * PAINT_SCALE
    const h = frame.heightPt * PAINT_SCALE
    if (canvas.width !== w) canvas.width = w
    if (canvas.height !== h) canvas.height = h
    frame.source.paint(ctx, w, h)
    return true
  }
  if (frame.source.kind !== 'jpeg') return false
  return drawJpeg(canvas, frame.source.data, onDecoded)
}

export class MainThreadPlayer {
  /** Bumped by `reset`: work queued for an older socket stops. */
  private gen = 0
  // JPEG frames draw strictly in order; one newer than the frame being
  // drawn makes it stale.
  private drawing: Promise<void> = Promise.resolve()
  private newestJpeg = -1
  private sink: VideoSink | null = null
  private warned = false
  private readonly tag: string

  constructor(
    private readonly h: MainPlayerHandlers,
    private readonly opts: MainPlayerOptions,
  ) {
    this.tag = `[${opts.tag ?? 'stream'}]`
  }

  /** Frames waiting in the video decoder (tests). */
  get queued(): number {
    return this.sink?.queued ?? 0
  }

  frame(frame: StreamFrame): void {
    if (isVideoSource(frame.source)) {
      const s = this.videoSink()
      if (!s) {
        this.h.ack(frame.seq)
        return
      }
      const { kind, key, data, widthPx, heightPx } = frame.source
      s.push({ seq: frame.seq, codec: kind, key, data, widthPx, heightPx })
      return
    }
    const gen = this.gen
    const live = (): boolean => gen === this.gen
    this.newestJpeg = frame.seq
    this.drawing = this.drawing.then(async () => {
      if (!live()) return
      try {
        const canvas = this.opts.canvas()
        // A newer frame is already here: this one is stale.
        const stale = frame.seq !== this.newestJpeg
        if (canvas && !this.opts.held() && !stale) {
          let decodedAt: number | null = null
          const drawn = await drawStill(canvas, frame, () => {
            decodedAt = performance.now()
          })
          if (drawn) {
            if (live()) this.h.drawn(frame.seq, decodedAt, performance.now(), null)
          } else {
            this.warnNoContext()
          }
        }
      } catch (err) {
        console.warn(`${this.tag} frame draw failed:`, err)
      } finally {
        // Ack even a skipped or failed frame, or the stream stalls.
        if (live()) this.h.ack(frame.seq)
      }
    })
  }

  /** The daemon switched the socket to JPEG: free what the decoder held
   *  (the socket stays open, so it is acked). */
  dropVideo(): void {
    this.sink?.close(true)
    this.sink = null
  }

  /** The socket ended (or a new one begins): nothing in flight is acked. */
  reset(): void {
    this.gen += 1
    this.sink?.close()
    this.sink = null
    this.drawing = Promise.resolve()
    this.newestJpeg = -1
  }

  private warnNoContext(): void {
    if (this.warned) return
    this.warned = true
    console.warn(`${this.tag} the canvas has no 2d context; frames are acked but not drawn`)
  }

  private videoSink(): VideoSink | null {
    if (this.sink) return this.sink
    const codecs = (this.opts.codecs ?? browserWebCodecs)()
    if (!codecs) {
      console.warn(`${this.tag} video frames but no WebCodecs here; switching to JPEG`)
      this.h.videoFailed('no WebCodecs here')
      return null
    }
    const gen = this.gen
    const live = (): boolean => gen === this.gen
    this.sink = new VideoSink({
      codecs,
      held: () => this.opts.held(),
      ack: (seq) => {
        if (live()) this.h.ack(seq)
      },
      needKeyframe: () => {
        if (live()) this.h.needKeyframe()
      },
      failed: (why) => {
        console.warn(`${this.tag} video failed here; switching to JPEG:`, why)
        if (live()) this.h.videoFailed(why)
      },
      draw: (frame, widthPx, heightPx) => {
        const canvas = this.opts.canvas()
        if (!live() || !canvas) return
        // The decoder's timestamp is the frame's seq (VideoSink).
        const decodedAt = performance.now()
        if (drawVideoFrame(canvas, frame, widthPx, heightPx)) {
          this.h.drawn(frame.timestamp, decodedAt, performance.now(), null)
        } else {
          this.warnNoContext()
        }
      },
    })
    return this.sink
  }
}
