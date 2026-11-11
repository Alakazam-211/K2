// Zoom on a video tile (Rosson 2026-11-11: "participants can zoom in on the
// video, when things being shared are really small"). Viewer-side only: it
// never changes what the sharer sends or what anyone else sees. Pure: the
// tile (VideoTile's ZoomLayer) holds the view and draws it as one CSS
// transform, `translate(x, y) scale(scale)` from the frame's top left.
//
// The rules:
//  - 1× (fit) to 6×. Wheel and pinch zoom about the pointer: the point under
//    it stays under it. The buttons and keys zoom about the frame's middle.
//  - The picture never leaves the frame. The picture is the video's own
//    rectangle inside the frame (letterboxed, object-fit: contain), not the
//    frame: no panning out into the black bars. While the zoomed picture is
//    still narrower (or shorter) than the frame, it stays centred that way.
//  - A double-click goes from fit to 2× at the click, and from any zoom back
//    to fit.
//  - Back to fit when the share ends or the tile shows another track
//    (`zoomResetKey`).

export const ZOOM_MIN = 1
export const ZOOM_MAX = 6
/** A button press or a key: ×1.5 (fit, 150%, 225%, 338%, 506%, 600%). */
export const ZOOM_STEP = 1.5
/** A double-click from fit. */
export const ZOOM_DOUBLE_CLICK = 2
/** An arrow key pans this share of the frame (Shift: four times as far). */
export const PAN_KEY_FRACTION = 0.1
/** Wheel and pinch: the scale changes by e^(-deltaY × this) per wheel pixel. */
export const WHEEL_ZOOM_RATE = 0.01

export interface Size {
  width: number
  height: number
}

export interface Point {
  x: number
  y: number
}

/** The view: the picture scaled by `scale` from the frame's top left, then
 *  moved by (x, y), in CSS pixels. */
export interface ZoomView {
  scale: number
  x: number
  y: number
}

export const FIT: ZoomView = { scale: 1, x: 0, y: 0 }

/** Anything closer to 1× than this is fit. */
const FIT_EPSILON = 1e-3

export function isZoomed(v: ZoomView): boolean {
  return v.scale > 1 + FIT_EPSILON
}

/** The video's rectangle inside the frame at fit (object-fit: contain). No
 *  image size yet (no frame drawn): the whole frame. */
export function pictureRect(frame: Size, image: Size | null): { left: number; top: number; width: number; height: number } {
  if (!image || image.width <= 0 || image.height <= 0 || frame.width <= 0 || frame.height <= 0) {
    return { left: 0, top: 0, width: Math.max(0, frame.width), height: Math.max(0, frame.height) }
  }
  const k = Math.min(frame.width / image.width, frame.height / image.height)
  const width = image.width * k
  const height = image.height * k
  return { left: (frame.width - width) / 2, top: (frame.height - height) / 2, width, height }
}

function clampAxis(offset: number, scale: number, frame: number, start: number, size: number): number {
  const scaled = size * scale
  // Narrower than the frame: centred on that axis.
  if (scaled <= frame) return (frame - scaled) / 2 - start * scale + 0
  const most = -start * scale // the picture's leading edge at the frame's
  const least = frame - (start + size) * scale // its trailing edge at the frame's
  // `+ 0`: never -0 (an edge at 0 is 0).
  return Math.min(most, Math.max(least, offset)) + 0
}

export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) return ZOOM_MIN
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, scale))
}

/** The view, held to the rules: scale in range, the picture inside the frame,
 *  and fit exactly once it's (nearly) 1×. */
export function clampView(v: ZoomView, frame: Size, image: Size | null): ZoomView {
  const scale = clampScale(v.scale)
  if (scale <= 1 + FIT_EPSILON) return FIT
  const r = pictureRect(frame, image)
  return {
    scale,
    x: clampAxis(v.x, scale, frame.width, r.left, r.width),
    y: clampAxis(v.y, scale, frame.height, r.top, r.height),
  }
}

/** Zoom to `scale` keeping the point `at` (frame pixels) where it is. */
export function zoomTo(v: ZoomView, scale: number, at: Point, frame: Size, image: Size | null): ZoomView {
  const next = clampScale(scale)
  // The picture's own point under `at`, before and after.
  const px = (at.x - v.x) / v.scale
  const py = (at.y - v.y) / v.scale
  return clampView({ scale: next, x: at.x - px * next, y: at.y - py * next }, frame, image)
}

