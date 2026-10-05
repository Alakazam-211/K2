// @vitest-environment jsdom
//
// prd-zen-mode-v1 Z27/Z28/Z64 (T4.5) and prd-zen-gardens-v1 G24 (TG4.1) —
// the required-controls registry and
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

const GARDENS = ['g1', 'g2']
let root: HTMLDivElement
let actions: { exit: ReturnType<typeof vi.fn>; selectGarden: ReturnType<typeof vi.fn>; gardenIds: () => string[] }
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
  registry.bind('garden-switcher', trigger)
  registry.bind('drag-region', drag)
  return { toggle, trigger, drag }
}

function check(declared: string[] = ['zen-toggle', 'garden-switcher', 'drag-region'], reserved: ZenRect[] = []) {
  return checkZenControls({ declared, registry, geometry: geo, reserved, gardenIds: GARDENS, root })
}

beforeEach(() => {
  root = document.createElement('div')
  document.body.appendChild(root)
  style.clear()
  rects.clear()
  covered.clear()
  actions = { exit: vi.fn(), selectGarden: vi.fn(), gardenIds: () => GARDENS }
  registry = createControlRegistry(actions)
})

afterEach(() => {
  registry.dispose()
  root.remove()
  vi.useRealTimers()
})

describe('binding', () => {
  it('K2 attaches the actions: toggle → exit (click, Enter, Space); option → switch to its Garden; trigger → nothing', () => {
    const { toggle, trigger } = goodPage()
    const option = el()
    registry.bind('garden-option', option, 'g2')
    toggle.click()
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }))
    toggle.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    expect(actions.exit).toHaveBeenCalledTimes(3)
    option.click()
    expect(actions.selectGarden).toHaveBeenCalledWith('g2')
    trigger.click()
    expect(actions.exit).toHaveBeenCalledTimes(3)
    expect(actions.selectGarden).toHaveBeenCalledTimes(1)
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

  it('refuses a bad kind and an option with no Garden id, loudly', () => {
    expect(() => registry.bind('nope' as never, el())).toThrow(/unknown control kind/)
    expect(() => registry.bind('garden-option', el())).toThrow(/needs its Garden id/)
  })
})

describe('the check', () => {
  it('passes for a good page', () => {
    goodPage()
    expect(check()).toEqual({ ok: true })
  })

  it('undeclared', () => {
    goodPage()
    expect(check(['zen-toggle', 'drag-region'])).toEqual({ ok: false, control: 'garden-switcher', problem: 'undeclared' })
  })

  it('not registered → missing; detached → missing', () => {
    const toggle = el()
    registry.bind('zen-toggle', toggle)
    const drag = el('div')
    rects.set(drag, { left: 300, top: 8, width: 400, height: 28 })
    registry.bind('drag-region', drag)
    expect(check()).toEqual({ ok: false, control: 'garden-switcher', problem: 'missing' })
    const trigger = el()
    registry.bind('garden-switcher', trigger)
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
    expect(check()).toEqual({ ok: false, control: 'garden-switcher', problem: 'invisible' })
  })

  it('a toggle under the macOS stoplights → invisible', () => {
    const { toggle } = goodPage()
    const lights = macStoplightArea([0, 0], 1).rect
    rects.set(toggle, { left: 20, top: 4, width: 30, height: 24 })
    expect(check(undefined, [lights])).toEqual({ ok: false, control: 'zen-toggle', problem: 'invisible' })
    rects.set(toggle, { left: lights.width + 10, top: 4, width: 30, height: 24 })
    expect(check(undefined, [lights])).toEqual({ ok: true })
  })

  it('only two controls are required (Rosson 2026-10-04): a tiny, covered, unbound or undeclared drag region never fails', () => {
    const { drag } = goodPage()
    rects.set(drag, { left: 300, top: 8, width: 100, height: 4 })
    covered.add(drag)
    expect(check()).toEqual({ ok: true })
    drag.remove()
    expect(check(['zen-toggle', 'garden-switcher'])).toEqual({ ok: true })
  })

  it('an element marked as a control but never bound → not wired', () => {
    goodPage()
    el('button', { 'data-zen-control': 'zen-toggle' })
    expect(check()).toEqual({ ok: false, control: 'zen-toggle', problem: 'not-wired' })
  })

  it('always-shown Gardens: options for every Garden, else not wired', () => {
    const toggle = el()
    const drag = el('div')
    rects.set(drag, { left: 300, top: 8, width: 400, height: 28 })
    registry.bind('zen-toggle', toggle)
    registry.bind('drag-region', drag)
    registry.bind('garden-option', el(), 'g1')
    expect(check()).toEqual({ ok: false, control: 'garden-switcher', problem: 'not-wired' })
    registry.bind('garden-option', el(), 'g2')
    expect(check()).toEqual({ ok: true })
  })

  it('a trigger whose activation binds no options within 1 s → not wired; options for only some Gardens → not wired; all → ok', () => {
    vi.useFakeTimers()
    const { trigger } = goodPage()
    trigger.click()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: false, control: 'garden-switcher', problem: 'not-wired' })

    trigger.click()
    registry.bind('garden-option', el(), 'g1')
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: false, control: 'garden-switcher', problem: 'not-wired' })

    trigger.click()
    registry.bind('garden-option', el(), 'g2')
    // Every Garden has an option inside the second: wired at once.
    expect(check()).toEqual({ ok: true })
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: true })
  })
})

// Rosson 2026-10-04 bug: "Zen Safe Mode keeps popping up" with the Zen
// toggle right there. The switcher's 1 s wiring deadline read the options
// AT the deadline, so a menu opened and shut inside that second (a second
// click on the pill, Esc, a click outside, picking the Garden you're on)
// left a sticky `not-wired`, and the next two scheduled checks put the
// window in safe mode. Enter / Space on the pill were also swallowed
// (preventDefault), so the menu never opened from the keyboard and the
// deadline failed the same way.
describe('no false safe mode from the Garden switcher', () => {
  function openMenu(): Array<() => void> {
    return GARDENS.map((id) => registry.bind('garden-option', el(), id))
  }

  it('a menu opened and closed inside 1 s is wired', () => {
    vi.useFakeTimers()
    const { trigger } = goodPage()
    trigger.click()
    const offs = openMenu()
    vi.advanceTimersByTime(200)
    for (const off of offs) off()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(registry.wiringFailure()).toBeNull()
    expect(check()).toEqual({ ok: true })
  })

  it('the click that closes an open menu is not a fresh activation that must bind options', () => {
    vi.useFakeTimers()
    const { trigger } = goodPage()
    trigger.click()
    const offs = openMenu()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS + 500)
    expect(check()).toEqual({ ok: true })
    // Second click on the pill: the menu is still open when K2 sees it,
    // then the page closes it.
    trigger.click()
    for (const off of offs) off()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(check()).toEqual({ ok: true })
  })

  it('Enter / Space on the trigger are not swallowed (the button still opens its menu)', () => {
    const { trigger } = goodPage()
    for (const key of ['Enter', ' ']) {
      const e = new KeyboardEvent('keydown', { key, cancelable: true })
      trigger.dispatchEvent(e)
      expect(e.defaultPrevented).toBe(false)
    }
  })

  it('two scheduled checks after a quick open/close never reach safe mode', () => {
    vi.useFakeTimers()
    const onFail = vi.fn()
    const streak = createControlStreak(onFail)
    const { trigger } = goodPage()
    trigger.click()
    const offs = openMenu()
    for (const off of offs) off()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS * 3)
    streak.record(check())
    streak.record(check())
    expect(onFail).not.toHaveBeenCalled()
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
