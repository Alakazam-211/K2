// Video in voice rooms: one sink's frames onto a tile's canvas (Sew's draft
// §B9.2, drawn straight to a canvas instead of through a MediaStream).
//
// Encoded mode: the webview's WebCodecs `VideoDecoder`, one per sink, tuned for
// latency. H.264 comes as Annex B, so it's configured without a `description`,
// and again whenever a keyframe's SPS names another profile or level. A
// discontinuity resets the decoder first. A decoder error, or a queue more than
// 3 frames behind, drops the deltas and asks the plugin for a keyframe.
//
// Decoded mode (the webview can't decode the codec): I420 frames become
// `VideoFrame`s on the same path; RGBA frames (no WebCodecs at all) go through
// `putImageData`.
//
// Every `VideoFrame` is closed.

import { H264_PROBE_CODEC, h264CodecOf, parseFrame, type FrameHeader, type VideoCodec } from './video-frame'
import type { SinkStats } from './video-engine'

/** The WebCodecs pieces this file uses: injected, so tests run without them. */
export interface WebCodecsEnv {
  VideoDecoder: typeof VideoDecoder
  EncodedVideoChunk: typeof EncodedVideoChunk
  VideoFrame: typeof VideoFrame
}

/** This webview's WebCodecs, or null (WebKitGTK may have none). */
export function webCodecsEnv(): WebCodecsEnv | null {
  const g = globalThis as Partial<WebCodecsEnv>
  if (typeof g.VideoDecoder !== 'function' || typeof g.EncodedVideoChunk !== 'function' || typeof g.VideoFrame !== 'function') return null
  return { VideoDecoder: g.VideoDecoder, EncodedVideoChunk: g.EncodedVideoChunk, VideoFrame: g.VideoFrame }
}

/** What this webview decodes (the sink's `decode` list). Asked once. */
export async function probeDecode(env: WebCodecsEnv | null): Promise<VideoCodec[]> {
  if (!env) return []
  const out: VideoCodec[] = []
  const ask = async (codec: VideoCodec, config: string): Promise<void> => {
    try {
      const r = await env.VideoDecoder.isConfigSupported({ codec: config, optimizeForLatency: true })
      if (r.supported) out.push(codec)
    } catch (err) {
      // A probe that throws means "no": the plugin then sends decoded frames.
      console.warn(`[video] probing ${config} failed:`, err)
    }
  }
  await ask('vp8', 'vp8')
  await ask('h264', H264_PROBE_CODEC)
  return out
}

/** Where a sink draws. */
export interface FrameSurface {
  draw(frame: VideoFrame): void
  drawRgba(rgba: Uint8ClampedArray<ArrayBuffer>, width: number, height: number): void
}

export function canvasSurface(canvas: HTMLCanvasElement): FrameSurface {
  const ctx = (): CanvasRenderingContext2D => {
    const c = canvas.getContext('2d')
    if (!c) throw new Error('video: the tile canvas has no 2d context')
    return c
  }
  const size = (w: number, h: number): void => {
    if (canvas.width !== w) canvas.width = w
    if (canvas.height !== h) canvas.height = h
  }
  return {
    draw(frame) {
      size(frame.displayWidth, frame.displayHeight)
      ctx().drawImage(frame, 0, 0, frame.displayWidth, frame.displayHeight)
    },
    drawRgba(rgba, width, height) {
      size(width, height)
      ctx().putImageData(new ImageData(rgba, width, height), 0, 0)
    },
  }
}

/** A gap between drawn frames longer than this counts as a freeze (stats only). */
export const FREEZE_MS = 600
/** Frames waiting in the decoder before deltas are dropped (§B9.2). */
export const MAX_DECODE_QUEUE = 3

export interface SinkDecoderOptions {
  env: WebCodecsEnv | null
  /** The tile's surface right now (a re-render may swap the canvas). */
  surface: () => FrameSurface | null
  requestKeyframe: () => void
  /** The first picture is up (the tile stops showing its placeholder). */
  onPicture?: () => void
  now?: () => number
}

export class SinkDecoder {
  readonly stats: SinkStats = { decoded: 0, dropped: 0, freezes: 0, freezeMs: 0 }
  private decoder: VideoDecoder | null = null
  /** The codec string the decoder is configured with. */
  private configured: string | null = null
  /** Deltas are useless until a keyframe. */
  private waitingKey = true
  /** A keyframe was asked for and hasn't come yet: don't ask again. */
  private asked = false
  private lastDrawAt: number | null = null
  private width = 0
  private height = 0
  private pictured = false
  private closed = false

  constructor(private readonly opts: SinkDecoderOptions) {}

  private now(): number {
    return this.opts.now ? this.opts.now() : performance.now()
  }

