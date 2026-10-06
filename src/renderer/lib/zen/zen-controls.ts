// prd-zen-mode-v1 Z27, Z28, Z64 and prd-zen-gardens-v1 G24 — the required
// controls: bound by K2, then checked by K2. Two are required (Rosson
// 2026-10-04): the Zen toggle and the Garden switcher. The drag region is
// bound for its behaviour only and never checked.
//
// The page draws its own Zen toggle, Garden switcher and drag region, in
// any form. It hands each element to `bridge.controls.bind(kind, el, gardenId?)`
// and K2 attaches the action itself, so the action can't be faked or
// dropped:
//   - `zen-toggle`: click / Enter / Space → exit Zen (this window off).
//   - `drag-region`: mousedown → `titleBarDragOnMouseDown` (drag, and a
//     double-click zooms).
//   - `garden-switcher`: the page's TRIGGER. K2 attaches no action (the page
//     opens its own dropdown or modal) and never cancels its keys (Enter /
//     Space must still open it); K2 only watches for activation so it can
//     check the options turn up.
//   - `garden-option`: one choice, with its Garden id → switch this window
//     to that Garden.
//
// Then K2 checks, for each of the two required controls:
//   declared  the page's `controls` list names it;
//   present   it is bound and the element is connected;
//   visible   displayed, opacity ≥ 0.3 (through its ancestors), a box of at
//             least 24×24 (120×12 for the drag region), inside the viewport,
//             clear of the stoplights / window controls, and
//             `elementFromPoint` at its centre lands on it or inside it;
//   wired     bound through `bind` (an element marked `data-zen-control`
//             that was never bound is "not wired"), and activating the
//             switcher trigger binds a `garden-option` for every Garden
//             within 1 s. Once every Garden had an option inside that
//             second the activation passed, even if the menu closed again
//             before the second was up; an activation while the options
//             are already bound (the click that closes the menu) passes
//             at once.
// Two failed checks in a row are a failure (Z28): one bad frame mid
// animation isn't.

import { titleBarDragOnMouseDown } from '@/lib/titlebar-drag'
import { ZEN_REQUIRED_CONTROLS, type ZenControlKind } from './zen-page'
import type { ZenControlProblem } from './zen-view'

export type ZenBindKind = ZenControlKind | 'garden-option' | 'drag-region'

export const ZEN_BIND_KINDS: readonly ZenBindKind[] = ['zen-toggle', 'garden-switcher', 'garden-option', 'drag-region']

export interface ZenRect {
  left: number
  top: number
  width: number
  height: number
}

/** Everything the check reads from layout, injectable for tests. */
export interface ZenGeometry {
  rect(el: Element): ZenRect
  style(el: Element): { display: string; visibility: string; opacity: number }
  viewport(): { width: number; height: number }
  elementFromPoint(x: number, y: number): Element | null
}

export const domZenGeometry: ZenGeometry = {
  rect(el) {
    const r = el.getBoundingClientRect()
    return { left: r.left, top: r.top, width: r.width, height: r.height }
  },
  style(el) {
    const s = getComputedStyle(el)
    const o = Number.parseFloat(s.opacity)
    return { display: s.display, visibility: s.visibility, opacity: Number.isFinite(o) ? o : 1 }
  },
  viewport() {
    return { width: window.innerWidth, height: window.innerHeight }
  },
  elementFromPoint(x, y) {
    return typeof document.elementFromPoint === 'function' ? document.elementFromPoint(x, y) : null
  },
}

/** What a bound control does. */
export interface ZenControlActions {
  exit(): void
  selectGarden(id: string): void
  /** Every Garden id, in order (`gardens.list()`). */
  gardenIds(): string[]
}

export interface ZenBinding {
  readonly kind: ZenBindKind
  readonly el: HTMLElement
  readonly gardenId: string | null
}

export const ZEN_WIRING_DEADLINE_MS = 1000

export interface ZenControlRegistry {
  /** Bind `el` as `kind`. Returns the unbind. Throws on a bad kind, or a
   *  `garden-option` without a Garden id (a page bug, loud). */
  bind(kind: ZenBindKind, el: HTMLElement, gardenId?: string): () => void
  bindings(): readonly ZenBinding[]
  /** Set when a switcher activation didn't bind every Garden in time; cleared
   *  by a later activation that did. */
  wiringFailure(): ZenControlKind | null
  dispose(): void
}

const ACTIVATE_KEYS = new Set(['Enter', ' ', 'Spacebar'])

