// The Desktop tab's input rules (prd-remote-desktop-tab-v1 §6, §8), mirrored
// from `crates/k2-core/src/desktop/input.rs`: caps, the rate budget,
// coalescing, the text check, pointer mapping, the Mac ↔ PC modifier flip
// and the held-keys set (R3). The daemon applies the same rules again on
// every message; this side applies them first so a refusal is rare.
//
// Pure (R14): no React, no DOM, no `window`. Time comes in as a number.

import { isKnownKey } from './desktop-keymap'
import type { Button, ViewerMsg } from './desktop-wire'

/** One `text` message (R12). */
export const TEXT_MAX_CHARS = 256
/** One `clipboard-set` (UTF-8 bytes). */
export const CLIPBOARD_MAX_BYTES = 64 * 1024

export const POINTER_PER_SEC = 240
export const POINTER_BURST = 240
export const KEYS_BURST = 120
export const KEYS_PER_SEC = 60
export const TEXTS_BURST = 4
export const TEXTS_PER_SEC = 4
export const CLIPBOARD_BURST = 1
export const CLIPBOARD_PER_SEC = 1
export const WHEEL_BURST = 240
export const WHEEL_PER_SEC = 240

export interface InputRefusal {
  code: 'desktop_invalid' | 'desktop_rate_limited'
  message: string
}

const invalid = (message: string): InputRefusal => ({ code: 'desktop_invalid', message })

// Any C0/C1 control (Rust's `char::is_control`).
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/
// eslint-disable-next-line no-control-regex
const CONTROL_BUT_LINES = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/

function charCount(s: string): number {
  return Array.from(s).length
}

function utf8Bytes(s: string): number {
  return new TextEncoder().encode(s).length
}

/** Size and content checks; `null` when the message may go. */
export function checkInput(m: ViewerMsg): InputRefusal | null {
  switch (m.t) {
    case 'pointer':
      if (!Number.isFinite(m.x) || !Number.isFinite(m.y) || m.x < 0 || m.y < 0) {
        return invalid('A pointer position must be a point on the display.')
      }
      return null
    case 'wheel':
      if (!Number.isFinite(m.dx) || !Number.isFinite(m.dy)) return invalid('A wheel step must be a number.')
      return null
    case 'key':
      return isKnownKey(m.code) ? null : invalid(`Unknown key '${m.code}'.`)
    case 'text': {
      const n = charCount(m.text)
      if (n === 0) return invalid('Empty text.')
      if (n > TEXT_MAX_CHARS) return invalid(`Typed text is over ${TEXT_MAX_CHARS} characters; send it as clipboard text.`)
      if (CONTROL.test(m.text)) {
        return invalid("Typed text can't hold control characters; send Enter, Tab and Backspace as keys.")
      }
      return null
    }
    case 'clipboard-set':
      if (m.text.length === 0) return invalid('The clipboard has no text.')
      if (utf8Bytes(m.text) > CLIPBOARD_MAX_BYTES) {
        return invalid(`The clipboard text is over ${CLIPBOARD_MAX_BYTES / 1024} KB; send less at a time.`)
      }
      if (CONTROL_BUT_LINES.test(m.text)) return invalid('The clipboard text holds control characters.')
      return null
    case 'display':
      if (m.id.trim() === '' || m.id.length > 128) return invalid('A display id is required.')
      return null
    default:
      return null
  }
}

/** Split long typed text into `text` messages of at most
 *  {@link TEXT_MAX_CHARS} characters (never inside a surrogate pair). */
export function splitText(text: string): string[] {
  const chars = Array.from(text)
  const out: string[] = []
  for (let i = 0; i < chars.length; i += TEXT_MAX_CHARS) out.push(chars.slice(i, i + TEXT_MAX_CHARS).join(''))
  return out
}

class Bucket {
  private tokens: number
  private at: number
  constructor(
    private readonly cap: number,
    private readonly perSec: number,
    now: number,
  ) {
    this.tokens = cap
    this.at = now
  }
  take(now: number): boolean {
    const dt = Math.max(0, now - this.at) / 1000
    this.at = Math.max(this.at, now)
    this.tokens = Math.min(this.cap, this.tokens + dt * this.perSec)
    if (this.tokens >= 1) {
      this.tokens -= 1
      return true
    }
    return false
  }
}

