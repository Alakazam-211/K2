// @vitest-environment jsdom
// The touch dot outside React (fix 4), fail loud: place / lift / clear set
// transform, scale, opacity and the data attribute; the fade timer clears
// it; a re-attach of the same span never resets a dot mid-drag.

import { describe, expect, it } from 'vitest'
import { DOT_FADE_MS, dotTransform, TouchDot, type DotTimers } from './touch-dot'

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
  it('z3mbpZ: the lift scale never moves the dot (scale after the translate, no separate scale property)', () => {
    expect(dotTransform(100, 200, 1)).toBe('translate3d(100px, 200px, 0) translate(-50%, -50%)')
    expect(dotTransform(100, 200, 1.5)).toBe('translate3d(100px, 200px, 0) translate(-50%, -50%) scale(1.5)')
    const el = document.createElement('span')
    const dot = new TouchDot(fakeTimers())
    dot.attach(el)
    el.style.scale = '1.5' // left over from an older build
    dot.place(100, 200, true)
    expect(el.style.scale).toBe('')
  })

  it('starts hidden; a finger down shows it at the point; a move follows it', () => {
    const el = document.createElement('span')
    const dot = new TouchDot(fakeTimers())
    dot.attach(el)
    expect(el.style.display).toBe('none')
    expect(el.hasAttribute('data-mobile-touch-dot')).toBe(false)
    dot.place(40, 60, true)
    expect(el.style.display).toBe('')
    expect(el.style.transform).toBe('translate3d(40px, 60px, 0) translate(-50%, -50%)')
    expect(el.style.scale, 'never the separate scale property (it would scale the translate)').toBe('')
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
    // Scaled in place: the scale comes after the translate, inside transform.
    expect(el.style.transform).toBe('translate3d(12px, 14px, 0) translate(-50%, -50%) scale(1.5)')
    expect(el.style.scale).toBe('')
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
