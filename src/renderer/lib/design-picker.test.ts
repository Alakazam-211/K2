// @vitest-environment jsdom
// The Design mode guest picker (src-tauri/src/commands/design_picker.js),
// run in jsdom the way the host injects it: the file is a function
// expression, the host appends `(<navSeq>)` and evaluates it in the page.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeAll, beforeEach, describe, expect, it } from 'vitest'

const SOURCE = readFileSync(
  resolve(__dirname, '../../../src-tauri/src/commands/design_picker.js'),
  'utf8',
)

interface Picker {
  version: number
  setNav(n: number): number
  arm(on: boolean): boolean
  isArmed(): boolean
  drain(): string
  setBadges(list: { n: number; selector: string }[]): number
  __test: { shadow: ShadowRoot; host: HTMLElement; selectorFor(el: Element): string }
}

function install(nav: number): unknown {
  return (0, eval)(`${SOURCE}(${nav})`)
}

function picker(): Picker {
  return (window as unknown as { __k2DesignPicker: Picker }).__k2DesignPicker
}

function drain(): { nav: number; events: Array<Record<string, unknown>> } {
  return JSON.parse(picker().drain())
}

function key(target: EventTarget, k: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const e = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, composed: true, ...init })
  target.dispatchEvent(e)
  return e
}

beforeAll(() => {
  ;(window as unknown as { __K2_DESIGN_TEST__: boolean }).__K2_DESIGN_TEST__ = true
  expect(install(1)).toBe('ok')
})

beforeEach(() => {
  picker().arm(false)
  picker().drain()
  document.body.textContent = ''
})

describe('design picker guest script', () => {
  it('never uses an HTML-string sink or a <style> element (Trusted Types, strict CSP)', () => {
    expect(SOURCE).not.toMatch(/innerHTML|outerHTML\s*=|insertAdjacentHTML|document\.write|createElement\('style'\)/)
    expect(SOURCE).toContain("attachShadow({ mode: 'closed' })")
  })

  it('installs once; a second install keeps the same picker and moves to the new page load', () => {
    const first = picker()
    expect(install(1)).toBe('ok')
    expect(picker()).toBe(first)
    key(document.body, 's')
    install(2)
    expect(drain()).toEqual({ nav: 2, events: [] })
    install(1)
    expect(picker().setNav(1)).toBe(1)
  })

  it('builds a selector: a unique id, else a tag/class/nth-of-type path', () => {
    const main = document.createElement('main')
    main.id = 'app'
    const list = document.createElement('ul')
    list.className = 'menu wide extra'
    const a = document.createElement('li')
    const b = document.createElement('li')
    list.append(a, b)
    main.append(list)
    document.body.append(main)
    const sel = picker().__test.selectorFor
    expect(sel(main)).toBe('#app')
    expect(sel(b)).toBe('#app > ul.menu.wide > li:nth-of-type(2)')
    expect(document.querySelector(sel(b))).toBe(b)
  })

  it('no S hotkey: S reaches the page and nothing is queued', () => {
    const e = key(document.body, 's')
    expect(e.defaultPrevented).toBe(false)
    key(document.body, 'S')
    expect(drain().events).toEqual([])
  })

  it('armed: a click picks the element and the page never sees the click', () => {
    let pageSaw = 0
    const button = document.createElement('button')
    button.className = 'cta primary'
    button.setAttribute('aria-label', 'Buy now')
    button.textContent = 'Buy'
    button.addEventListener('click', () => pageSaw++)
    document.body.append(button)
    document.title = 'Pricing'
    picker().arm(true)
    const click = new MouseEvent('click', { bubbles: true, cancelable: true, composed: true })
    button.dispatchEvent(click)
    expect(click.defaultPrevented).toBe(true)
    expect(pageSaw).toBe(0)
    const { events } = drain()
    expect(events).toHaveLength(1)
    const pick = events[0] as {
      type: string
      pickId: string
      url: string
      title: string
      viewport: Record<string, number>
      element: Record<string, unknown>
    }
    expect(pick.type).toBe('pick')
    expect(pick.pickId).toMatch(/^p/)
    expect(pick.url).toBe(window.location.href)
    expect(pick.title).toBe('Pricing')
    expect(Object.keys(pick.viewport).sort()).toEqual(['dpr', 'height', 'scrollX', 'scrollY', 'width'])
    expect(pick.element.tag).toBe('button')
    expect(pick.element.role).toBe('button')
    expect(pick.element.name).toBe('Buy now')
    expect(pick.element.text).toBe('Buy')
    expect(pick.element.classes).toEqual(['cta', 'primary'])
    expect(pick.element.html).toContain('<button')
    expect(pick.element.selector).toBe('button.cta.primary')
    expect(pick.element.rect).toEqual({ x: 0, y: 0, width: 0, height: 0 })
  })

  it('disarmed: clicks reach the page and nothing is picked', () => {
    let pageSaw = 0
    const button = document.createElement('button')
    button.addEventListener('click', () => pageSaw++)
    document.body.append(button)
    picker().arm(true)
    picker().arm(false)
    button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    expect(pageSaw).toBe(1)
    expect(drain().events).toEqual([])
  })

  it('Esc while armed queues escape and disarms; Esc unarmed is the page’s', () => {
    picker().arm(true)
    const e = key(document.body, 'Escape')
    expect(e.defaultPrevented).toBe(true)
    expect(picker().isArmed()).toBe(false)
    const again = key(document.body, 'Escape')
    expect(again.defaultPrevented).toBe(false)
    expect(drain().events).toEqual([{ type: 'key', key: 'escape' }])
  })

  it('the queue is bounded to the last 20 events', () => {
    const button = document.createElement('button')
    document.body.append(button)
    picker().arm(true)
    for (let i = 0; i < 30; i++) button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    expect(drain().events).toHaveLength(20)
  })

  it('badges are drawn in the closed shadow root and replaced on each call', () => {
    const t = picker().__test
    expect(document.querySelectorAll('[data-k2-design]')).toHaveLength(1)
    expect(t.host.shadowRoot).toBeNull()
    const target = document.createElement('p')
    target.id = 'hero'
    document.body.append(target)
    expect(picker().setBadges([{ n: 1, selector: '#hero' }, { n: 2, selector: '#missing' }, { n: 3, selector: '((' }])).toBe(3)
    const layer = t.shadow.childNodes[2] as HTMLElement
    expect(Array.from(layer.childNodes).map((n) => n.textContent)).toEqual(['1', '2', '3'])
    expect((layer.childNodes[0] as HTMLElement).style.display).toBe('block')
    expect((layer.childNodes[1] as HTMLElement).style.display).toBe('none')
    expect(picker().setBadges([])).toBe(0)
    expect(layer.childNodes).toHaveLength(0)
  })
})
