// @vitest-environment jsdom
//
// prd-zen-mode-v1 Z27/Z28/Z64 (T4.5) — the required-controls registry and
// check, against an injected layout. Each failing shape maps to its cause:
// opacity, 10×10, off screen, under the stoplight rect, covered, not
// registered, bypassing bind, a switcher whose activation binds no options
// (or only some) within 1 s. One failed check alone is not a failure.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ startDragging: async () => undefined, isMaximized: async () => false }),
}))

import {
  checkZenControls,
  createControlRegistry,
  createControlStreak,
  ZEN_WIRING_DEADLINE_MS,
  type ZenControlRegistry,
  type ZenGeometry,
  type ZenRect,
} from './zen-controls'
import { macStoplightArea } from './zen-chrome'

const style = new Map<Element, Partial<{ display: string; visibility: string; opacity: number }>>()
const rects = new Map<Element, ZenRect>()
const covered = new Set<Element>()
let last: Element | null = null
const geo: ZenGeometry = {
  rect(el) {
    last = el
    return rects.get(el) ?? { left: 200, top: 8, width: 120, height: 28 }
  },
  style(el) {
    return { display: 'block', visibility: 'visible', opacity: 1, ...style.get(el) }
  },
  viewport: () => ({ width: 1200, height: 800 }),
  elementFromPoint: () => (last && !covered.has(last) ? last : document.body),
}

const HOMES = ['h1', 'h2']
let root: HTMLDivElement
let actions: { exit: ReturnType<typeof vi.fn>; selectHome: ReturnType<typeof vi.fn>; homeIds: () => string[] }
let registry: ZenControlRegistry

function el(tag = 'button', attrs: Record<string, string> = {}): HTMLElement {
  const e = document.createElement(tag)
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v)
  root.appendChild(e)
  return e
}

/** Move `t` into its own wrapper (an ancestor only it has). */
function wrap(t: HTMLElement): HTMLElement {
  const w = document.createElement('div')
  root.appendChild(w)
  w.appendChild(t)
  return w
}

/** A page with all three controls bound (trigger-style switcher). */
function goodPage(): { toggle: HTMLElement; trigger: HTMLElement; drag: HTMLElement } {
  const toggle = el()
  const trigger = el()
  const drag = el('div')
  rects.set(drag, { left: 300, top: 8, width: 400, height: 28 })
  registry.bind('zen-toggle', toggle)
  registry.bind('home-switcher', trigger)
  registry.bind('drag-region', drag)
  return { toggle, trigger, drag }
}

function check(declared: string[] = ['zen-toggle', 'home-switcher', 'drag-region'], reserved: ZenRect[] = []) {
  return checkZenControls({ declared, registry, geometry: geo, reserved, homeIds: HOMES, root })
}

beforeEach(() => {
  root = document.createElement('div')
  document.body.appendChild(root)
  style.clear()
  rects.clear()
  covered.clear()
  actions = { exit: vi.fn(), selectHome: vi.fn(), homeIds: () => HOMES }
  registry = createControlRegistry(actions)
})

afterEach(() => {
  registry.dispose()
  root.remove()
  vi.useRealTimers()
})

describe('binding', () => {
  it('K2 attaches the actions: toggle → exit (click, Enter, Space); option → select its Home; trigger → nothing', () => {
    const { toggle, trigger } = goodPage()
    const option = el()
    registry.bind('home-option', option, 'h2')
    toggle.click()
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }))
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    expect(actions.exit).toHaveBeenCalledTimes(3)
    option.click()
    expect(actions.selectHome).toHaveBeenCalledWith('h2')
    trigger.click()
    expect(actions.exit).toHaveBeenCalledTimes(3)
    expect(actions.selectHome).toHaveBeenCalledTimes(1)
  })

  it('Z64: bound controls get .no-drag (the drag region does not); unbinding removes listener and marks', () => {
    const toggle = el()
    const drag = el('div')
    const off = registry.bind('zen-toggle', toggle)
    registry.bind('drag-region', drag)
    expect(toggle.classList.contains('no-drag')).toBe(true)
    expect(toggle.getAttribute('data-zen-bound')).toBe('zen-toggle')
    expect(drag.classList.contains('no-drag')).toBe(false)
    off()
    expect(toggle.classList.contains('no-drag')).toBe(false)
    expect(toggle.hasAttribute('data-zen-bound')).toBe(false)
    toggle.click()
    expect(actions.exit).not.toHaveBeenCalled()
  })

  it('refuses a bad kind and an option with no Home id, loudly', () => {
    expect(() => registry.bind('nope' as never, el())).toThrow(/unknown control kind/)
    expect(() => registry.bind('home-option', el())).toThrow(/needs its Home id/)
  })
})