/** One viewer's input rate limit (`now` in ms). A key-up and a button-up
 *  are always free: dropping a release would leave a key held. */
export class InputBudget {
  private readonly pointer: Bucket
  private readonly keys: Bucket
  private readonly texts: Bucket
  private readonly clipboard: Bucket
  private readonly wheel: Bucket
  constructor(now: number) {
    this.pointer = new Bucket(POINTER_BURST, POINTER_PER_SEC, now)
    this.keys = new Bucket(KEYS_BURST, KEYS_PER_SEC, now)
    this.texts = new Bucket(TEXTS_BURST, TEXTS_PER_SEC, now)
    this.clipboard = new Bucket(CLIPBOARD_BURST, CLIPBOARD_PER_SEC, now)
    this.wheel = new Bucket(WHEEL_BURST, WHEEL_PER_SEC, now)
  }
  take(m: ViewerMsg, now: number): InputRefusal | null {
    let ok = true
    switch (m.t) {
      case 'pointer':
        ok = this.pointer.take(now)
        break
      case 'key':
      case 'button':
        ok = m.down ? this.keys.take(now) : true
        break
      case 'text':
        ok = this.texts.take(now)
        break
      case 'clipboard-set':
      case 'clipboard-get':
        ok = this.clipboard.take(now)
        break
      case 'wheel':
        ok = this.wheel.take(now)
        break
      default:
        ok = true
    }
    return ok ? null : { code: 'desktop_rate_limited', message: 'Sending input faster than the desktop takes it; slow down.' }
  }
}

/** Consecutive `pointer` messages collapse to the last; consecutive
 *  same-mode `wheel` messages sum. Never across anything else. */
export function coalesce(batch: readonly ViewerMsg[]): ViewerMsg[] {
  const out: ViewerMsg[] = []
  for (const m of batch) {
    const last = out[out.length - 1]
    if (last && last.t === 'pointer' && m.t === 'pointer') {
      out[out.length - 1] = { t: 'pointer', x: m.x, y: m.y }
    } else if (last && last.t === 'wheel' && m.t === 'wheel' && last.mode === m.mode) {
      out[out.length - 1] = { t: 'wheel', dx: last.dx + m.dx, dy: last.dy + m.dy, mode: m.mode }
    } else {
      out.push(m)
    }
  }
  return out
}

export type Fit = 'fit' | '1:1'

export interface PointerMapInput {
  /** Where the event landed, in CSS pixels from the canvas's top left. */
  cx: number
  cy: number
  /** The canvas's CSS size. */
  canvasW: number
  canvasH: number
  /** The display in points. */
  widthPt: number
  heightPt: number
  fit: Fit
  /** 1:1 only: the scroll offset of the canvas inside its frame (CSS px),
   *  and CSS pixels per display point (devicePixelRatio / scale on 1:1). */
  scrollX?: number
  scrollY?: number
  cssPerPt?: number
}

/** A canvas point → the display's points (origin top left), or `null`
 *  outside the picture (fit's letterbox bands). */
export function mapPointer(p: PointerMapInput): { x: number; y: number } | null {
  if (p.widthPt <= 0 || p.heightPt <= 0 || p.canvasW <= 0 || p.canvasH <= 0) return null
  if (p.fit === '1:1') {
    const per = p.cssPerPt && p.cssPerPt > 0 ? p.cssPerPt : 1
    const x = (p.cx + (p.scrollX ?? 0)) / per
    const y = (p.cy + (p.scrollY ?? 0)) / per
    if (x < 0 || y < 0 || x > p.widthPt || y > p.heightPt) return null
    return { x, y }
  }
  // Fit: the picture is scaled to fit and centred.
  const s = Math.min(p.canvasW / p.widthPt, p.canvasH / p.heightPt)
  const drawnW = p.widthPt * s
  const drawnH = p.heightPt * s
  const ox = (p.canvasW - drawnW) / 2
  const oy = (p.canvasH - drawnH) / 2
  const x = (p.cx - ox) / s
  const y = (p.cy - oy) / s
  if (x < 0 || y < 0 || x > p.widthPt || y > p.heightPt) return null
  return { x, y }
}

