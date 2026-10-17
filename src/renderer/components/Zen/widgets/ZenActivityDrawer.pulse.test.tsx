// @vitest-environment jsdom
// Rosson 2026-10-17: "The pulse for the words and the pulse for the square
// dot to the left of the words: can we make those be in sync?" and "The
// slower of the two is better." The dot and the thinking words play one
// keyframe on one timing (1.6s ease-in-out), each phase-locked to the page
// clock by the same delay rule.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import {
  ZEN_DRAWER_CSS,
  ZEN_DRAWER_EXPANDED_KEY,
  ZEN_DRAWER_PULSE_ANIMATION,
  ZEN_DRAWER_PULSE_MS,
  ZenActivityDrawer,
  zenDrawerPulseDelay,
} from './ZenActivityDrawer'

const T0 = 1_800_000_000_000

function turn(fields: Record<string, unknown>): ThreadTurn {
  const t = coerceThreadTurn(
    {
      state: 'working',
      phase: 'thinking',
      line: '',
      subagents: 0,
      background: 0,
      tally: {},
      end: null,
      rev: 1,
      serverNow: T0,
      turnId: 't1',
      startedAt: T0 - 8_000,
      phaseSince: T0 - 3_000,
      ...fields,
    },
    T0,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

/** The drawer sheet's (non-reduced-motion) `animation` rule naming `selector`. */
function ruleFor(selector: string): string {
  const rule = ZEN_DRAWER_CSS.split('\n').find((l) => l.includes(selector) && l.includes('{ animation:') && !l.includes('none'))
  if (!rule) throw new Error(`no animation rule for ${selector}`)
  return rule
}

/** Where in the period a pulse sits: started at `start` with `delay`. */
function phaseOrigin(start: number, delay: string): number {
  const p = ZEN_DRAWER_PULSE_MS
  return (((start + Number.parseInt(delay, 10)) % p) + p) % p
}

describe('zenDrawerPulseDelay', () => {
  it('is minus the time into the current period of the page clock', () => {
    expect(zenDrawerPulseDelay(0)).toBe('0ms')
    expect(zenDrawerPulseDelay(ZEN_DRAWER_PULSE_MS * 3)).toBe('0ms')
    expect(zenDrawerPulseDelay(ZEN_DRAWER_PULSE_MS * 7 + 250)).toBe('-250ms')
    expect(zenDrawerPulseDelay(1_234.4, 1_000)).toBe('-234ms')
  })
})

describe('the activity ear: the dot and the words share one pulse', () => {
  let clock = 0
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(T0)
    clock = 9 * ZEN_DRAWER_PULSE_MS + 400
    vi.spyOn(performance, 'now').mockImplementation(() => clock)
    localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
  })
  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.useRealTimers()
    localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
  })

  it('uses the slower curve (1.6s ease-in-out) on one keyframe for both', () => {
    expect(ZEN_DRAWER_PULSE_MS).toBe(1600)
    expect(ZEN_DRAWER_PULSE_ANIMATION).toBe('zen-drawer-pulse 1600ms ease-in-out infinite')
    const dotRule = ruleFor('[data-zen-drawer] [data-zen-pulse]')
    const wordsRule = ruleFor('[data-zen-drawer] .zen-drawer-pulse-text')
    expect(dotRule).toBe(wordsRule)
    expect(dotRule).toContain(`{ animation: ${ZEN_DRAWER_PULSE_ANIMATION}; }`)
    expect(ZEN_DRAWER_CSS).toContain('@keyframes zen-drawer-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.45; } }')
    expect(ZEN_DRAWER_CSS).not.toContain('zen-drawer-shimmer')
  })

  it('both are still under reduced motion', () => {
    const reduced = ZEN_DRAWER_CSS.slice(ZEN_DRAWER_CSS.indexOf('@media (prefers-reduced-motion: reduce)'))
    expect(reduced).toMatch(/\.zen-drawer-pulse-text, [^{]*\[data-zen-pulse\] \{ animation: none !important; \}/)
  })

  it('the dot and the words take the same delay from the page clock, collapsed and expanded', () => {
    render(<ZenActivityDrawer turn={turn({})} />)
    const dot = el('[data-zen-drawer-dot]')
    const words = el('[data-zen-drawer-step]')
    expect(dot.hasAttribute('data-zen-pulse')).toBe(true)
    expect(words.classList.contains('zen-drawer-pulse-text')).toBe(true)
    expect(dot.style.animationDelay).toBe('-400ms')
    expect(words.style.animationDelay).toBe(dot.style.animationDelay)

    // Expanding remounts both: they take the clock's phase again, together.
    clock += 700
    act(() => {
      fireEvent.click(el('[data-zen-drawer-tile]'))
    })
    expect(el('[data-zen-drawer]').getAttribute('data-expanded')).toBe('true')
    const words2 = el('[data-zen-drawer-step-text]')
    expect(words2.classList.contains('zen-drawer-pulse-text')).toBe(true)
    expect(el('[data-zen-drawer-dot]').style.animationDelay).toBe('-1100ms')
    expect(words2.style.animationDelay).toBe('-1100ms')
  })

  it('words that start thinking later join the phase the dot already has', () => {
    const { rerender } = render(<ZenActivityDrawer turn={turn({ phase: 'tool', line: 'Running `cargo test`' })} />)
    const dotStart = clock
    expect(el('[data-zen-drawer-dot]').style.animationDelay).toBe('-400ms')
    expect(el('[data-zen-drawer-step]').classList.contains('zen-drawer-pulse-text')).toBe(false)
    expect(el('[data-zen-drawer-step]').style.animationDelay).toBe('')

    // 900ms on, the step turns into thinking: its pulse starts now, so its
    // delay is now's phase; the running dot keeps its own (no jump).
    clock += 900
    rerender(<ZenActivityDrawer turn={turn({ rev: 2 })} />)
    expect(el('[data-zen-drawer-step]').classList.contains('zen-drawer-pulse-text')).toBe(true)
    expect(el('[data-zen-drawer-dot]').style.animationDelay).toBe('-400ms')
    expect(el('[data-zen-drawer-step]').style.animationDelay).toBe('-1300ms')
    expect(phaseOrigin(clock, '-1300ms')).toBe(phaseOrigin(dotStart, '-400ms'))
  })
})
