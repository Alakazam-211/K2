// prd-zen-mode-v1 Z27, Z28, Z64 and prd-zen-gardens-v1 G24 — the required
// controls: bound by K2, then checked by K2. Two are required (Rosson
// 2026-10-04): the Zen toggle and the Garden switcher. The drag region is
// bound for its behaviour only and never checked.
//
// The page draws its own Zen toggle, Garden switcher and drag region, in
// any form. It hands each element to `bridge.controls.bind(kind, el, ref?)`
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
//   - `zen-menu` (prd-zen-freeform-chrome FC25): a K2 menu button, with its
//     menu id. Like the switcher trigger: no action, its keys are never
//     cancelled, K2 only watches its activation.
//
// Then K2 checks, for each of the two required controls. First it reads
// WHERE the page put it (`page.placement`, FC25):
//   - placed directly (a band or a column edge): the live checks below;
//   - inside menu M: M's BUTTON is checked instead (visible, keyboard,
//     wired). The control itself isn't looked at while M is closed (a
//     closed menu is not a missing control), and while M is open its rows
//     are K2's own overlay.
//
//   declared  the page's `controls` list names it;
//   present   it is bound and the element is connected;
//   visible   displayed, opacity ≥ 0.3 (through its ancestors), a box of at
//             least 24×24 (120×12 for the drag region), inside the viewport,
//             clear of the stoplights / window controls, and
//             `elementFromPoint` at its centre lands on it, inside it, or on
//             a K2 overlay;
//   keyboard  it can take focus (`tabIndex ≥ 0`, not disabled) and isn't
//             inside `[inert]` or `aria-hidden="true"` (`no-keyboard`);
//   wired     bound through `bind` (an element marked `data-zen-control`
//             that was never bound is "not wired"), and activating the
//             switcher trigger (or the menu button) binds what it holds
//             within 1 s: an option for every Garden, the toggle. Once that
//             happened inside the second the activation passed, even if the
//             menu closed again before the second was up; an activation
//             while they are already bound (the click that closes the menu)
//             passes at once. Each menu has its own timer.
// Two failed checks in a row are a failure (Z28): one bad frame mid
// animation isn't.

import { titleBarDragOnMouseDown } from '@/lib/titlebar-drag'
import {
  ZEN_REQUIRED_CONTROLS,
  zenControlPlacement,
  zenMenuLabel,
  type ZenControlKind,
  type ZenPlacement,
} from './zen-page'
import type { ZenControlMenu, ZenControlProblem } from './zen-view'

export type ZenBindKind = ZenControlKind | 'garden-option' | 'drag-region' | 'zen-menu'

export const ZEN_BIND_KINDS: readonly ZenBindKind[] = [
  'zen-toggle',
  'garden-switcher',
  'garden-option',
  'drag-region',
  'zen-menu',
]

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
  /** The required controls menu `id` holds (from the page; FC25). */
  menuHolds?(id: string): readonly ZenControlKind[]
}

export interface ZenBinding {
  readonly kind: ZenBindKind
  readonly el: HTMLElement
  /** A `garden-option`'s Garden id. */
  readonly gardenId: string | null
  /** A `zen-menu` button's menu id. */
  readonly menuId: string | null
}

export const ZEN_WIRING_DEADLINE_MS = 1000

export interface ZenControlRegistry {
  /** Bind `el` as `kind`. `ref` is a `garden-option`'s Garden id or a
   *  `zen-menu`'s menu id. Returns the unbind. Throws on a bad kind, or an
   *  option / menu without its id (a page bug, loud). */
  bind(kind: ZenBindKind, el: HTMLElement, ref?: string): () => void
  bindings(): readonly ZenBinding[]
  /** Set when a switcher activation didn't bind every Garden in time; cleared
   *  by a later activation that did. */
  wiringFailure(): ZenControlKind | null
  /** The required control menu `id` failed to bind in time after its
   *  button was activated (FC25), else null. */
  menuWiringFailure(id: string): ZenControlKind | null
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
  // FC45: one timer and one result per menu, so two menus can't settle
  // each other.
  const menuPending = new Map<string, unknown>()
  const menuWiring = new Map<string, ZenControlKind>()

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

