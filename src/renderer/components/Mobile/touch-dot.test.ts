// @vitest-environment jsdom
// The touch dot outside React (fix 4), fail loud: place / lift / clear set
// transform, scale, opacity and the data attribute; the fade timer clears
// it; a re-attach of the same span never resets a dot mid-drag.

import { describe, expect, it } from 'vitest'
import { DOT_FADE_MS, TouchDot, type DotTimers } from './touch-dot'

function fakeTimers(): DotTimers & { fire(): void; pending(): number; lastMs: number | null } {
  const q = new Map<number, () => void>()
  let next = 1
  const t = {
    lastMs: null as number | null,
    set(fn: () => void, ms: number): number {
      t.lastMs = ms
      q.set(next, fn)
      return next++
    },
    clear(id: number): void {
      q.delete(id)
    },
    fire(): void {
      const fns = [...q.values()]
      q.clear()
      for (const f of fns) f()
    },
    pending: () => q.size,
  }
  return t
}

describe('TouchDot', () => {
  it('starts hidden; a finger down shows it at the point; a move follows it', () => {
    const el = document.createElement('span')
    const dot = new TouchDot(fakeTimers())
    dot.attach(el)
    expect(el.style.display).toBe('none')
    expect(el.hasAttribute('data-mobile-touch-dot')).toBe(false)
    dot.place(40, 60, true)
    expect(el.style.display).toBe('')
    expect(el.style.transform).toBe('translate3d(40px, 60px, 0) translate(-50%, -50%)')
    expect(el.style.scale).toBe('1')
    expect(el.style.opacity).toBe('1')
    expect(el.getAttribute('data-mobile-touch-dot')).toBe('down')
    dot.place(45, 90, true)
    expect(el.style.transform).toBe('translate3d(45px, 90px, 0) translate(-50%, -50%)')
  })

  it('a lift scales up and fades, and the fade timer clears it', () => {
    const el = document.createElement('span')
    const timers = fakeTimers()
    const dot = new TouchDot(timers)
    dot.attach(el)
    dot.place(10, 10, true)
    dot.place(12, 14, false)
    expect(el.getAttribute('data-mobile-touch-dot')).toBe('up')
    expect(el.style.scale).toBe('1.5')
    expect(el.style.opacity).toBe('0')
    expect(timers.lastMs).toBe(DOT_FADE_MS)
    expect(timers.pending()).toBe(1)
    timers.fire()
    expect(el.style.display).toBe('none')
    expect(el.hasAttribute('data-mobile-touch-dot')).toBe(false)
  })

  it('a new touch inside the fade cancels it; re-attaching the same span keeps the dot', () => {
    const el = document.createElement('span')
    const timers = fakeTimers()
    const dot = new TouchDot(timers)
    dot.attach(el)
    dot.place(1, 1, false)
    dot.place(2, 2, true)
    expect(timers.pending()).toBe(0)
    dot.attach(el)
    expect(el.getAttribute('data-mobile-touch-dot')).toBe('down')
    dot.attach(null)
    dot.place(3, 3, true) // no span: nothing to do, nothing thrown
    expect(el.style.transform).toBe('translate3d(2px, 2px, 0) translate(-50%, -50%)')
  })
})
