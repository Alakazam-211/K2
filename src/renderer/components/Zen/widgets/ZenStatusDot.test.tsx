// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 Q4/S7: Zen keeps its circles, mapped to the
// app's states and rank (needs you > failed > done > working > monitoring >
// no update > idle), one shape per state, each labelled.

import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ZenStatusDot, zenDotState } from './ZenAgentsWidget'
import type { ZenAgentRow } from '@/lib/zen/zen-data'

afterEach(cleanup)

type Dot = Pick<ZenAgentRow, 'activity' | 'unseen'>

describe('zenDotState', () => {
  it('ranks like the app', () => {
    const cases: Array<[Dot, string | null]> = [
      [{ activity: null, unseen: 'failed' }, null],
      [{ activity: 'needs-you', unseen: 'failed' }, 'needs-you'],
      [{ activity: 'monitoring', unseen: 'failed' }, 'failed'],
      [{ activity: 'monitoring', unseen: 'done' }, 'done'],
      [{ activity: 'idle', unseen: 'done' }, 'done'],
      [{ activity: 'working', unseen: null }, 'working'],
      [{ activity: 'monitoring', unseen: null }, 'monitoring'],
      [{ activity: 'unverifiable' }, 'unverifiable'],
      [{ activity: 'idle', unseen: null }, 'idle'],
    ]
    for (const [row, want] of cases) expect([row, zenDotState(row)]).toEqual([row, want])
  })
})

describe('ZenStatusDot', () => {
  function dot(row: Dot): Element {
    const { container } = render(<ZenStatusDot row={row as ZenAgentRow} />)
    const el = container.querySelector('[data-zen-status-dot]')
    if (!el) throw new Error('no dot')
    return el
  }

  it('failed and done are rings with × and ✓ (shape, not colour alone)', () => {
    const failed = dot({ activity: 'idle', unseen: 'failed' })
    expect(failed.getAttribute('data-zen-status-dot')).toBe('failed')
    expect(failed.getAttribute('aria-label')).toBe('Failed (not viewed yet)')
    expect(failed.querySelector('circle')?.getAttribute('stroke')).toBe('var(--zen-danger)')
    expect(failed.querySelector('path')?.getAttribute('d')).toContain('M7.8 4.2')
    cleanup()
    const done = dot({ activity: 'idle', unseen: 'done' })
    expect(done.getAttribute('aria-label')).toBe('Done (not viewed yet)')
    expect(done.querySelector('path')?.getAttribute('d')).toBe('M3.7 6.2 L5.3 7.8 L8.4 4.4')
  })

  it('working pulses, needs you is filled, both labelled; circles stay round', () => {
    const working = dot({ activity: 'working', unseen: null }) as HTMLElement
    expect(working.hasAttribute('data-zen-pulse')).toBe(true)
    expect(working.style.borderRadius).toBe('999px')
    expect(working.getAttribute('aria-label')).toBe('Working')
    cleanup()
    const needs = dot({ activity: 'needs-you', unseen: null }) as HTMLElement
    expect(needs.style.background).toBe('var(--zen-needs-you)')
  })
})