  /** The first required control menu `id` holds that isn't bound now. */
  const menuGap = (id: string): ZenControlKind | null => {
    for (const kind of actions.menuHolds?.(id) ?? []) {
      const ok =
        kind === 'garden-switcher'
          ? optionsCoverGardens()
          : list.some((b) => b.kind === kind && b.el.isConnected)
      if (!ok) return kind
    }
    return null
  }

  const settleMenu = (id: string): void => {
    const h = menuPending.get(id)
    if (h !== undefined) timers.clearTimeout(h)
    menuPending.delete(id)
    menuWiring.delete(id)
  }

  const onMenuActivated = (id: string): void => {
    // Everything it holds is bound right now (this click closes it): wired.
    if (menuGap(id) === null) {
      settleMenu(id)
      return
    }
    const prev = menuPending.get(id)
    if (prev !== undefined) timers.clearTimeout(prev)
    menuPending.set(
      id,
      timers.setTimeout(() => {
        menuPending.delete(id)
        const gap = menuGap(id)
        if (gap) menuWiring.set(id, gap)
        else menuWiring.delete(id)
      }, ZEN_WIRING_DEADLINE_MS),
    )
  }

  return {
    bind(kind, el, ref) {
      if (!ZEN_BIND_KINDS.includes(kind)) throw new Error(`zen controls: unknown control kind "${String(kind)}"`)
      if (!(el instanceof HTMLElement)) throw new Error(`zen controls: bind("${kind}") needs an element`)
      if (kind === 'garden-option' && !ref) throw new Error('zen controls: a garden-option needs its Garden id')
      if (kind === 'zen-menu' && !ref) throw new Error('zen controls: a zen-menu needs its menu id')
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
          // A trigger's action is the page's: its key must still click.
          if (own) e.preventDefault()
          fn()
        })
      }
      if (kind === 'zen-toggle') activate(() => actions.exit())
      else if (kind === 'garden-option') activate(() => actions.selectGarden(ref as string))
      else if (kind === 'garden-switcher') activate(onTriggerActivated, false)
      else if (kind === 'zen-menu') activate(() => onMenuActivated(ref as string), false)
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
        gardenId: kind === 'garden-option' ? (ref ?? null) : null,
        menuId: kind === 'zen-menu' ? (ref ?? null) : null,
        off: () => {
          for (const off of offs) off()
          if (addedNoDrag) el.classList.remove('no-drag')
          el.removeAttribute('data-zen-bound')
        },
      }
      list.push(binding)
      // Every Garden now has an option inside the activation's second: wired.
      if (kind === 'garden-option' && pending !== null && optionsCoverGardens()) settle()
      // A menu's controls turned up inside its second: that menu is wired.
      if (kind === 'garden-option' || kind === 'zen-toggle') {
        for (const id of [...menuPending.keys()]) if (menuGap(id) === null) settleMenu(id)
      }
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
    menuWiringFailure(id) {
      return menuWiring.get(id) ?? null
    },
    dispose() {
      if (pending !== null) timers.clearTimeout(pending)
      pending = null
      for (const h of menuPending.values()) timers.clearTimeout(h)
      menuPending.clear()
      menuWiring.clear()
      for (const b of list.splice(0)) b.off()
    },
  }
}

const MIN_BUTTON = { width: 24, height: 24 }
/** The drag region's minimum width (CSS px): every band keeps at least
 *  this much empty space (FC12), and the check's "visible" needs it. */
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

/** FC23 rule 4: the button can take focus and isn't hidden from the
 *  keyboard (`[inert]`, `aria-hidden="true"`). */
export function zenControlKeyboardReachable(el: HTMLElement): boolean {
  if (el.tabIndex < 0) return false
  if ((el as HTMLButtonElement).disabled === true) return false
  if (el.closest('[inert]')) return false
  if (el.closest('[aria-hidden="true"]')) return false
  return true
}

// K2's own transient Zen overlays (the shortcut cheat sheet, the theme
// picker, the usage menu, every open K2 menu) sit above the page while
// open. They are K2's, closed with Esc, and never a page hiding its
// controls, so a control under one still counts as visible. Only elements
// K2 registered count (a page can't claim it).
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

