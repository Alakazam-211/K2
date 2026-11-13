// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { fitTopBar } from './top-bar-fit'

/** A bar `room` CSS px wide whose clusters measure per step: the server
 *  label goes at `compact` (-110), the Usage word at `tight` (-30), the
 *  switcher and the dividers at `minimal` (-50, -10). */
function bar(room: number): { b: HTMLElement; l: HTMLElement; r: HTMLElement } {
  const b = document.createElement('div')
  const l = document.createElement('div')
  const r = document.createElement('div')
  const step = (): string => b.dataset.topBarFit ?? 'full'
  Object.defineProperty(b, 'clientWidth', { get: () => room })
  Object.defineProperty(l, 'scrollWidth', { get: () => ({ full: 400, compact: 290, tight: 290, minimal: 240 })[step()] ?? 400 })
  Object.defineProperty(r, 'offsetWidth', { get: () => ({ full: 180, compact: 180, tight: 150, minimal: 140 })[step()] ?? 180 })
  return { b, l, r }
}

describe('a top bar too narrow for its items', () => {
  it('stays full with room for both clusters', () => {
    const { b, l, r } = bar(700)
    expect(fitTopBar(b, l, r)).toBe('full')
    expect(b.dataset.topBarFit).toBe('full')
  })

  it('first drops the server name, then the Usage word, before the clusters overlap', () => {
    const one = bar(500)
    expect(fitTopBar(one.b, one.l, one.r)).toBe('compact')
    const two = bar(450)
    expect(fitTopBar(two.b, two.l, two.r)).toBe('tight')
  })

  it('as the last resort hides the dividers and the server switcher (an 800 px window at 175 %)', () => {
    const { b, l, r } = bar(400)
    expect(fitTopBar(b, l, r)).toBe('minimal')
  })

  it('comes back as the bar widens', () => {
    const narrow = bar(450)
    expect(fitTopBar(narrow.b, narrow.l, narrow.r)).toBe('tight')
    const wide = bar(800)
    wide.b.dataset.topBarFit = 'tight'
    expect(fitTopBar(wide.b, wide.l, wide.r)).toBe('full')
  })
})
