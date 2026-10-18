import { describe, it, expect } from 'vitest'
import { assignOrders, isValidOrder, orderBetween } from './fractional-index'

describe('fractional-index (homes-sync-v1 order keys)', () => {
  it('matches the Rust helper on the shared fixtures', () => {
    // Same values as k2_core::homes_sync::tests::order_between_matches_the_client_helper.
    expect(orderBetween(null, null)).toBe('V')
    expect(orderBetween('V', null)).toBe('l')
    expect(orderBetween(null, 'V')).toBe('G')
    expect(orderBetween('z', null)).toBe('zV')
    expect(orderBetween('a', 'b')).toBe('aV')
    expect(orderBetween('a1', 'a2')).toBe('a1V')
    expect(orderBetween(null, '05')).toBe('03')
  })

  it('refuses bad input loudly', () => {
    expect(() => orderBetween('b', 'a')).toThrow(/out of order/)
    expect(() => orderBetween('a', 'a')).toThrow(/out of order/)
    expect(() => orderBetween('a0', null)).toThrow(/invalid/)
    expect(() => orderBetween('a.b', null)).toThrow(/invalid/)
    expect(isValidOrder('')).toBe(false)
    expect(isValidOrder('V')).toBe(true)
  })

  it('stays strictly ordered over long runs of appends, prepends and squeezes', () => {
    const keys = [orderBetween(null, null)]
    for (let i = 0; i < 200; i++) {
      keys.push(orderBetween(keys[keys.length - 1], null))
      keys.unshift(orderBetween(null, keys[0]))
    }
    for (let i = 1; i < keys.length; i++) {
      expect(keys[i - 1] < keys[i]).toBe(true)
      expect(isValidOrder(keys[i])).toBe(true)
    }
    let lo = 'V'
    for (let i = 0; i < 100; i++) {
      const m = orderBetween(lo, 'W')
      expect(lo < m && m < 'W' && isValidOrder(m)).toBe(true)
      lo = m
    }
  })

  it('assignOrders keeps every key it can: one move rewrites one key', () => {
    const known: Record<string, string> = { a: 'G', b: 'V', c: 'l', d: 't' }
    // Move d to the front.
    const list = ['d', 'a', 'b', 'c']
    const out = assignOrders(list.length, (i) => known[list[i]] ?? null)
    expect(out.slice(1)).toEqual(['G', 'V', 'l'])
    expect(out[0] < 'G').toBe(true)
    // New entries go between their neighbours; unknown keys are filled.
    const list2 = ['a', 'x', 'b', 'y']
    const out2 = assignOrders(list2.length, (i) => known[list2[i]] ?? null)
    expect(out2[0]).toBe('G')
    expect(out2[2]).toBe('V')
    expect('G' < out2[1] && out2[1] < 'V').toBe(true)
    expect(out2[3] > 'V').toBe(true)
    // Duplicate keys (two devices inserted at the same spot) get separated.
    const dup = assignOrders(3, () => 'V')
    expect(dup[0] < dup[1] && dup[1] < dup[2]).toBe(true)
    // No keys at all: a fresh increasing run.
    const fresh = assignOrders(4, () => null)
    expect([...fresh].sort()).toEqual(fresh)
    expect(new Set(fresh).size).toBe(4)
  })
})