export function createControlRegistry(
  actions: ZenControlActions,
  timers: { setTimeout(fn: () => void, ms: number): unknown; clearTimeout(h: unknown): void } = {
    setTimeout: (fn, ms) => setTimeout(fn, ms),
    clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  },
): ZenControlRegistry {
  const list: Array<ZenBinding & { off(): void }> = []
  let wiring: ZenControlKind | null = null
  let pending: unknown = null

  const optionsCoverGardens = (): boolean => {
    const bound = new Set(
      list.filter((b) => b.kind === 'garden-option' && b.el.isConnected && b.gardenId).map((b) => b.gardenId as string),
    )
    return actions.gardenIds().every((id) => bound.has(id))
  }

  const settle = (): void => {
    if (pending !== null) timers.clearTimeout(pending)
    pending = null
    wiring = null
  }

  const onTriggerActivated = (): void => {
    // The options are bound right now (the menu is open and this is the
    // click that closes it): wired.
    if (optionsCoverGardens()) {
      settle()
      return
    }
    if (pending !== null) timers.clearTimeout(pending)
    pending = timers.setTimeout(() => {
      pending = null
      wiring = optionsCoverGardens() ? null : 'garden-switcher'
    }, ZEN_WIRING_DEADLINE_MS)
  }

  return {
    bind(kind, el, gardenId) {
      if (!ZEN_BIND_KINDS.includes(kind)) throw new Error(`zen controls: unknown control kind "${String(kind)}"`)
      if (!(el instanceof HTMLElement)) throw new Error(`zen controls: bind("${kind}") needs an element`)
      if (kind === 'garden-option' && !gardenId) throw new Error('zen controls: a garden-option needs its Garden id')
      const offs: Array<() => void> = []
      const on = <K extends keyof HTMLElementEventMap>(type: K, fn: (e: HTMLElementEventMap[K]) => void): void => {
        el.addEventListener(type, fn)
        offs.push(() => el.removeEventListener(type, fn))
      }
      const activate = (fn: () => void, own = true): void => {
        on('click', () => fn())
        on('keydown', (e) => {
          if (!ACTIVATE_KEYS.has(e.key)) return
          // K2's own action replaces the key's default (no second click).
          // The switcher's action is the page's: its key must still click.
          if (own) e.preventDefault()
          fn()
        })
      }
      if (kind === 'zen-toggle') activate(() => actions.exit())
      else if (kind === 'garden-option') activate(() => actions.selectGarden(gardenId as string))
      else if (kind === 'garden-switcher') activate(onTriggerActivated, false)
      else {
        on('mousedown', (e) => titleBarDragOnMouseDown(e as unknown as Parameters<typeof titleBarDragOnMouseDown>[0]))
      }
      // Z64: a click on a control never starts a window drag.
      const addedNoDrag = kind !== 'drag-region' && !el.classList.contains('no-drag')
      if (addedNoDrag) el.classList.add('no-drag')
      el.setAttribute('data-zen-bound', kind)
      const binding = {
        kind,
        el,
        gardenId: gardenId ?? null,
        off: () => {
          for (const off of offs) off()
          if (addedNoDrag) el.classList.remove('no-drag')
          el.removeAttribute('data-zen-bound')
        },
      }
      list.push(binding)
      // Every Garden now has an option inside the activation's second: wired.
      if (kind === 'garden-option' && pending !== null && optionsCoverGardens()) settle()
      return () => {
        const i = list.indexOf(binding)
        if (i < 0) return
        list.splice(i, 1)
        binding.off()
      }
    },
    bindings() {
      return list
    },
    wiringFailure() {
      return wiring
    },
    dispose() {
      if (pending !== null) timers.clearTimeout(pending)
      pending = null
      for (const b of list.splice(0)) b.off()
    },
  }
}

const MIN_BUTTON = { width: 24, height: 24 }
/** The drag region's minimum width (CSS px): the template keeps it at least
 *  this wide, and the check's "visible" needs it. */
export const ZEN_DRAG_MIN_WIDTH_PX = 120
const MIN_DRAG = { width: ZEN_DRAG_MIN_WIDTH_PX, height: 12 }
const MIN_OPACITY = 0.3

function intersects(a: ZenRect, b: ZenRect): boolean {
  return a.left < b.left + b.width && b.left < a.left + a.width && a.top < b.top + b.height && b.top < a.top + a.height
}

