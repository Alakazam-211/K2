// The player's main-thread canvas sink (E9): where the worker player can't
// run (no OffscreenCanvas, a worker that failed, the `k2.stream.player`
// switch) the pane draws on its own <canvas> with these. The worker does the
// same in `stream-player.worker.ts`.

import type { PixelRgb } from './stream-player.worker'
import type { VideoFrameLike } from './stream-video'

/** Decode a JPEG and draw it at its own size. False without a 2d context
 *  (`onDecoded` runs between decode and draw, for the latency readout). */
export async function drawJpeg(canvas: HTMLCanvasElement, data: Blob, onDecoded?: () => void): Promise<boolean> {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  const bmp = await createImageBitmap(data)
  onDecoded?.()
  try {
    if (canvas.width !== bmp.width) canvas.width = bmp.width
    if (canvas.height !== bmp.height) canvas.height = bmp.height
    ctx.drawImage(bmp, 0, 0)
  } finally {
    bmp.close()
  }
  return true
}

/** Draw a decoded video frame, cropped to the header's size (the coded
 *  picture may be one pixel wider / taller: H.264 pads odd sizes). */
export function drawVideoFrame(canvas: HTMLCanvasElement, frame: VideoFrameLike, widthPx: number, heightPx: number): boolean {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  const w = Math.min(widthPx, frame.displayWidth)
  const h = Math.min(heightPx, frame.displayHeight)
  if (canvas.width !== w) canvas.width = w
  if (canvas.height !== h) canvas.height = h
  ctx.drawImage(frame as unknown as CanvasImageSource, 0, 0, w, h, 0, 0, w, h)
  return true
}

/** A small JPEG of what the canvas shows, or null when it can't export
 *  (no frame, jsdom). */
export function canvasThumb(canvas: HTMLCanvasElement | null): string | null {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return null
  try {
    const url = canvas.toDataURL('image/jpeg', 0.6)
    return typeof url === 'string' && url.startsWith('data:image/') ? url : null
  } catch {
    return null
  }
}

/** The canvas's centre pixel, or null without a 2d context (jsdom). */
export function canvasCentre(canvas: HTMLCanvasElement | null): PixelRgb | null {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return null
  const ctx = canvas.getContext('2d', { willReadFrequently: true })
  if (!ctx) return null
  const d = ctx.getImageData(canvas.width >> 1, canvas.height >> 1, 1, 1).data
  return [d[0], d[1], d[2]]
}
