// The dev video demo's frames: an animated test card per tile, encoded to VP8
// by this webview's own WebCodecs `VideoEncoder`, then framed as Sew's plugin
// would send it. It runs the whole receive path (frame header, decoder,
// keyframe requests, canvas) with no plugin and no camera.

import type { FrameSource } from './fake-video-engine'
import { encodeFrame } from './video-frame'

export interface SyntheticOptions {
  label: string
  /** CSS color of the card. */
  color: string
  width?: number
  height?: number
  fps?: number
}

/** True where this webview can encode VP8 (the demo needs it). */
export function canMakeSyntheticVideo(): boolean {
  const g = globalThis as { VideoEncoder?: unknown; VideoFrame?: unknown; OffscreenCanvas?: unknown }
  return typeof g.VideoEncoder === 'function' && typeof g.VideoFrame === 'function' && typeof g.OffscreenCanvas === 'function'
}

export function syntheticVp8Source(opts: SyntheticOptions): FrameSource {
  const width = opts.width ?? 640
  const height = opts.height ?? 360
  const fps = opts.fps ?? 15
  let timer: ReturnType<typeof setInterval> | null = null
  let encoder: VideoEncoder | null = null
  let wantKey = true
  let n = 0
  return {
    start(sink, send) {
      const canvas = new OffscreenCanvas(width, height)
      const ctx = canvas.getContext('2d')
      if (!ctx) throw new Error('synthetic video: no 2d context')
      let seq = 0
      encoder = new VideoEncoder({
        output: (chunk) => {
          const data = new Uint8Array(chunk.byteLength)
          chunk.copyTo(data)
          seq += 1
          send(
            encodeFrame(
              {
                kind: 'encoded',
                codec: 'vp8',
                keyframe: chunk.type === 'key',
                selfView: sink.selfView,
                sinkId: sink.sinkId,
                timestampUs: chunk.timestamp,
                width,
                height,
                seq,
              },
              data,
            ),
          )
        },
        error: (err) => console.error('[video demo] encoder error:', err),
      })
      encoder.configure({ codec: 'vp8', width, height, bitrate: 600_000, framerate: fps, latencyMode: 'realtime' })
      timer = setInterval(() => {
        if (!encoder || encoder.state !== 'configured' || encoder.encodeQueueSize > 2) return
        n += 1
        ctx.fillStyle = opts.color
        ctx.fillRect(0, 0, width, height)
        const x = ((n * 6) % (width + 120)) - 60
        ctx.fillStyle = 'rgba(255,255,255,0.85)'
        ctx.fillRect(x, height / 2 - 30, 60, 60)
        ctx.fillStyle = '#fff'
        ctx.font = '28px system-ui, sans-serif'
        ctx.fillText(opts.label, 24, 48)
        ctx.font = '16px ui-monospace, monospace'
        ctx.fillText(`frame ${n}`, 24, height - 24)
        const frame = new VideoFrame(canvas, { timestamp: Math.round((n * 1_000_000) / fps) })
        encoder.encode(frame, { keyFrame: wantKey || n % (fps * 4) === 0 })
        wantKey = false
        frame.close()
      }, 1000 / fps)
    },
    keyframe() {
      wantKey = true
    },
    stop() {
      if (timer) clearInterval(timer)
      timer = null
      if (encoder && encoder.state !== 'closed') encoder.close()
      encoder = null
    },
  }
}
