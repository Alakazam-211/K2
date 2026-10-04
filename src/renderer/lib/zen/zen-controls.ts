// prd-zen-mode-v1 Z27, Z28, Z64 — the required controls: bound by K2, then
// checked by K2.
//
// The page draws its own Zen toggle, Home switcher and drag region, in any
// form. It hands each element to `bridge.controls.bind(kind, el, homeId?)`
// and K2 attaches the action itself, so the action can't be faked or
// dropped:
//   - `zen-toggle`: click / Enter / Space → exit Zen (this Home off).
//   - `drag-region`: mousedown → `titleBarDragOnMouseDown` (drag, and a
//     double-click zooms).
//   - `home-switcher`: the page's TRIGGER. K2 attaches no action (the page
//     opens its own dropdown or modal); K2 only watches for activation so
//     it can check the options turn up.
//   - `home-option`: one choice, with its Home id → select that Home.
//
// Then K2 checks, for each required control:
//   declared  the page's `controls` list names it;
//   present   it is bound and the element is connected;
//   visible   displayed, opacity ≥ 0.3 (through its ancestors), a box of at
//             least 24×24 (120×12 for the drag region), inside the viewport,
//             clear of the stoplights / window controls, and
//             `elementFromPoint` at its centre lands on it or inside it;
//   wired     bound through `bind` (an element marked `data-zen-control`
//             that was never bound is "not wired"), and activating the
//             switcher trigger binds a `home-option` for every Home within
//             1 s.
// Two failed checks in a row are a failure (Z28): one bad frame mid
// animation isn't.

import { titleBarDragOnMouseDown } from '@/lib/titlebar-drag'
import { ZEN_REQUIRED_CONTROLS, type ZenControlKind } from './zen-page'
import type { ZenControlProblem } from './zen-view'

export type ZenBindKind = ZenControlKind | 'home-option'

export const ZEN_BIND_KINDS: readonly ZenBindKind[] = ['zen-toggle', 'home-switcher', 'home-option', 'drag-region']

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
  selectHome(id: string): void
  /** Every Home id, in order (`homes.list()`). */
  homeIds(): string[]
}

export interface ZenBinding {
  readonly kind: ZenBindKind
  readonly el: HTMLElement
  readonly homeId: string | null
}

export const ZEN_WIRING_DEADLINE_MS = 1000

export interface ZenControlRegistry {
  /** Bind `el` as `kind`. Returns the unbind. Throws on a bad kind, or a
   *  `home-option` without a Home id (a page bug, loud). */
  bind(kind: ZenBindKind, el: HTMLElement, homeId?: string): () => void
  bindings(): readonly ZenBinding[]
  /** Set when a switcher activation didn't bind every Home in time; cleared
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

  const optionsCoverHomes = (): boolean => {
    const bound = new Set(
      list.filter((b) => b.kind === 'home-option' && b.el.isConnected && b.homeId).map((b) => b.homeId as string),
    )
    return actions.homeIds().every((id) => bound.has(id))
  }

  const onTriggerActivated = (): void => {
    if (pending !== null) timers.clearTimeout(pending)
    pending = timers.setTimeout(() => {
      pending = null
      wiring = optionsCoverHomes() ? null : 'home-switcher'
    }, ZEN_WIRING_DEADLINE_MS)
  }

  return {
    bind(kind, el, homeId) {
      if (!ZEN_BIND_KINDS.includes(kind)) throw new Error(`zen controls: unknown control kind "${String(kind)}"`)
      if (!(el instanceof HTMLElement)) throw new Error(`zen controls: bind("${kind}") needs an element`)
      if (kind === 'home-option' && !homeId) throw new Error('zen controls: a home-option needs its Home id')
      const offs: Array<() => void> = []
      const on = <K extends keyof HTMLElementEventMap>(type: K, fn: (e: HTMLElementEventMap[K]) => void): void => {
        el.addEventListener(type, fn)
        offs.push(() => el.removeEventListener(type, fn))
      }
      const activate = (fn: () => void): void => {
        on('click', () => fn())
        on('keydown', (e) => {
          if (!ACTIVATE_KEYS.has(e.key)) return
          e.preventDefault()
          fn()
        })
      }
      if (kind === 'zen-toggle') activate(() => actions.exit())
      else if (kind === 'home-option') activate(() => actions.selectHome(homeId as string))
      else if (kind === 'home-switcher') activate(onTriggerActivated)
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
        homeId: homeId ?? null,
        off: () => {
          for (const off of offs) off()
          if (addedNoDrag) el.classList.remove('no-drag')
          el.removeAttribute('data-zen-bound')
        },
      }
      list.push(binding)
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
const MIN_DRAG = { width: 120, height: 12 }
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
  return hit !== null && (hit === el || el.contains(hit))
}

export type ZenControlCheck = { ok: true } | { ok: false; control: ZenControlKind; problem: ZenControlProblem }

/** One full check of the three required controls (Z27). */
export function checkZenControls(input: {
  declared: readonly string[]
  registry: ZenControlRegistry
  geometry: ZenGeometry
  /** Stoplight / window-control rects the controls must stay clear of. */
  reserved: readonly ZenRect[]
  homeIds: readonly string[]
  /** The page root, searched for `data-zen-control` elements never bound. */
  root: Element | null
}): ZenControlCheck {
  const { registry, geometry, reserved } = input
  const bound = registry.bindings()
  for (const control of ZEN_REQUIRED_CONTROLS) {
    if (!input.declared.includes(control)) return { ok: false, control, problem: 'undeclared' }
    const marked = input.root ? Array.from(input.root.querySelectorAll(`[data-zen-control="${control}"]`)) : []
    if (marked.some((el) => !el.hasAttribute('data-zen-bound'))) return { ok: false, control, problem: 'not-wired' }
    if (control === 'home-switcher') {
      const triggers = bound.filter((b) => b.kind === 'home-switcher')
      const options = bound.filter((b) => b.kind === 'home-option')
      if (triggers.length === 0 && options.length === 0) {
        return { ok: false, control, problem: marked.length > 0 ? 'not-wired' : 'missing' }
      }
      if (triggers.length > 0) {
        const live = triggers.filter((b) => b.el.isConnected)
        if (live.length === 0) return { ok: false, control, problem: 'missing' }
        if (!live.some((b) => zenControlVisible(b.el, 'home-switcher', geometry, reserved))) {
          return { ok: false, control, problem: 'invisible' }
        }
      } else {
        // Always-shown Homes: every Home has a visible option.
        const live = options.filter((b) => b.el.isConnected)
        const ids = new Set(live.map((b) => b.homeId))
        if (!input.homeIds.every((id) => ids.has(id))) return { ok: false, control, problem: 'not-wired' }
        if (!live.every((b) => zenControlVisible(b.el, 'home-option', geometry, reserved))) {
          return { ok: false, control, problem: 'invisible' }
        }
      }
      if (registry.wiringFailure() === 'home-switcher') return { ok: false, control, problem: 'not-wired' }
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
