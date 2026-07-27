import { describe, expect, it } from 'vitest'
import { asArray } from './as-array'

describe('asArray', () => {
  it('passes arrays through', () => {
    const a = [1, 2]
    expect(asArray(a)).toBe(a)
  })

  it('unwraps common list wrappers', () => {
    expect(asArray({ items: ['a'] })).toEqual(['a'])
    expect(asArray({ entries: ['b'] })).toEqual(['b'])
    expect(asArray({ users: ['c'] })).toEqual(['c'])
  })

  it('soft-empties non-arrays (the remote crash case)', () => {
    expect(asArray(undefined)).toEqual([])
    expect(asArray(null)).toEqual([])
    expect(asArray({})).toEqual([])
    expect(asArray('nope')).toEqual([])
    // Spreading the result must never throw.
    expect([...asArray({ agents: [] })]).toEqual([])
  })
})
