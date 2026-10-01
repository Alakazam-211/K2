// Home M4 — room tiers (MS24, Q7), driven by fake timers.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import {
  DEFAULT_ROOM_TIER_CONFIG,
  computeRoomTiers,
  createRoomTierManager,
  type RoomTierChange,
  type RoomTierManager,
} from './room-tiers'

const MIN = 60_000

let mgr: RoomTierManager
let changes: RoomTierChange[]

function make(config?: Parameters<typeof createRoomTierManager>[0]['config']): RoomTierManager {
  const m = createRoomTierManager({
    now: () => Date.now(),
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
    config,
  })
  m.onTierChange((c) => changes.push(c))
  return m
}

function tiers(m: RoomTierManager, keys: string[]): string[] {
  return keys.map((k) => m.tier(k))
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-10-01T00:00:00Z'))
  changes = []
  mgr = make()
})

afterEach(() => {
  mgr.dispose()
  vi.useRealTimers()
})

describe('room tiers', () => {
  it('uses the MS24 numbers: 3 hot, 6 warm, 5 min, 30 min', () => {
    expect(DEFAULT_ROOM_TIER_CONFIG).toEqual({
      hotCap: 3,
      warmCap: 6,
      hotGraceMs: 5 * MIN,
      coldAfterMs: 30 * MIN,
    })
  })

  it('an unknown or never-shown room is cold', () => {
    expect(mgr.tier('b|p:w')).toBe('cold')
  })

  it('shown → hot; off screen 5 min → warm; 30 min → cold, each emitted once', () => {
    mgr.show('B')
    expect(mgr.tier('B')).toBe('hot')
    expect(changes).toEqual([{ roomKey: 'B', from: 'cold', to: 'hot' }])

    mgr.hide('B')
    expect(mgr.tier('B')).toBe('hot')
    vi.advanceTimersByTime(5 * MIN - 1)
    expect(mgr.tier('B')).toBe('hot')
    vi.advanceTimersByTime(1)
    expect(mgr.tier('B')).toBe('warm')

    vi.advanceTimersByTime(25 * MIN - 1)
    expect(mgr.tier('B')).toBe('warm')
    vi.advanceTimersByTime(1)
    expect(mgr.tier('B')).toBe('cold')

    expect(changes).toEqual([
      { roomKey: 'B', from: 'cold', to: 'hot' },
      { roomKey: 'B', from: 'hot', to: 'warm' },
      { roomKey: 'B', from: 'warm', to: 'cold' },
    ])
    // No timer left once everything is cold.
    expect(vi.getTimerCount()).toBe(0)
  })

  it('showing it again within the grace keeps it hot and resets the clock', () => {
    mgr.show('B')
    mgr.hide('B')
    vi.advanceTimersByTime(4 * MIN)
    mgr.show('B')
    vi.advanceTimersByTime(10 * MIN)
    expect(mgr.tier('B')).toBe('hot')
    mgr.hide('B')
    vi.advanceTimersByTime(5 * MIN)
    expect(mgr.tier('B')).toBe('warm')
    expect(changes.map((c) => `${c.from}>${c.to}`)).toEqual(['cold>hot', 'hot>warm'])
  })

  it('a fourth hot room demotes the least recently shown to warm', () => {
    for (const k of ['R1', 'R2', 'R3']) {
      mgr.show(k)
      vi.advanceTimersByTime(1_000)
      mgr.hide(k)
    }
    expect(tiers(mgr, ['R1', 'R2', 'R3'])).toEqual(['hot', 'hot', 'hot'])
    mgr.show('R4')
    expect(tiers(mgr, ['R1', 'R2', 'R3', 'R4'])).toEqual(['warm', 'hot', 'hot', 'hot'])
    expect(changes).toContainEqual({ roomKey: 'R1', from: 'hot', to: 'warm' })
  })

  it('a cap-demoted room does not climb back when a hot room closes', () => {
    for (const k of ['R1', 'R2', 'R3', 'R4']) {
      mgr.show(k)
      vi.advanceTimersByTime(1_000)
      mgr.hide(k)
    }
    expect(mgr.tier('R1')).toBe('warm')
    mgr.close('R4')
    expect(changes.at(-1)).toEqual({ roomKey: 'R4', from: 'hot', to: 'cold' })
    expect(mgr.tier('R1')).toBe('warm')
    expect(mgr.tier('R4')).toBe('cold')
    // Only showing it again makes it hot.
    mgr.show('R1')
    expect(mgr.tier('R1')).toBe('hot')
  })

  it('a demoted-by-cap room still goes cold on its own 30 min clock', () => {
    for (const k of ['R1', 'R2', 'R3', 'R4']) {
      mgr.show(k)
      mgr.hide(k)
    }
    expect(mgr.tier('R1')).toBe('warm')
    vi.advanceTimersByTime(30 * MIN)
    expect(tiers(mgr, ['R1', 'R2', 'R3', 'R4'])).toEqual(['cold', 'cold', 'cold', 'cold'])
  })

  it('a seventh warm room demotes the least recent warm room to cold', () => {
    const keys = ['W1', 'W2', 'W3', 'W4', 'W5', 'W6', 'W7']
    for (const k of keys) {
      mgr.show(k)
      vi.advanceTimersByTime(1_000)
      mgr.hide(k)
    }
    // 7 shown in a row: the newest 3 hot, the next 4 warm, nothing cold yet.
    expect(tiers(mgr, keys)).toEqual(['warm', 'warm', 'warm', 'warm', 'hot', 'hot', 'hot'])
    vi.advanceTimersByTime(5 * MIN)
    // All 7 past the grace: 6 warm fit, the oldest goes cold.
    expect(tiers(mgr, keys)).toEqual(['cold', 'warm', 'warm', 'warm', 'warm', 'warm', 'warm'])
  })

  it('visible rooms are always hot, even past the cap', () => {
    for (const k of ['V1', 'V2', 'V3', 'V4']) mgr.show(k)
    expect(tiers(mgr, ['V1', 'V2', 'V3', 'V4'])).toEqual(['hot', 'hot', 'hot', 'hot'])
  })

  it('close emits a change to cold and forgets the room; closing a cold room emits nothing', () => {
    mgr.show('B')
    mgr.close('B')
    expect(changes.at(-1)).toEqual({ roomKey: 'B', from: 'hot', to: 'cold' })
    expect(mgr.store.getState().rooms).toEqual({})
    mgr.show('C')
    mgr.hide('C')
    vi.advanceTimersByTime(30 * MIN)
    const before = changes.length
    mgr.close('C')
    expect(changes.length).toBe(before)
    mgr.close('never-seen')
    expect(changes.length).toBe(before)
  })

  it('the numbers are configurable (placeholders, Q7)', () => {
    mgr.dispose()
    changes = []
    mgr = make({ hotCap: 1, hotGraceMs: 1 * MIN, coldAfterMs: 2 * MIN })
    mgr.show('A')
    mgr.hide('A')
    mgr.show('B')
    expect(mgr.tier('A')).toBe('warm')
    mgr.hide('B')
    vi.advanceTimersByTime(1 * MIN)
    expect(mgr.tier('B')).toBe('warm')
    vi.advanceTimersByTime(1 * MIN)
    expect(tiers(mgr, ['A', 'B'])).toEqual(['cold', 'cold'])

    // reconfigure re-applies right away.
    mgr.show('C')
    mgr.hide('C')
    vi.advanceTimersByTime(30_000)
    expect(mgr.tier('C')).toBe('hot')
    mgr.reconfigure({ hotGraceMs: 10_000 })
    expect(mgr.tier('C')).toBe('warm')
    expect(mgr.config().hotGraceMs).toBe(10_000)
  })

  it('refuses nonsense numbers', () => {
    expect(() => make({ hotGraceMs: -1 })).toThrow(/hotGraceMs/)
    expect(() => make({ hotGraceMs: 10 * MIN, coldAfterMs: 5 * MIN })).toThrow(/coldAfterMs/)
    expect(() => mgr.reconfigure({ warmCap: Number.NaN })).toThrow(/warmCap/)
  })

  it('computeRoomTiers is pure and deterministic', () => {
    const now = 1_000_000
    const entries = [
      { roomKey: 'a', tier: 'hot' as const, visible: true, hiddenAt: null, shownAt: now, shownSeq: 2, ceiling: 'hot' as const },
      { roomKey: 'b', tier: 'hot' as const, visible: false, hiddenAt: now - 6 * MIN, shownAt: now - 7 * MIN, shownSeq: 1, ceiling: 'hot' as const },
      { roomKey: 'c', tier: 'cold' as const, visible: false, hiddenAt: null, shownAt: null, shownSeq: 0, ceiling: 'cold' as const },
    ]
    const out = computeRoomTiers(entries, now, DEFAULT_ROOM_TIER_CONFIG)
    expect(out).toEqual({ a: 'hot', b: 'warm', c: 'cold' })
    expect(computeRoomTiers(entries, now, DEFAULT_ROOM_TIER_CONFIG)).toEqual(out)
  })
})
