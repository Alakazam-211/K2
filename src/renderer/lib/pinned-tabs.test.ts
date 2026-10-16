import { describe, expect, it } from 'vitest'
import {
  clampReorderTarget,
  clampTabDropIndex,
  isPinnedTab,
  orderPinnedTabs,
  pinnedBands,
  withTabPinned,
} from '@/lib/pinned-tabs'

interface T {
  id: string
  isSystemAgent?: boolean
  isPinnedFile?: boolean
}

const sys = (id: string): T => ({ id, isSystemAgent: true })
const pin = (id: string): T => ({ id, isPinnedFile: true })
const reg = (id: string): T => ({ id })
const ids = (tabs: T[]): string[] => tabs.map((t) => t.id)
const mark = (t: T, on: boolean): T => {
  const { isPinnedFile: _was, ...rest } = t
  return on ? { ...rest, isPinnedFile: true } : rest
}

describe('isPinnedTab', () => {
  it('is the pin flag, never a system tab', () => {
    expect(isPinnedTab(pin('a'))).toBe(true)
    expect(isPinnedTab(reg('a'))).toBe(false)
    expect(isPinnedTab({ isSystemAgent: true, isPinnedFile: true })).toBe(false)
    expect(isPinnedTab(null)).toBe(false)
  })
})

describe('orderPinnedTabs', () => {
  it('returns the same array when already [system][pinned][regular]', () => {
    const tabs = [sys('chat'), sys('inbox'), pin('p1'), pin('p2'), reg('r1')]
    expect(orderPinnedTabs(tabs)).toBe(tabs)
  })

  it('stable-sorts out-of-order tabs into the three bands', () => {
    const tabs = [reg('r1'), pin('p1'), sys('chat'), reg('r2'), pin('p2'), sys('inbox')]
    expect(ids(orderPinnedTabs(tabs))).toEqual(['chat', 'inbox', 'p1', 'p2', 'r1', 'r2'])
  })
})

describe('clampTabDropIndex (drag indicator, insert-before index)', () => {
  const tabs = [sys('chat'), sys('inbox'), pin('p1'), pin('p2'), reg('r1'), reg('r2')]

  it('keeps a pinned tab among the pinned tabs', () => {
    expect(clampTabDropIndex(tabs, 3, 0)).toBe(2)
    expect(clampTabDropIndex(tabs, 2, 6)).toBe(4)
    expect(clampTabDropIndex(tabs, 3, 2)).toBe(2)
  })

  it('keeps a regular tab among the regular tabs', () => {
    expect(clampTabDropIndex(tabs, 5, 0)).toBe(4)
    expect(clampTabDropIndex(tabs, 4, 3)).toBe(4)
    expect(clampTabDropIndex(tabs, 4, 6)).toBe(6)
  })

  it('refuses a system tab', () => {
    expect(clampTabDropIndex(tabs, 0, 3)).toBeNull()
  })
})

describe('clampReorderTarget (index after removal)', () => {
  const tabs = [sys('chat'), pin('p1'), pin('p2'), reg('r1'), reg('r2')]

  it('clamps pinned and regular moves into their own band', () => {
    expect(clampReorderTarget(tabs, 1, 4)).toBe(2)
    expect(clampReorderTarget(tabs, 2, 0)).toBe(1)
    expect(clampReorderTarget(tabs, 4, 0)).toBe(3)
    expect(clampReorderTarget(tabs, 3, 4)).toBe(4)
  })

  it('refuses a system tab', () => {
    expect(clampReorderTarget(tabs, 0, 2)).toBeNull()
  })
})

describe('withTabPinned', () => {
  const tabs = [sys('chat'), pin('p1'), reg('r1'), reg('r2'), reg('r3')]

  it('pins to the end of the pinned group', () => {
    const out = withTabPinned(tabs, 'r3', true, mark)
    expect(ids(out)).toEqual(['chat', 'p1', 'r3', 'r1', 'r2'])
    expect(out[2].isPinnedFile).toBe(true)
    expect(pinnedBands(out)).toEqual({ system: 1, pinned: 2 })
  })

  it('unpins to the first slot after the pinned group', () => {
    const pinned = withTabPinned(withTabPinned(tabs, 'r2', true, mark), 'r3', true, mark)
    expect(ids(pinned)).toEqual(['chat', 'p1', 'r2', 'r3', 'r1'])
    const out = withTabPinned(pinned, 'p1', false, mark)
    expect(ids(out)).toEqual(['chat', 'r2', 'r3', 'p1', 'r1'])
    expect(out[3].isPinnedFile).toBeUndefined()
  })

  it('is a no-op (same array) for system tabs, missing tabs and no change', () => {
    expect(withTabPinned(tabs, 'chat', true, mark)).toBe(tabs)
    expect(withTabPinned(tabs, 'nope', true, mark)).toBe(tabs)
    expect(withTabPinned(tabs, 'p1', true, mark)).toBe(tabs)
    expect(withTabPinned(tabs, 'r1', false, mark)).toBe(tabs)
  })
})
