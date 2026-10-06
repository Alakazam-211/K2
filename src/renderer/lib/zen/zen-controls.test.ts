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
  registerZenK2Overlay,
  ZEN_WIRING_DEADLINE_MS,
  type ZenControlRegistry,
  type ZenGeometry,
  type ZenRect,
} from './zen-controls'
import { macStoplightArea } from './zen-chrome'
import { zenMenuRequiredControls, type ZenPlacement } from './zen-page'
import { zenSafeCauseText } from './zen-view'

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

// prd-zen-freeform-chrome FC23, FC25, FC26 (FC-T7, FC-T8, FC-T9): a
// required control inside a menu is checked through the menu's BUTTON.
describe('controls in a menu (FC25)', () => {
  /** `menu-both`: one ⋯ menu (More) top right holding both required
   *  controls and the theme. */
  const MENU_BOTH: ZenPlacement = {
    from: 'garden',
    items: [
      { id: 'more', kind: 'menu', slot: 'top', align: 'end', props: { icon: 'dots', label: 'More' }, caps: [] },
      { id: 'garden-switcher', kind: 'garden-switcher', slot: 'menu', menu: 'more', props: {}, caps: ['gardens:manage'] },
      { id: 'theme-picker', kind: 'theme-picker', slot: 'menu', menu: 'more', props: {}, caps: [] },
      { id: 'zen-toggle', kind: 'zen-toggle', slot: 'menu', menu: 'more', props: {}, caps: [] },
    ],
    bands: { top: { start: [], center: [], end: ['more'] }, bottom: null },
    edges: [],
    menus: { more: ['garden-switcher', 'theme-picker', 'zen-toggle'] },
  }
  const MORE = { id: 'more', label: 'More' }
  let menuRegistry: ZenControlRegistry
  let button: HTMLElement

  function checkMenu(placement: ZenPlacement = MENU_BOTH, reserved: ZenRect[] = []) {
    return checkZenControls({
      declared: ['garden-switcher', 'drag-region', 'zen-toggle'],
      registry: menuRegistry,
      geometry: geo,
      reserved,
      gardenIds: GARDENS,
      root,
      placement,
    })
  }

  /** The open menu's rows, bound as K2 binds them. */
  function openRows(): Array<() => void> {
    return [...GARDENS.map((id) => menuRegistry.bind('garden-option', el(), id)), menuRegistry.bind('zen-toggle', el())]
  }

  beforeEach(() => {
    menuRegistry = createControlRegistry({
      ...actions,
      menuHolds: (id) => zenMenuRequiredControls(MENU_BOTH, id),
    })
    button = el()
    menuRegistry.bind('zen-menu', button, 'more')
  })

  afterEach(() => menuRegistry.dispose())

  it('FC-T7: closed, neither control is bound, the button is on screen → ok, twice; no safe mode', () => {
    const onFail = vi.fn()
    const streak = createControlStreak(onFail)
    expect(menuRegistry.bindings().map((b) => b.kind)).toEqual(['zen-menu'])
    expect(checkMenu()).toEqual({ ok: true })
    streak.record(checkMenu())
    streak.record(checkMenu())
    expect(onFail).not.toHaveBeenCalled()
  })

  it('FC-T8: the button at opacity 0.2, 10×10, off screen, under the stoplights, covered → invisible, naming the menu', () => {
    const invisible = { ok: false, control: 'zen-toggle', problem: 'invisible', menu: MORE }
    style.set(button, { opacity: 0.2 })
    expect(checkMenu()).toEqual(invisible)
    style.clear()
    rects.set(button, { left: 200, top: 8, width: 10, height: 10 })
    expect(checkMenu()).toEqual(invisible)
    rects.set(button, { left: 1300, top: 8, width: 30, height: 30 })
    expect(checkMenu()).toEqual(invisible)
    rects.set(button, { left: 8, top: 6, width: 30, height: 30 })
    expect(checkMenu(MENU_BOTH, [macStoplightArea([0, 0], 1).rect])).toEqual(invisible)
    rects.clear()
    covered.add(button)
    expect(checkMenu()).toEqual(invisible)
    covered.clear()
    expect(checkMenu()).toEqual({ ok: true })
  })

  it('FC-T8: a button that can’t take focus, or inside [inert] / aria-hidden → no-keyboard', () => {
    const noKeys = { ok: false, control: 'zen-toggle', problem: 'no-keyboard', menu: MORE }
    button.tabIndex = -1
    expect(checkMenu()).toEqual(noKeys)
    button.tabIndex = 0
    const w = wrap(button)
    w.setAttribute('inert', '')
    expect(checkMenu()).toEqual(noKeys)
    w.removeAttribute('inert')
    w.setAttribute('aria-hidden', 'true')
    expect(checkMenu()).toEqual(noKeys)
    w.removeAttribute('aria-hidden')
    expect(checkMenu()).toEqual({ ok: true })
  })

  it('no menu button on the page → missing, naming the menu', () => {
    menuRegistry.dispose()
    expect(checkMenu()).toEqual({ ok: false, control: 'zen-toggle', problem: 'missing', menu: MORE })
  })

  it('FC-T8: activating the button without binding the toggle within 1 s → not wired; binding inside the second passes at once, even if it closes', () => {
    vi.useFakeTimers()
    button.click()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    // The Gardens came, the toggle didn't.
    expect(menuRegistry.menuWiringFailure('more')).toBe('garden-switcher')
    expect(checkMenu()).toEqual({ ok: false, control: 'garden-switcher', problem: 'not-wired', menu: MORE })

    button.click()
    const gardens = GARDENS.map((id) => menuRegistry.bind('garden-option', el(), id))
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(menuRegistry.menuWiringFailure('more')).toBe('zen-toggle')
    expect(checkMenu()).toEqual({ ok: false, control: 'zen-toggle', problem: 'not-wired', menu: MORE })
    for (const off of gardens) off()

    // Everything it holds binds inside the second: wired at once, and the
    // menu closing before the second is up changes nothing.
    button.click()
    const offs = openRows()
    expect(checkMenu()).toEqual({ ok: true })
    vi.advanceTimersByTime(100)
    for (const off of offs) off()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(menuRegistry.menuWiringFailure('more')).toBeNull()
    expect(checkMenu()).toEqual({ ok: true })
  })

  it('FC26: an activation while its rows are bound (the click that closes it) passes at once', () => {
    vi.useFakeTimers()
    button.click()
    const offs = openRows()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS + 500)
    button.click()
    for (const off of offs) off()
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS * 2)
    expect(checkMenu()).toEqual({ ok: true })
  })

  it('FC25: two menus keep their own timers; one settling never settles the other', () => {
    vi.useFakeTimers()
    const twoMenus: ZenPlacement = {
      ...MENU_BOTH,
      items: [
        ...MENU_BOTH.items.filter((i) => i.kind !== 'garden-switcher'),
        { id: 'gm', kind: 'menu', slot: 'bottom', align: 'start', props: {}, caps: [] },
        { id: 'garden-switcher', kind: 'garden-switcher', slot: 'menu', menu: 'gm', props: {}, caps: [] },
      ],
      menus: { more: ['theme-picker', 'zen-toggle'], gm: ['garden-switcher'] },
    }
    menuRegistry.dispose()
    menuRegistry = createControlRegistry({ ...actions, menuHolds: (id) => zenMenuRequiredControls(twoMenus, id) })
    const more = el()
    const gm = el()
    menuRegistry.bind('zen-menu', more, 'more')
    menuRegistry.bind('zen-menu', gm, 'gm')
    gm.click()
    more.click()
    // `more`'s toggle binds; `gm`'s Gardens never do.
    menuRegistry.bind('zen-toggle', el())
    vi.advanceTimersByTime(ZEN_WIRING_DEADLINE_MS)
    expect(menuRegistry.menuWiringFailure('more')).toBeNull()
    expect(menuRegistry.menuWiringFailure('gm')).toBe('garden-switcher')
    expect(checkMenu(twoMenus)).toEqual({ ok: false, control: 'garden-switcher', problem: 'not-wired', menu: { id: 'gm', label: 'More' } })
  })

  it('FC25: K2 never cancels Enter or Space on a menu button', () => {
    for (const key of ['Enter', ' ']) {
      const e = new KeyboardEvent('keydown', { key, cancelable: true })
      button.dispatchEvent(e)
      expect(e.defaultPrevented).toBe(false)
    }
  })

  it('a zen-menu needs its menu id, loudly', () => {
    expect(() => menuRegistry.bind('zen-menu', el())).toThrow(/menu id/)
  })

  it('FC-T9: a K2 overlay (an open menu) over the button is not covering it; a page element is', () => {
    const list = el('div')
    const off = registerZenK2Overlay(list)
    const coverWith = (hit: Element): ZenGeometry => ({ ...geo, elementFromPoint: () => hit })
    const run = (g: ZenGeometry) =>
      checkZenControls({ declared: ['garden-switcher', 'zen-toggle'], registry: menuRegistry, geometry: g, reserved: [], gardenIds: GARDENS, root, placement: MENU_BOTH })
    expect(run(coverWith(list))).toEqual({ ok: true })
    expect(run(coverWith(document.body))).toEqual({ ok: false, control: 'zen-toggle', problem: 'invisible', menu: MORE })
    off()
    expect(run(coverWith(list))).toEqual({ ok: false, control: 'zen-toggle', problem: 'invisible', menu: MORE })
  })

  it('a control placed directly is checked where it is drawn, keyboard included', () => {
    const direct: ZenPlacement = {
      from: 'garden',
      items: [
        { id: 'garden-switcher', kind: 'garden-switcher', slot: 'bottom', align: 'start', props: {}, caps: [] },
        { id: 'zen-toggle', kind: 'zen-toggle', slot: 'bottom', align: 'end', props: {}, caps: [] },
      ],
      bands: { top: null, bottom: { start: ['garden-switcher'], center: [], end: ['zen-toggle'] } },
      edges: [],
      menus: {},
    }
    const { toggle } = goodPage()
    expect(check()).toEqual({ ok: true })
    expect(checkZenControls({ declared: ['garden-switcher', 'zen-toggle'], registry, geometry: geo, reserved: [], gardenIds: GARDENS, root, placement: direct })).toEqual({ ok: true })
    toggle.tabIndex = -1
    expect(check()).toEqual({ ok: false, control: 'zen-toggle', problem: 'no-keyboard' })
  })
})

describe('the safe-mode cause names the menu (FC25)', () => {
  it('“The Zen toggle’s menu (More) isn’t visible.” and the no-keyboard sentence', () => {
    expect(zenSafeCauseText({ kind: 'control', control: 'zen-toggle', problem: 'invisible', menu: { id: 'more', label: 'More' } })).toBe(
      'The Zen toggle’s menu (More) isn’t visible.',
    )
    expect(zenSafeCauseText({ kind: 'control', control: 'garden-switcher', problem: 'no-keyboard' })).toBe(
      'The Garden switcher can’t be reached from the keyboard.',
    )
    expect(zenSafeCauseText({ kind: 'control', control: 'garden-switcher', problem: 'not-wired', menu: { id: 'gm', label: 'Gardens' } })).toBe(
      'The Garden switcher’s menu (Gardens) isn’t wired.',
    )
  })
})
