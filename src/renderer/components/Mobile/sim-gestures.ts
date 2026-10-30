// Trackpad and mouse gestures on the device canvas (0.46.7, Rosson
// 2026-10-30: "green light there on gestures with momentum"). Pure, so the
// rules are tested without a canvas; MobilePane wires them to events.
//
//  - Flick velocity: a live-touch drag's `up` carries the release velocity
//    (points per second). The daemon replays the flick's tail on the
//    device's clock (`k2-core::sim::gesture`), so lists coast like on a
//    phone however the moves were bunched in transit.
//  - Wheel → drag: two-finger trackpad scroll and the mouse wheel become a
//    one-finger drag on the device (touch down, move…, up), coalesced per
//    animation frame by the pane. The finger moves against the scroll delta
//    (content follows it 1:1 on the canvas). The finger stays inside a safe
//    band away from the edges, where the phone's own edge gestures live
//    (Home, notifications, Back); at the band's edge it lifts and grips
//    again on the far side, like a person scrolling a long list. macOS's
//    own momentum keeps sending wheel deltas after the fingers lift, so the
//    drag coasts with it; the gesture ends when the wheel goes quiet.
//  - Pinch: trackpad pinch (WebKit `gesture*` events, or Ctrl+wheel) and
//    ⌥-drag (like Xcode's Simulator: the second finger mirrored around the
//    screen's centre) become two fingers. Only where `hello.input.multiTouch`
//    says the device takes them; otherwise the pane says so and sends
//    nothing (never one finger instead).

import { canvasPointToDevice, type BoxRect, type PointerSample, type ScreenPt } from './sim-input'
import type { SimInput, SimPoint } from './sim-stream'

const round1 = (v: number): number => Math.round(v * 10) / 10
const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v))

// ── Flick velocity ──────────────────────────────────────────────────────

/** Samples older than this before the release don't count. */
export const VELOCITY_WINDOW_MS = 100
/** Still this long before lifting: a stop, not a flick. */
export const STOPPED_MS = 50
/** Below this (pt/s) the release is a stop (matches the daemon). */
export const MIN_FLING_PT_S = 60

/** Keep only the samples a release velocity can use. */
export function trimSamples(samples: PointerSample[], now: number): PointerSample[] {
  return samples.filter((s) => now - s.at <= VELOCITY_WINDOW_MS)
}

/** The release velocity in points per second, or null for a stop. */
export function releaseVelocity(samples: PointerSample[], upAt: number, box: BoxRect, screen: ScreenPt): { vx: number; vy: number } | null {
  const recent = trimSamples(samples, upAt)
  if (recent.length < 2) return null
  const first = recent[0]
  const last = recent[recent.length - 1]
  if (upAt - last.at > STOPPED_MS) return null
  const dt = last.at - first.at
  if (dt <= 0 || box.width <= 0 || box.height <= 0) return null
  const vx = (((last.clientX - first.clientX) * screen.widthPt) / box.width / dt) * 1000
  const vy = (((last.clientY - first.clientY) * screen.heightPt) / box.height / dt) * 1000
  if (Math.hypot(vx, vy) < MIN_FLING_PT_S) return null
  return { vx: round1(vx), vy: round1(vy) }
}

/** A live-touch `up` with the flick's velocity (unchanged for a stop). */
export function withVelocity(up: SimInput, v: { vx: number; vy: number } | null): SimInput {
  if (!v || up.t !== 'touch' || up.phase !== 'up') return up
  return { ...up, vx: v.vx, vy: v.vy }
}

// ── The safe band ───────────────────────────────────────────────────────

/** Share of the width / height kept clear on each side for the phone's
 *  edge gestures (Back from the sides, notifications from the top, Home
 *  from the bottom). */
export const SAFE_X = 0.15
export const SAFE_Y = 0.2

export interface SafeBand {
  minX: number
  maxX: number
  minY: number
  maxY: number
}

export function safeBand(screen: ScreenPt): SafeBand {
  return {
    minX: screen.widthPt * SAFE_X,
    maxX: screen.widthPt * (1 - SAFE_X),
    minY: screen.heightPt * SAFE_Y,
    maxY: screen.heightPt * (1 - SAFE_Y),
  }
}

function intoBand(p: SimPoint, b: SafeBand): SimPoint {
  return { x: clamp(p.x, b.minX, b.maxX), y: clamp(p.y, b.minY, b.maxY) }
}

const touch = (phase: 'down' | 'move' | 'up', p: SimPoint): SimInput => ({ t: 'touch', phase, x: round1(p.x), y: round1(p.y) })

// ── Wheel → drag ────────────────────────────────────────────────────────

/** The wheel quiet this long ends the drag (lifts the finger). */
export const WHEEL_IDLE_MS = 120
/** Pixels per line for `deltaMode` 1 (Firefox, some mice). */
export const WHEEL_LINE_PX = 16
/** Grips per frame at most (a huge delta loses the rest, never loops). */
const MAX_REGRIPS = 4

export interface WheelLike {
  deltaX: number
  deltaY: number
  /** 0 pixels, 1 lines, 2 pages. */
  deltaMode: number
}

/** A wheel event's delta in CSS pixels. */
export function wheelDeltaPx(e: WheelLike, box: BoxRect): { dx: number; dy: number } {
  const unit = e.deltaMode === 1 ? WHEEL_LINE_PX : e.deltaMode === 2 ? box.height : 1
  return { dx: e.deltaX * unit, dy: e.deltaY * unit }
}

