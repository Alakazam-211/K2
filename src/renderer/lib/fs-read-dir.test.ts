import { describe, expect, it } from 'vitest'
import { normalizeFsReadDir } from './fs-read-dir'

describe('normalizeFsReadDir', () => {
  it('passes through a bare array (daemon wire shape)', () => {
    const rows = [
      { name: 'a', path: '/a', isDirectory: true },
      { name: 'b.txt', path: '/b.txt', isDirectory: false },
    ]
    expect(normalizeFsReadDir(rows)).toBe(rows)
  })

  it('unwraps { entries } (forward-compat / zip-list-like shape)', () => {
    const rows = [{ name: 'x', path: '/x', isDirectory: false }]
    expect(normalizeFsReadDir({ entries: rows, truncated: false })).toEqual(rows)
  })

  it('throws a clear error for objects / null / undefined (the spread crash)', () => {
    expect(() => normalizeFsReadDir({})).toThrow(/non-array/)
    expect(() => normalizeFsReadDir(undefined)).toThrow(/undefined/)
    expect(() => normalizeFsReadDir(null)).toThrow(/null/)
    // Reproduce the user-facing crash mode: spreading a plain object.
    expect(() => {
      const raw: unknown = { users: [] }
      const entries = normalizeFsReadDir(raw)
      return [...entries]
    }).toThrow(/non-array/)
  })
})
