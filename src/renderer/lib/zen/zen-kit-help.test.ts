// K2's widget kit: the (i) help button library, k2-help@1 (0.46.3, Garden
// gaps). The shipped file runs byte for byte in a fresh JSDOM per test. It
// pins: hover and keyboard focus show a tip; click, tap, Enter or Space pin
// it; Esc, a tap outside or the same (i) close it; long text is a popover
// that takes focus and gives it back on Esc or Close; markup is wired;
// a redrawn button with the same key keeps its open tip; placement stays
// inside the frame (flipped above when there's no room below); aria.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const HELP = readFileSync(resolve(__dirname, '../../zen-lib/k2-help@1.0.0/k2-help.js'), 'utf8')

function boot(body = '') {
  const dom = new JSDOM(`<!doctype html><html><head></head><body>${body}<p id="out">outside</p></body></html>`, {
    runScripts: 'outside-only',
    pretendToBeVisual: true,
  })
  const win = dom.window as unknown as Window & typeof globalThis & { eval: (s: string) => unknown; K2Help: any }
  win.eval(HELP)
  const doc = win.document
  const ev = (el: Element, type: string, init: Record<string, unknown> = {}): void => {
    const e = new win.Event(type, { bubbles: true, cancelable: true })
    for (const [k, v] of Object.entries(init)) Object.defineProperty(e, k, { value: v })
    el.dispatchEvent(e)
  }
  const keydown = (el: Element, key: string): void => {
    el.dispatchEvent(new win.KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }))
  }
  const tipText = (): string | null => {
    const t = doc.querySelector<HTMLElement>('.k2h-tip')
    return t && !t.hidden ? t.textContent : null
  }
  return { win, doc, ev, keydown, tipText }
}