/** CSS pixels on the canvas → device points. */
export function cssToPoints(d: { dx: number; dy: number }, box: BoxRect, screen: ScreenPt): { dx: number; dy: number } {
  return {
    dx: box.width > 0 ? (d.dx * screen.widthPt) / box.width : 0,
    dy: box.height > 0 ? (d.dy * screen.heightPt) / box.height : 0,
  }
}

export interface WheelDrag {
  /** Where the synthetic finger is, in points. */
  finger: SimPoint
}

/** The first wheel delta: the finger goes down under the pointer (inside
 *  the safe band). */
export function wheelBegin(at: SimPoint, screen: ScreenPt): { drag: WheelDrag; send: SimInput[] } {
  const finger = intoBand(at, safeBand(screen))
  return { drag: { finger }, send: [touch('down', finger)] }
}

/** One frame's summed scroll delta (points): the finger moves against it,
 *  lifting and gripping again at the safe band's edge. */
export function wheelMove(drag: WheelDrag, delta: { dx: number; dy: number }, screen: ScreenPt): { drag: WheelDrag; send: SimInput[] } {
  const band = safeBand(screen)
  const send: SimInput[] = []
  let finger = drag.finger
  let rx = -delta.dx
  let ry = -delta.dy
  for (let grip = 0; grip <= MAX_REGRIPS; grip++) {
    const target = { x: finger.x + rx, y: finger.y + ry }
    const at = intoBand(target, band)
    if (at.x !== finger.x || at.y !== finger.y) send.push(touch('move', at))
    rx = target.x - at.x
    ry = target.y - at.y
    finger = at
    if (Math.abs(rx) < 0.05 && Math.abs(ry) < 0.05) break
    if (grip === MAX_REGRIPS) break
    // Out of room: lift, and grip again on the far side of the band.
    send.push(touch('up', finger))
    finger = {
      x: Math.abs(rx) >= 0.05 ? (rx < 0 ? band.maxX : band.minX) : finger.x,
      y: Math.abs(ry) >= 0.05 ? (ry < 0 ? band.maxY : band.minY) : finger.y,
    }
    send.push(touch('down', finger))
  }
  return { drag: { finger }, send }
}

/** The wheel went quiet: lift where the finger is. Momentum already came
 *  in as wheel deltas, so the lift is still (no fling of its own). */
export function wheelEnd(drag: WheelDrag): SimInput[] {
  return [touch('up', drag.finger)]
}

// ── Pinch ───────────────────────────────────────────────────────────────

export const PINCH_UNAVAILABLE = "Pinch isn't available on this device yet."
/** Ctrl+wheel (Chrome-style pinch) quiet this long ends the pinch. */
export const PINCH_IDLE_MS = 150

export interface Pinch {
  /** The fingers' midpoint, in points. */
  center: SimPoint
  /** Finger spread at scale 1, in points. */
  spread0: number
  /** Cumulative scale since the pinch started. */
  scale: number
}

/** Start a trackpad pinch around `at`: two fingers a quarter of the short
 *  side apart, side by side. */
export function pinchBegin(at: SimPoint, screen: ScreenPt): Pinch {
  return { center: intoBand(at, safeBand(screen)), spread0: Math.min(screen.widthPt, screen.heightPt) * 0.25, scale: 1 }
}

/** Ctrl+wheel delta → the new cumulative scale (`exp(-dy / 100)` per step). */
export function pinchWheelScale(p: Pinch, deltaYPx: number): Pinch {
  return { ...p, scale: p.scale * Math.exp(-deltaYPx / 100) }
}

/** The two fingers for a pinch's current scale, kept on the screen. */
export function pinchPoints(p: Pinch, screen: ScreenPt): [SimPoint, SimPoint] {
  const spread = clamp(p.spread0 * p.scale, 20, screen.widthPt * 0.9)
  const x1 = clamp(p.center.x - spread / 2, 1, screen.widthPt - 1)
  const x2 = clamp(p.center.x + spread / 2, 1, screen.widthPt - 1)
  const y = clamp(p.center.y, 1, screen.heightPt - 1)
  return [
    { x: round1(x1), y: round1(y) },
    { x: round1(x2), y: round1(y) },
  ]
}

/** ⌥-drag: the second finger mirrored around the screen's centre (Xcode's
 *  Simulator does the same). */
export function mirrorPoint(p: SimPoint, screen: ScreenPt): SimPoint {
  return { x: round1(screen.widthPt - p.x), y: round1(screen.heightPt - p.y) }
}

/** Two fingers in one `touch` message. */
export function twoFingers(phase: 'down' | 'move' | 'up', a: SimPoint, b: SimPoint): SimInput {
  return { t: 'touch', phase, x: round1(a.x), y: round1(a.y), x2: round1(b.x), y2: round1(b.y) }
}

/** ⌥-drag at a canvas point → both fingers. */
export function altDragFingers(
  phase: 'down' | 'move' | 'up',
  clientX: number,
  clientY: number,
  box: BoxRect,
  screen: ScreenPt,
): SimInput {
  const a = canvasPointToDevice(clientX, clientY, box, screen)
  return twoFingers(phase, a, mirrorPoint(a, screen))
}