  /** One raw Channel message. */
  push(data: ArrayBuffer | Uint8Array): void {
    if (this.closed) return
    const { header, payload } = parseFrame(data)
    if (header.width) this.width = header.width
    if (header.height) this.height = header.height
    switch (header.kind) {
      case 'reset':
        this.reset()
        return
      case 'encoded':
        this.decodeEncoded(header, payload)
        return
      case 'i420':
        this.drawI420(header, payload)
        return
      case 'rgba':
        this.drawRgba(payload)
        return
    }
  }

  /** The track came back (unmuted, unpaused, visible again): its next frames
   *  need a keyframe, and the gap wasn't a freeze. */
  resume(): void {
    this.lastDrawAt = null
    this.waitingKey = true
    this.asked = false
    this.askKeyframe()
  }

  /** The track stopped on purpose (muted, hidden): the gap isn't a freeze. */
  hold(): void {
    this.lastDrawAt = null
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    const d = this.decoder
    this.decoder = null
    if (d && d.state !== 'closed') d.close()
  }

  private askKeyframe(): void {
    if (this.asked) return
    this.asked = true
    this.opts.requestKeyframe()
  }

  private reset(): void {
    const d = this.decoder
    if (d && d.state !== 'closed') d.reset()
    this.configured = null
    this.waitingKey = true
  }

  private ensureDecoder(env: WebCodecsEnv): VideoDecoder {
    if (this.decoder && this.decoder.state !== 'closed') return this.decoder
    this.configured = null
    this.decoder = new env.VideoDecoder({
      output: (frame) => this.picture(frame),
      error: (err) => this.failed(err),
    })
    return this.decoder
  }

  /** A decoder error closes it: start over at the next keyframe. */
  private failed(err: unknown): void {
    if (this.closed) return
    console.warn('[video] decoder error, asking for a keyframe:', err)
    this.decoder = null
    this.configured = null
    this.waitingKey = true
    this.asked = false
    this.askKeyframe()
  }

  private decodeEncoded(h: FrameHeader, payload: Uint8Array): void {
    const env = this.opts.env
    if (!env) {
      // The plugin sends encoded frames only to a webview that said it decodes them.
      this.stats.dropped += 1
      return
    }
    if (h.discontinuity) this.reset()
    if (this.waitingKey && !h.keyframe) {
      this.stats.dropped += 1
      this.askKeyframe()
      return
    }
    const decoder = this.ensureDecoder(env)
    if (h.keyframe) {
      const codec = h.codec === 'h264' ? (h264CodecOf(payload) ?? this.configured ?? H264_PROBE_CODEC) : 'vp8'
      if (codec !== this.configured) {
        if (this.configured !== null) decoder.reset()
        decoder.configure({ codec, optimizeForLatency: true })
        this.configured = codec
      }
    } else if (decoder.decodeQueueSize > MAX_DECODE_QUEUE) {
      // Behind: skip to the next keyframe rather than show old pictures late.
      this.stats.dropped += 1
      this.waitingKey = true
      this.askKeyframe()
      return
    }
    try {
      decoder.decode(new env.EncodedVideoChunk({ type: h.keyframe ? 'key' : 'delta', timestamp: h.timestampUs, data: payload }))
    } catch (err) {
      // decode() throws for a chunk the decoder can't take in its state.
      this.stats.dropped += 1
      this.reset()
      this.failed(err)
      return
    }
    if (h.keyframe) {
      this.waitingKey = false
      this.asked = false
    }
  }

  private drawI420(h: FrameHeader, payload: Uint8Array): void {
    const env = this.opts.env
    if (!env || !this.width || !this.height) {
      this.stats.dropped += 1
      return
    }
    const frame = new env.VideoFrame(payload, { format: 'I420', codedWidth: this.width, codedHeight: this.height, timestamp: h.timestampUs })
    this.picture(frame)
  }

  private drawRgba(payload: Uint8Array): void {
    const surface = this.opts.surface()
    if (!surface || !this.width || !this.height) {
      this.stats.dropped += 1
      return
    }
    surface.drawRgba(new Uint8ClampedArray(payload.buffer as ArrayBuffer, payload.byteOffset, payload.byteLength), this.width, this.height)
    this.drew()
  }

  private picture(frame: VideoFrame): void {
    try {
      if (this.closed) return
      const surface = this.opts.surface()
      if (!surface) {
        this.stats.dropped += 1
        return
      }
      surface.draw(frame)
      this.drew()
    } finally {
      frame.close()
    }
  }

  private drew(): void {
    const t = this.now()
    if (this.lastDrawAt !== null && t - this.lastDrawAt > FREEZE_MS) {
      this.stats.freezes += 1
      this.stats.freezeMs += Math.round(t - this.lastDrawAt)
    }
    this.lastDrawAt = t
    this.stats.decoded += 1
    if (!this.pictured) {
      this.pictured = true
      this.opts.onPicture?.()
    }
  }
}