describe('k2-help@1', () => {
  it('defines K2Help once and injects its style once', () => {
    const { win, doc } = boot()
    expect(win.K2Help.version).toBe('1.0.0')
    win.eval(HELP)
    expect(doc.querySelectorAll('style[data-k2-lib="k2-help"]')).toHaveLength(1)
  })

  it('a tip: an (i) with a label and a hidden description; hover shows, leaving hides', () => {
    const { win, doc, ev, tipText } = boot()
    const wrap = win.K2Help.tip('Boxes more than one minor behind.', { label: 'Version spread' })
    doc.body.appendChild(wrap)
    const b = wrap.querySelector('button') as HTMLButtonElement
    expect(b.getAttribute('aria-label')).toBe('About Version spread')
    expect(b.textContent).toBe('ⓘ')
    const desc = doc.getElementById(b.getAttribute('aria-describedby')!)
    expect(desc?.textContent).toBe('Boxes more than one minor behind.')
    expect(b.getAttribute('aria-expanded')).toBe('false')
    ev(b, 'mouseover')
    expect(tipText()).toBe('Boxes more than one minor behind.')
    expect(win.K2Help.state()).toBe('tip')
    expect(b.getAttribute('aria-expanded')).toBe('true')
    ev(b, 'mouseout')
    expect(tipText()).toBeNull()
  })

  it('keyboard: focus shows, Enter (a click) pins, Esc closes and keeps focus on the (i)', () => {
    const { win, doc, ev, keydown, tipText } = boot()
    doc.body.appendChild(win.K2Help.tip('Short help', { label: 'x' }))
    const b = doc.querySelector('.k2h-btn') as HTMLButtonElement
    b.focus()
    expect(tipText()).toBe('Short help')
    b.click()
    expect(win.K2Help.state()).toBe('pinned')
    ev(b, 'mouseout')
    expect(tipText()).toBe('Short help')
    keydown(b, 'Escape')
    expect(tipText()).toBeNull()
    expect(doc.activeElement).toBe(b)
    // The same (i) again toggles.
    b.click()
    expect(win.K2Help.state()).toBe('pinned')
    b.click()
    expect(win.K2Help.state()).toBe('')
  })

  it('touch: a tap pins (no hover flash), a tap outside closes', () => {
    const { win, doc, ev, tipText } = boot()
    doc.body.appendChild(win.K2Help.tip('Tap help', { label: 'x' }))
    const b = doc.querySelector('.k2h-btn') as HTMLButtonElement
    ev(b, 'pointerdown', { pointerType: 'touch' })
    ev(b, 'mouseover') // the tap's compatibility mouse event
    expect(tipText()).toBeNull()
    b.click()
    expect(win.K2Help.state()).toBe('pinned')
    ev(doc.getElementById('out')!, 'pointerdown', { pointerType: 'touch' })
    expect(tipText()).toBeNull()
    expect(win.K2Help.state()).toBe('')
  })

  it('long text is a popover: it takes focus; Esc and Close give it back to the (i)', () => {
    const { win, doc, keydown } = boot()
    const long = 'First paragraph. '.repeat(10) + '\n\nSecond paragraph.'
    doc.body.appendChild(win.K2Help.tip(long, { label: 'Rollouts', title: 'How rollouts work' }))
    const b = doc.querySelector('.k2h-btn') as HTMLButtonElement
    expect(b.getAttribute('aria-haspopup')).toBe('dialog')
    const pop = doc.getElementById(b.getAttribute('aria-controls')!) as HTMLElement
    expect(pop.getAttribute('role')).toBe('dialog')
    expect(pop.hidden).toBe(true)
    expect([...pop.querySelectorAll('p')].map((p) => p.textContent)).toEqual(['First paragraph. '.repeat(10).trim(), 'Second paragraph.'])
    expect(doc.getElementById(pop.getAttribute('aria-labelledby')!)?.textContent).toBe('How rollouts work')
    b.focus()
    b.click()
    expect(pop.hidden).toBe(false)
    expect(doc.activeElement).toBe(pop)
    expect(win.K2Help.state()).toBe('popover')
    keydown(pop, 'Escape')
    expect(pop.hidden).toBe(true)
    expect(doc.activeElement).toBe(b)
    b.click()
    ;(pop.querySelector('.k2h-close') as HTMLButtonElement).click()
    expect(pop.hidden).toBe(true)
    expect(doc.activeElement).toBe(b)
  })

  it('markup is wired: data-tip and data-help with its own popover', async () => {
    const { win, doc, ev, tipText } = boot(
      '<button class="k2h-btn" data-tip="From markup">i</button><button class="k2h-btn" id="h" data-help="hp">i</button>' +
        '<div id="hp" class="k2h-pop" hidden><p>Popover from markup</p><button class="k2h-close">Close</button></div>',
    )
    // A library runs in the frame's head: it wires the page once it is parsed.
    await new Promise((r) => setTimeout(r, 0))
    const tipBtn = doc.querySelector('[data-tip]') as HTMLButtonElement
    expect(tipBtn.getAttribute('aria-describedby')).toBeTruthy()
    ev(tipBtn, 'mouseover')
    expect(tipText()).toBe('From markup')
    const h = doc.getElementById('h') as HTMLButtonElement
    expect(h.getAttribute('aria-controls')).toBe('hp')
    h.click()
    expect(doc.getElementById('hp')!.hidden).toBe(false)
    ev(doc.getElementById('out')!, 'pointerdown', { pointerType: 'mouse' })
    expect(doc.getElementById('hp')!.hidden).toBe(true)
    expect(win.K2Help.state()).toBe('')
  })

  it('a card redraw: the pinned tip follows the new (i) with the same key', async () => {
    const { win, doc } = boot()
    const row = doc.createElement('div')
    doc.body.appendChild(row)
    row.appendChild(win.K2Help.tip('Row help', { label: 'row', key: 'r1' }))
    ;(row.querySelector('.k2h-btn') as HTMLButtonElement).click()
    expect(win.K2Help.state()).toBe('pinned')
    row.replaceChildren(win.K2Help.tip('Row help', { label: 'row', key: 'r1' }))
    await new Promise((r) => setTimeout(r, 0))
    const nb = row.querySelector('.k2h-btn') as HTMLButtonElement
    expect(win.K2Help.state()).toBe('pinned')
    expect(nb.getAttribute('aria-expanded')).toBe('true')
    row.replaceChildren()
    await new Promise((r) => setTimeout(r, 0))
    expect(win.K2Help.state()).toBe('')
  })

  it('stays inside the frame: below the (i), above when there is no room, clamped at the sides', () => {
    const { win, doc, ev } = boot()
    doc.body.appendChild(win.K2Help.tip('Edge help', { label: 'x' }))
    const b = doc.querySelector('.k2h-btn') as HTMLButtonElement
    Object.defineProperty(doc.documentElement, 'clientWidth', { configurable: true, value: 300 })
    Object.defineProperty(doc.documentElement, 'clientHeight', { configurable: true, value: 200 })
    const proto = win.HTMLElement.prototype as HTMLElement
    Object.defineProperty(proto, 'offsetWidth', { configurable: true, get: () => 120 })
    Object.defineProperty(proto, 'offsetHeight', { configurable: true, get: () => 40 })
    const at = (left: number, top: number): void => {
      b.getBoundingClientRect = () => ({ left, top, right: left + 18, bottom: top + 18, width: 18, height: 18, x: left, y: top, toJSON: () => ({}) }) as DOMRect
    }
    at(290, 20)
    ev(b, 'mouseover')
    const tip = doc.querySelector('.k2h-tip') as HTMLElement
    expect([tip.style.left, tip.style.top, tip.getAttribute('data-k2h-side')]).toEqual(['172px', '44px', 'below'])
    ev(b, 'mouseout')
    at(0, 170)
    ev(b, 'mouseover')
    expect([tip.style.left, tip.style.top, tip.getAttribute('data-k2h-side')]).toEqual(['8px', '124px', 'above'])
  })

  it('follows the Garden’s light/dark (--zen-* and data-zen-scheme), never the computer’s', () => {
    const { doc } = boot()
    const css = doc.querySelector('style[data-k2-lib="k2-help"]')!.textContent ?? ''
    expect(css).toContain('var(--zen-surface-raised,#ffffff)')
    expect(css).toContain(':root[data-zen-scheme="dark"] .k2h-tip')
    expect(css).not.toContain('prefers-color-scheme')
  })
})