export type ZenControlCheck =
  | { ok: true }
  | { ok: false; control: ZenControlKind; problem: ZenControlProblem; menu?: ZenControlMenu }

/** One full check of the two required controls (Z27, G24, FC25). */
export function checkZenControls(input: {
  declared: readonly string[]
  registry: ZenControlRegistry
  geometry: ZenGeometry
  /** Stoplight / window-control rects the controls must stay clear of. */
  reserved: readonly ZenRect[]
  gardenIds: readonly string[]
  /** The page root, searched for `data-zen-control` elements never bound. */
  root: Element | null
  /** Where the page put each control (FC25). Absent: every control is
   *  checked where it is drawn (a page with no menus). */
  placement?: ZenPlacement
}): ZenControlCheck {
  const { registry, geometry, reserved } = input
  const bound = registry.bindings()
  const shown = (b: ZenBinding): boolean => zenControlVisible(b.el, b.kind, geometry, reserved)
  for (const control of ZEN_REQUIRED_CONTROLS) {
    if (!input.declared.includes(control)) return { ok: false, control, problem: 'undeclared' }
    const marked = input.root ? Array.from(input.root.querySelectorAll(`[data-zen-control="${control}"]`)) : []
    if (marked.some((el) => !el.hasAttribute('data-zen-bound'))) return { ok: false, control, problem: 'not-wired' }
    const where = input.placement ? zenControlPlacement(input.placement, control) : null
    if (where && where.item.slot === 'menu') {
      // FC25: in menu M, M's button is what is on screen.
      const id = where.item.menu ?? ''
      const menu: ZenControlMenu = { id, label: where.menu ? zenMenuLabel(where.menu) : 'More' }
      const buttons = bound.filter((b) => b.kind === 'zen-menu' && b.menuId === id && b.el.isConnected)
      if (buttons.length === 0) return { ok: false, control, problem: 'missing', menu }
      const visible = buttons.filter(shown)
      if (visible.length === 0) return { ok: false, control, problem: 'invisible', menu }
      if (!visible.some((b) => zenControlKeyboardReachable(b.el))) return { ok: false, control, problem: 'no-keyboard', menu }
      if (registry.menuWiringFailure(id) === control) return { ok: false, control, problem: 'not-wired', menu }
      continue
    }
    if (control === 'garden-switcher') {
      const triggers = bound.filter((b) => b.kind === 'garden-switcher')
      const options = bound.filter((b) => b.kind === 'garden-option')
      if (triggers.length === 0 && options.length === 0) {
        return { ok: false, control, problem: marked.length > 0 ? 'not-wired' : 'missing' }
      }
      if (triggers.length > 0) {
        const live = triggers.filter((b) => b.el.isConnected)
        if (live.length === 0) return { ok: false, control, problem: 'missing' }
        const visible = live.filter(shown)
        if (visible.length === 0) return { ok: false, control, problem: 'invisible' }
        if (!visible.some((b) => zenControlKeyboardReachable(b.el))) return { ok: false, control, problem: 'no-keyboard' }
      } else {
        // Always-shown Gardens: every Garden has a visible option.
        const live = options.filter((b) => b.el.isConnected)
        const ids = new Set(live.map((b) => b.gardenId))
        if (!input.gardenIds.every((id) => ids.has(id))) return { ok: false, control, problem: 'not-wired' }
        if (!live.every(shown)) return { ok: false, control, problem: 'invisible' }
        if (!live.every((b) => zenControlKeyboardReachable(b.el))) return { ok: false, control, problem: 'no-keyboard' }
      }
      if (registry.wiringFailure() === 'garden-switcher') return { ok: false, control, problem: 'not-wired' }
      continue
    }
    const mine = bound.filter((b) => b.kind === control)
    if (mine.length === 0) return { ok: false, control, problem: marked.length > 0 ? 'not-wired' : 'missing' }
    const live = mine.filter((b) => b.el.isConnected)
    if (live.length === 0) return { ok: false, control, problem: 'missing' }
    const visible = live.filter(shown)
    if (visible.length === 0) return { ok: false, control, problem: 'invisible' }
    if (!visible.some((b) => zenControlKeyboardReachable(b.el))) return { ok: false, control, problem: 'no-keyboard' }
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