/** Where the picture sits in the canvas's CSS box: display point (x, y)
 *  is at CSS (ox + x·scale, oy + y·scale). Fit: scaled to fit and centred
 *  (the canvas is `object-contain`); 1:1: `cssPerPt` per point from the top
 *  left (the canvas is its own size inside a scrolled frame). */
export interface PictureLayout {
  scale: number
  ox: number
  oy: number
}

export function pictureLayout(p: { canvasW: number; canvasH: number; widthPt: number; heightPt: number; fit: Fit; cssPerPt?: number }): PictureLayout | null {
  if (p.widthPt <= 0 || p.heightPt <= 0 || p.canvasW <= 0 || p.canvasH <= 0) return null
  if (p.fit === '1:1') return { scale: p.cssPerPt && p.cssPerPt > 0 ? p.cssPerPt : 1, ox: 0, oy: 0 }
  const s = Math.min(p.canvasW / p.widthPt, p.canvasH / p.heightPt)
  return { scale: s, ox: (p.canvasW - p.widthPt * s) / 2, oy: (p.canvasH - p.heightPt * s) / 2 }
}

/** CSS pixels per display point at 1:1: one display pixel per device pixel
 *  (`widthPx / widthPt` pixels per point, over the viewer's DPR). */
export function oneToOneCssPerPt(widthPx: number, widthPt: number, dpr: number): number {
  if (widthPx <= 0 || widthPt <= 0) return 1
  const d = Number.isFinite(dpr) && dpr > 0 ? dpr : 1
  return widthPx / widthPt / d
}

/** A wheel event's mode on the wire (`WheelEvent.deltaMode`: 0 = pixels;
 *  lines and pages go as lines). */
export function wheelMode(deltaMode: number): 'pixel' | 'line' {
  return deltaMode === 0 ? 'pixel' : 'line'
}

/** `PointerEvent.button` → the wire's button, or null. */
export function pointerButton(b: number): Button | null {
  switch (b) {
    case 0:
      return 'left'
    case 1:
      return 'middle'
    case 2:
      return 'right'
    case 3:
      return 'back'
    case 4:
      return 'forward'
    default:
      return null
  }
}

/** Mac ↔ PC modifiers: ⌘ and Ctrl trade places (the daemon's
 *  `keymap::flip_modifier`). */
export function flipModifier(code: string): string {
  switch (code) {
    case 'MetaLeft':
      return 'ControlLeft'
    case 'MetaRight':
      return 'ControlRight'
    case 'ControlLeft':
      return 'MetaLeft'
    case 'ControlRight':
      return 'MetaRight'
    default:
      return code
  }
}

/** The code to send: flipped when the toolbar's Mac ↔ PC switch is on and
 *  the viewer's OS family differs from the host's. */
export function keyCodeToSend(code: string, viewerIsMac: boolean, hostIsMac: boolean, flip: boolean): string {
  return flip && viewerIsMac !== hostIsMac ? flipModifier(code) : code
}

type Held = { kind: 'key'; code: string } | { kind: 'button'; button: Button }

function heldKey(h: Held): string {
  return h.kind === 'key' ? `k:${h.code}` : `b:${h.button}`
}

/** What a viewer holds down, in press order (R3): every drop releases it,
 *  newest press first, once. */
export class HeldInput {
  private order: Held[] = []
  note(m: ViewerMsg): void {
    let h: Held
    let down: boolean
    if (m.t === 'key') {
      h = { kind: 'key', code: m.code }
      down = m.down
    } else if (m.t === 'button') {
      h = { kind: 'button', button: m.button }
      down = m.down
    } else {
      return
    }
    const k = heldKey(h)
    const at = this.order.findIndex((x) => heldKey(x) === k)
    if (down) {
      if (at < 0) this.order.push(h)
    } else if (at >= 0) {
      this.order.splice(at, 1)
    }
  }
  isEmpty(): boolean {
    return this.order.length === 0
  }
  releaseAll(): ViewerMsg[] {
    const out: ViewerMsg[] = [...this.order]
      .reverse()
      .map((h) => (h.kind === 'key' ? { t: 'key', code: h.code, down: false } : { t: 'button', button: h.button, down: false }))
    this.order = []
    return out
  }
}
