import { describe, expect, it } from 'vitest'

import {
  BASE_RETAINED_CAP,
  computeRetainedSet,
  pruneOrderToActive,
  recordVisitOrder,
  retainedCap,
  seedBootOrder,
} from './retainedChat'

const activeSet = (...ids: string[]): ReadonlySet<string> => new Set(ids)

describe('retainedCap — max(base, pinnedToTop)', () => {
  it('base cap is 5', () => {
    expect(BASE_RETAINED_CAP).toBe(5)
  })

  it('pinned count at or below the base never shrinks the cap', () => {
    expect(retainedCap(0)).toBe(5)
    expect(retainedCap(1)).toBe(5)
    expect(retainedCap(5)).toBe(5)
  })

  it('pinned count above the base grows the cap to the pinned count', () => {
    expect(retainedCap(6)).toBe(6)
    expect(retainedCap(9)).toBe(9)
  })

  it('honors an explicit base cap', () => {
    expect(retainedCap(2, 3)).toBe(3)
    expect(retainedCap(7, 3)).toBe(7)
  })
})

describe('recordVisitOrder — MRU move-to-front', () => {
  it('a first visit prepends', () => {
    expect(recordVisitOrder([], 'a')).toEqual(['a'])
    expect(recordVisitOrder(['b', 'c'], 'a')).toEqual(['a', 'b', 'c'])
  })

  it('re-visiting moves to front and preserves the rest of the order', () => {
    expect(recordVisitOrder(['a', 'b', 'c'], 'c')).toEqual(['c', 'a', 'b'])
    expect(recordVisitOrder(['a', 'b', 'c'], 'b')).toEqual(['b', 'a', 'c'])
  })

  it('visiting the current front is idempotent on order', () => {
    expect(recordVisitOrder(['a', 'b'], 'a')).toEqual(['a', 'b'])
  })

  it('never mutates its input', () => {
    const input = ['a', 'b']
    recordVisitOrder(input, 'b')
    expect(input).toEqual(['a', 'b'])
  })
})

describe('computeRetainedSet — Active-gated, MRU-first, capped', () => {
  it('foreground (MRU front) is always retained when Active', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['fg'],
        activeProjectIds: activeSet('fg'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['fg'])
  })

  it('non-Active entries are excluded (Active-leave ⇒ detach)', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['a', 'b', 'c'],
        activeProjectIds: activeSet('a', 'c'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['a', 'c'])
  })

  it('a skipped non-Active entry does NOT count against the cap', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['dead', 'a', 'b'],
        activeProjectIds: activeSet('a', 'b'),
        pinnedToTopCount: 0,
        baseCap: 2,
      }),
    ).toEqual(['a', 'b'])
  })

  it('caps at 5 by default, evicting the least-recently-visited', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['f', 'e', 'd', 'c', 'b', 'a'],
        activeProjectIds: activeSet('a', 'b', 'c', 'd', 'e', 'f'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['f', 'e', 'd', 'c', 'b']) // 'a' (visited longest ago) evicted
  })

  it('eviction order is strictly least-recently-visited-first', () => {
    // Re-visiting 'a' rescues it; 'b' becomes the eviction victim.
    const order = recordVisitOrder(['f', 'e', 'd', 'c', 'b', 'a'], 'a')
    expect(
      computeRetainedSet({
        mruOrder: order,
        activeProjectIds: activeSet('a', 'b', 'c', 'd', 'e', 'f'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['a', 'f', 'e', 'd', 'c'])
  })

  it('pinned-to-top growth: cap = max(5, pinnedToTopCount)', () => {
    const seven = ['g', 'f', 'e', 'd', 'c', 'b', 'a']
    expect(
      computeRetainedSet({
        mruOrder: seven,
        activeProjectIds: activeSet(...seven),
        pinnedToTopCount: 7,
      }),
    ).toEqual(seven) // cap grew to 7 — nothing evicted
    expect(
      computeRetainedSet({
        mruOrder: seven,
        activeProjectIds: activeSet(...seven),
        pinnedToTopCount: 3, // ≤5 pins do NOT shrink the cap below 5
      }),
    ).toEqual(['g', 'f', 'e', 'd', 'c'])
  })

  it('empty Active mirror retains nothing (boot-transient safe)', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['a', 'b'],
        activeProjectIds: activeSet(),
        pinnedToTopCount: 0,
      }),
    ).toEqual([])
  })

  it('preserves MRU order in the result', () => {
    expect(
      computeRetainedSet({
        mruOrder: ['c', 'a', 'b'],
        activeProjectIds: activeSet('a', 'b', 'c'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['c', 'a', 'b'])
  })
})

describe('seedBootOrder — display-order only (does not spawn)', () => {
  it('seeds Active-list order into an empty session, bounded by cap', () => {
    expect(seedBootOrder([], ['p1', 'p2', 'p3'], 5)).toEqual(['p1', 'p2', 'p3'])
    expect(seedBootOrder([], ['p1', 'p2', 'p3', 'p4', 'p5', 'p6'], 5)).toEqual([
      'p1',
      'p2',
      'p3',
      'p4',
      'p5',
    ])
  })

  it('real visits stay in front; seeds append behind them', () => {
    expect(seedBootOrder(['fg'], ['p1', 'p2'], 5)).toEqual(['fg', 'p1', 'p2'])
  })

  it('a seed never duplicates or reorders an existing visit', () => {
    expect(seedBootOrder(['p2'], ['p1', 'p2', 'p3'], 5)).toEqual(['p2', 'p1', 'p3'])
  })

  it('existing visits count against the cap', () => {
    expect(seedBootOrder(['fg'], ['p1', 'p2', 'p3', 'p4', 'p5'], 5)).toEqual([
      'fg',
      'p1',
      'p2',
      'p3',
      'p4',
    ])
  })

  it('never mutates its inputs', () => {
    const existing = ['fg']
    const boot = ['p1']
    seedBootOrder(existing, boot, 5)
    expect(existing).toEqual(['fg'])
    expect(boot).toEqual(['p1'])
  })
})

describe('pruneOrderToActive — Active-leave drops the visit', () => {
  it('drops entries that left the Active section, preserving order', () => {
    expect(pruneOrderToActive(['a', 'b', 'c'], activeSet('a', 'c'))).toEqual([
      'a',
      'c',
    ])
  })

  it('a pruned workspace re-joining Active does NOT reappear (no auto-attach)', () => {
    const pruned = pruneOrderToActive(['a', 'b'], activeSet('a'))
    expect(pruned).toEqual(['a'])
    // Re-join: 'b' is Active again, but only a visit re-adds it.
    expect(
      computeRetainedSet({
        mruOrder: pruned,
        activeProjectIds: activeSet('a', 'b'),
        pinnedToTopCount: 0,
      }),
    ).toEqual(['a'])
  })

  it('empty Active mirror prunes everything', () => {
    expect(pruneOrderToActive(['a'], activeSet())).toEqual([])
  })
})