/** Z27 "visible" for one element. */
export function zenControlVisible(
  el: HTMLElement,
  kind: ZenBindKind,
  geo: ZenGeometry,
  reserved: readonly ZenRect[],
): boolean {
  if (!el.isConnected) return false
  let opacity = 1
  for (let n: Element | null = el; n; n = n.parentElement) {
    const s = geo.style(n)
    if (s.display === 'none') return false
    opacity *= s.opacity
  }
  const own = geo.style(el)
  if (own.visibility === 'hidden' || own.visibility === 'collapse') return false
  if (opacity < MIN_OPACITY) return false
  const r = geo.rect(el)
  const min = kind === 'drag-region' ? MIN_DRAG : MIN_BUTTON
  if (r.width < min.width || r.height < min.height) return false
  const vp = geo.viewport()
  if (r.left < 0 || r.top < 0 || r.left + r.width > vp.width + 0.5 || r.top + r.height > vp.height + 0.5) return false
  if (reserved.some((z) => z.width > 0 && z.height > 0 && intersects(r, z))) return false
  const hit = geo.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)
  return hit !== null && (hit === el || el.contains(hit) || isZenK2Overlay(hit))
}

// K2's own transient Zen overlays (the shortcut cheat sheet, the theme
// picker) sit above the page while open. They are K2's, closed with Esc, and
// never a page hiding its controls, so a control under one still counts as
// visible. Only elements K2 registered count (a page can't claim it).
const k2Overlays = new Set<Element>()

/** Register a K2-drawn Zen overlay element. Returns the unregister. */
export function registerZenK2Overlay(el: Element): () => void {
  k2Overlays.add(el)
  return () => void k2Overlays.delete(el)
}

function isZenK2Overlay(hit: Element): boolean {
  for (const o of k2Overlays) if (o === hit || o.contains(hit)) return true
  return false
}

export type ZenControlCheck = { ok: true } | { ok: false; control: ZenControlKind; problem: ZenControlProblem }

/** One full check of the two required controls (Z27, G24). */
export function checkZenControls(input: {
  declared: readonly string[]
  registry: ZenControlRegistry
  geometry: ZenGeometry
  /** Stoplight / window-control rects the controls must stay clear of. */
  reserved: readonly ZenRect[]
  gardenIds: readonly string[]
  /** The page root, searched for `data-zen-control` elements never bound. */
  root: Element | null
}): ZenControlCheck {
  const { registry, geometry, reserved } = input
  const bound = registry.bindings()
  for (const control of ZEN_REQUIRED_CONTROLS) {
    if (!input.declared.includes(control)) return { ok: false, control, problem: 'undeclared' }
    const marked = input.root ? Array.from(input.root.querySelectorAll(`[data-zen-control="${control}"]`)) : []
    if (marked.some((el) => !el.hasAttribute('data-zen-bound'))) return { ok: false, control, problem: 'not-wired' }
    if (control === 'garden-switcher') {
      const triggers = bound.filter((b) => b.kind === 'garden-switcher')
      const options = bound.filter((b) => b.kind === 'garden-option')
      if (triggers.length === 0 && options.length === 0) {
        return { ok: false, control, problem: marked.length > 0 ? 'not-wired' : 'missing' }
      }
      if (triggers.length > 0) {
        const live = triggers.filter((b) => b.el.isConnected)
        if (live.length === 0) return { ok: false, control, problem: 'missing' }
        if (!live.some((b) => zenControlVisible(b.el, 'garden-switcher', geometry, reserved))) {
          return { ok: false, control, problem: 'invisible' }
        }
      } else {
        // Always-shown Gardens: every Garden has a visible option.
        const live = options.filter((b) => b.el.isConnected)
        const ids = new Set(live.map((b) => b.gardenId))
        if (!input.gardenIds.every((id) => ids.has(id))) return { ok: false, control, problem: 'not-wired' }
        if (!live.every((b) => zenControlVisible(b.el, 'garden-option', geometry, reserved))) {
          return { ok: false, control, problem: 'invisible' }
        }
      }
      if (registry.wiringFailure() === 'garden-switcher') return { ok: false, control, problem: 'not-wired' }
      continue
    }
    const mine = bound.filter((b) => b.kind === control)
    if (mine.length === 0) return { ok: false, control, problem: marked.length > 0 ? 'not-wired' : 'missing' }
    const live = mine.filter((b) => b.el.isConnected)
    if (live.length === 0) return { ok: false, control, problem: 'missing' }
    if (!live.some((b) => zenControlVisible(b.el, control, geometry, reserved))) {
      return { ok: false, control, problem: 'invisible' }
    }
  }
  return { ok: true }
}

/** Z28: two failed checks in a row are a failure. */
export function createControlStreak(onFail: (failure: Extract<ZenControlCheck, { ok: false }>) => void): {
  record(result: ZenControlCheck): void
  failures(): number
} {
  let streak = 0
  let fired = false
  return {
    record(result) {
      if (result.ok) {
        streak = 0
        return
      }
      streak += 1
      if (streak >= 2 && !fired) {
        fired = true
        onFail(result)
      }
    },
    failures() {
      return streak
    },
  }
}