/** Zoom by a factor about `at`. */
export function zoomBy(v: ZoomView, factor: number, at: Point, frame: Size, image: Size | null): ZoomView {
  return zoomTo(v, v.scale * factor, at, frame, image)
}

/** The frame's middle: where the buttons and keys zoom. */
export function frameCentre(frame: Size): Point {
  return { x: frame.width / 2, y: frame.height / 2 }
}

/** One step in (+1) or out (-1), about the frame's middle. */
export function zoomStep(v: ZoomView, dir: 1 | -1, frame: Size, image: Size | null): ZoomView {
  return zoomBy(v, dir === 1 ? ZOOM_STEP : 1 / ZOOM_STEP, frameCentre(frame), frame, image)
}

/** Move the picture by (dx, dy) CSS pixels, held inside the frame. */
export function panBy(v: ZoomView, dx: number, dy: number, frame: Size, image: Size | null): ZoomView {
  if (!isZoomed(v)) return FIT
  return clampView({ scale: v.scale, x: v.x + dx, y: v.y + dy }, frame, image)
}

/** Double-click: fit → 2× at the click; zoomed → fit. */
export function toggleAt(v: ZoomView, at: Point, frame: Size, image: Size | null): ZoomView {
  return isZoomed(v) ? FIT : zoomTo(FIT, ZOOM_DOUBLE_CLICK, at, frame, image)
}

/** A wheel (or pinch, which the webview sends as a wheel with Ctrl) as a
 *  zoom factor. `deltaMode` 1 is lines, 2 pages: turned into pixels first. */
export function wheelFactor(deltaY: number, deltaMode = 0): number {
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY
  return Math.exp(-px * WHEEL_ZOOM_RATE)
}

/** An arrow key as a pan: showing more of what's that way, so the picture
 *  moves the other way. Null: not an arrow. */
export function arrowPan(key: string, frame: Size, far = false): Point | null {
  const k = PAN_KEY_FRACTION * (far ? 4 : 1)
  switch (key) {
    case 'ArrowLeft':
      return { x: frame.width * k, y: 0 }
    case 'ArrowRight':
      return { x: -frame.width * k, y: 0 }
    case 'ArrowUp':
      return { x: 0, y: frame.height * k }
    case 'ArrowDown':
      return { x: 0, y: -frame.height * k }
    default:
      return null
  }
}

/** The zoom as the overlay shows it ("250%"); null at fit (nothing shows). */
export function zoomPercent(v: ZoomView): string | null {
  return isZoomed(v) ? `${Math.round(v.scale * 100)}%` : null
}

/** The CSS transform (origin: the frame's top left). None at fit. */
export function viewTransform(v: ZoomView): string | undefined {
  if (!isZoomed(v)) return undefined
  return `translate(${v.x}px, ${v.y}px) scale(${v.scale})`
}

/** The zoom the tile's sink asks for (video-core `setZoom`): in steps of
 *  1, 2, 4 and 6, so a wheel's every notch doesn't ask the media server for
 *  a new size. */
export function sinkZoom(v: ZoomView): number {
  if (!isZoomed(v)) return 1
  for (const z of [2, 4]) if (v.scale <= z + FIT_EPSILON) return z
  return ZOOM_MAX
}

/** The canvas's picture size: the sink sets it on every frame. Before the
 *  first frame it is the HTML default (300×150), which says nothing. */
export function canvasImage(canvas: { width: number; height: number } | null): Size | null {
  if (!canvas || canvas.width <= 0 || canvas.height <= 0) return null
  if (canvas.width === 300 && canvas.height === 150) return null
  return { width: canvas.width, height: canvas.height }
}

/** What the zoom belongs to: another track, or the share ending (or never
 *  having a picture), starts again at fit. */
export function zoomResetKey(track: string, state: string): string {
  const ended = state === 'off' || state === 'unsupported'
  return `${track}\n${ended ? 'ended' : 'on'}`
}

/** A view kept with the key it was made for. */
export interface KeptView {
  key: string
  view: ZoomView
}

/** The view for `key` now: what was kept for it, else fit. */
export function viewFor(kept: KeptView, key: string): ZoomView {
  return kept.key === key ? kept.view : FIT
}