describe('the check', () => {
  it('passes for a good page', () => {
    goodPage()
    expect(check()).toEqual({ ok: true })
  })

  it('undeclared', () => {
    goodPage()
    expect(check(['zen-toggle', 'drag-region'])).toEqual({ ok: false, control: 'home-switcher', problem: 'undeclared' })
  })

  it('not registered → missing; detached → missing', () => {
    const toggle = el()
    registry.bind('zen-toggle', toggle)
    const drag = el('div')
    rects.set(drag, { left: 300, top: 8, width: 400, height: 28 })
    registry.bind('drag-region', drag)
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'missing' })
    const trigger = el()
    registry.bind('home-switcher', trigger)
    expect(check()).toEqual({ ok: true })
    toggle.remove()
    expect(check()).toEqual({ ok: false, control: 'zen-toggle', problem: 'missing' })
  })

  it.each([
    ['opacity 0', (t: HTMLElement) => style.set(t, { opacity: 0 })],
    ['opacity through an ancestor', (t: HTMLElement) => style.set(wrap(t), { opacity: 0.2 })],
    ['display none on an ancestor', (t: HTMLElement) => style.set(wrap(t), { display: 'none' })],
    ['visibility hidden', (t: HTMLElement) => style.set(t, { visibility: 'hidden' })],
    ['10×10', (t: HTMLElement) => rects.set(t, { left: 200, top: 8, width: 10, height: 10 })],
    ['off screen', (t: HTMLElement) => rects.set(t, { left: 1190, top: 8, width: 40, height: 28 })],
    ['covered by another element', (t: HTMLElement) => covered.add(t)],
  ])('an invisible switcher (%s) → invisible', (_name, apply) => {
    const { trigger } = goodPage()
    apply(trigger)
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'invisible' })
  })

  it('a toggle under the macOS stoplights → invisible', () => {
    const { toggle } = goodPage()
    const lights = macStoplightArea(0, 1).rect
    rects.set(toggle, { left: 20, top: 4, width: 30, height: 24 })
    expect(check(undefined, [lights])).toEqual({ ok: false, control: 'zen-toggle', problem: 'invisible' })
    rects.set(toggle, { left: lights.width + 10, top: 4, width: 30, height: 24 })
    expect(check(undefined, [lights])).toEqual({ ok: true })
  })

  it('the drag region needs 120×12', () => {
    const { drag } = goodPage()
    rects.set(drag, { left: 300, top: 8, width: 100, height: 28 })
    expect(check()).toEqual({ ok: false, control: 'drag-region', problem: 'invisible' })
  })

  it('an element marked as a control but never bound → not wired', () => {
    goodPage()
    el('button', { 'data-zen-control': 'zen-toggle' })
    expect(check()).toEqual({ ok: false, control: 'zen-toggle', problem: 'not-wired' })
  })

  it('always-shown Homes: options for every Home, else not wired', () => {
    const toggle = el()
    const drag = el('div')
    rects.set(drag, { left: 300, top: 8, width: 400, height: 28 })
    registry.bind('zen-toggle', toggle)
    registry.bind('drag-region', drag)
    registry.bind('home-option', el(), 'h1')
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'not-wired' })
    registry.bind('home-option', el(), 'h2')
    expect(check()).toEqual({ ok: true })
  })

  it('a trigger whose activation binds no options within 1 s → not wired; options for only some Homes → not wired; all → ok', () => {
    vi.useFakeTimers()
    const { trigger } = goodPage()
    trigger.click()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'not-wired' })

    trigger.click()
    registry.bind('home-option', el(), 'h1')
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'not-wired' })

    trigger.click()
    registry.bind('home-option', el(), 'h2')
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS - 1)
    // Not decided yet: the earlier failure still stands until the deadline.
    expect(check()).toEqual({ ok: false, control: 'home-switcher', problem: 'not-wired' })
    vi.advanceTimersByTime(1)
    expect(check()).toEqual({ ok: true })
  })
})

describe('two in a row (Z28)', () => {
  it('one failure is not a failure; two in a row are; a pass in between resets', () => {
    const onFail = vi.fn()
    const streak = createControlStreak(onFail)
    const bad = { ok: false as const, control: 'zen-toggle' as const, problem: 'invisible' as const }
    streak.record(bad)
    expect(onFail).not.toHaveBeenCalled()
    streak.record({ ok: true })
    streak.record(bad)
    expect(onFail).not.toHaveBeenCalled()
    streak.record(bad)
    expect(onFail).toHaveBeenCalledTimes(1)
    expect(onFail).toHaveBeenCalledWith(bad)
    streak.record(bad)
    expect(onFail).toHaveBeenCalledTimes(1)
  })
})
