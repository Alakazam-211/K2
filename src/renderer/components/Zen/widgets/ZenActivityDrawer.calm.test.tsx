// @vitest-environment jsdom
// Rosson 2026-11-23 (0.46.17): "the activity ear (the activity area that
// sits above the message box for an agent), when it's open, is just jumping
// up and down a lot more", and "if it jumps around in height, it makes it
// hard to read the messages above."
//
// The open ear sized itself to its words on every frame: the step went from
// one line ("Thinking…") to two or three (a long command) and away (once
// the agent replied), the tally and subagent lines came and went. Now, open:
// a burst of frames for one running turn paints at most once per
// ZEN_DRAWER_COALESCE_MS, its height only grows during the turn, and each
// line stops at a few lines (a ceiling for the growing).
//
// jsdom lays nothing out: the open ear's height here is its lines, the way a
// browser would stack them (a step of N characters wraps every 40), and its
// own min-height when that is taller.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import {
  ZEN_DRAWER_COALESCE_MS,
  ZEN_DRAWER_EXPANDED_KEY,
  ZEN_DRAWER_NOTE_LINES,
  ZEN_DRAWER_STEP_LINES,
  ZenActivityDrawer,
} from './ZenActivityDrawer'
import { LONG, T0, burst, earRectPx, frame, naturalEarPx } from '@/test-utils/activity-ear'

let rectSaved: PropertyDescriptor | undefined

beforeEach(() => {
  vi.useFakeTimers()
  localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, '1')
  rectSaved = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect')
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const height = this.hasAttribute?.('data-zen-drawer-detail') ? earRectPx(this) : 0
      return { top: 0, bottom: height, left: 0, right: 600, width: 600, height, x: 0, y: 0, toJSON: () => ({}) }
    },
  })
})

afterEach(() => {
  cleanup()
  localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
  if (rectSaved) Object.defineProperty(Element.prototype, 'getBoundingClientRect', rectSaved)
  vi.useRealTimers()
})

function detail(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-zen-drawer-detail]')
  if (!el) throw new Error('the ear is not open')
  return el
}

function stepText(): string {
  return document.querySelector('[data-zen-drawer-step-text]')?.textContent ?? '(no step)'
}

describe('the open activity ear stays calm while the agent works', () => {
  it('its height never shrinks during a turn, frame after frame of a burst', () => {
    const first = frame(1, 'thinking', null)
    const { rerender } = render(<ZenActivityDrawer turn={first} />)
    const heights: number[] = [earRectPx(detail())]
    const natural: number[] = [naturalEarPx(detail())]
    for (const f of [...burst(), ...burst()]) {
      act(() => {
        rerender(<ZenActivityDrawer turn={f} />)
        vi.advanceTimersByTime(ZEN_DRAWER_COALESCE_MS + 10)
      })
      heights.push(earRectPx(detail()))
      natural.push(naturalEarPx(detail()))
    }
    // The words alone would have gone up and down (the old behaviour)...
    const turns = natural.slice(1).filter((h, i) => Math.sign(h - natural[i]) !== 0)
    expect(new Set(natural).size, `natural ${natural.join(',')}`).toBeGreaterThan(2)
    expect(turns.length).toBeGreaterThan(3)
    // ...the ear only grows, and settles: no shrink-then-grow.
    for (let i = 1; i < heights.length; i++) {
      expect(heights[i], `frame ${i}: ${heights.join(',')}`).toBeGreaterThanOrEqual(heights[i - 1])
    }
    // The second run of the same burst adds nothing: steady.
    const second = heights.slice(heights.length - burst().length)
    expect(new Set(second).size, `second burst ${second.join(',')}`).toBe(1)
    // And it has a ceiling: head, the step's cap, the tally's and the
    // subagent line's caps.
    const cap = 12 + 18 + ZEN_DRAWER_STEP_LINES * 19 + 1 + 2 * (ZEN_DRAWER_NOTE_LINES * 18 + 1)
    expect(Math.max(...heights)).toBeLessThanOrEqual(cap)
  })

  it('a new turn starts small again; closing and opening starts anew', () => {
    const { rerender } = render(<ZenActivityDrawer turn={frame(1, 'tool', LONG, { tally: { read: 1, cmd: 1 } })} />)
    const tall = earRectPx(detail())
    act(() => {
      rerender(<ZenActivityDrawer turn={frame(1, 'thinking', null, {}, 'calm-2')} />)
    })
    expect(earRectPx(detail())).toBeLessThan(tall)
    expect(detail().style.minHeight).not.toBe(`${tall}px`)
  })

  it('a burst inside one window paints twice at most: the first frame, then the newest', () => {
    const { rerender } = render(<ZenActivityDrawer turn={frame(1, 'thinking', null)} />)
    act(() => {
      vi.advanceTimersByTime(ZEN_DRAWER_COALESCE_MS + 10)
    })
    const painted: string[] = []
    const seen = (): void => {
      const t = stepText()
      if (painted[painted.length - 1] !== t) painted.push(t)
    }
    const run = burst()
    for (const f of run) {
      act(() => {
        rerender(<ZenActivityDrawer turn={f} />)
        vi.advanceTimersByTime(5)
      })
      seen()
    }
    act(() => {
      vi.advanceTimersByTime(ZEN_DRAWER_COALESCE_MS)
    })
    seen()
    // The leading frame, then the newest one (thinking): nothing between.
    expect(painted).toHaveLength(2)
    expect(painted[0]).toContain('src/a.ts')
    expect(painted[1]).toMatch(/Thinking/)
  })

  it('an end is never held back (the ear goes as the turn ends)', () => {
    const { rerender } = render(<ZenActivityDrawer turn={frame(1, 'tool', 'Reading `a`')} />)
    act(() => {
      rerender(<ZenActivityDrawer turn={frame(2, 'tool', 'Reading `b`')} />)
    })
    act(() => {
      rerender(<ZenActivityDrawer turn={frame(3, 'thinking', null, { state: 'stopped', end: { at: T0, reason: 'interrupted' } })} />)
    })
    expect(document.querySelector('[data-zen-drawer-head]')?.textContent).toMatch(/Stopped/)
  })

  it('closed, every frame paints at once (one fixed line: nothing to calm)', () => {
    localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, '0')
    const { rerender } = render(<ZenActivityDrawer turn={frame(1, 'tool', 'Reading `a`')} />)
    for (const f of burst()) {
      act(() => {
        rerender(<ZenActivityDrawer turn={f} />)
      })
      expect(document.querySelector('[data-zen-drawer-tile]')?.getAttribute('aria-label')).toBeTruthy()
    }
    expect(document.querySelector('[data-zen-drawer-step]')?.textContent).toMatch(/Thinking/)
  })

  it('the step, tally and subagent lines each stop at their caps', () => {
    render(
      <ZenActivityDrawer
        turn={frame(1, 'tool', LONG, {
          tally: { read: 9, cmd: 9, search: 9, edit: 9 },
        })}
      />,
    )
    const step = document.querySelector<HTMLElement>('[data-zen-drawer-step-text]')
    const tally = document.querySelector<HTMLElement>('[data-zen-drawer-tally]')
    expect(step?.getAttribute('style') ?? '').toMatch(new RegExp(`line-clamp:\\s*${ZEN_DRAWER_STEP_LINES}`))
    expect(tally?.getAttribute('style') ?? '').toMatch(new RegExp(`line-clamp:\\s*${ZEN_DRAWER_NOTE_LINES}`))
  })
})
