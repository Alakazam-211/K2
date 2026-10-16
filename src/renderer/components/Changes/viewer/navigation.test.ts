import { describe, expect, it } from 'vitest'
import { buildStops, currentChangeNumber, stepChange } from './navigation'
import { parseDiffPrefs, DEFAULT_DIFF_PREFS } from './prefs'

describe('buildStops', () => {
  it('flattens files in order, sorts chunk starts, skips files with nothing to land on', () => {
    expect(
      buildStops([{ state: 'chunks', starts: [40, 10] }, { state: 'none' }, { state: 'pending' }, { state: 'chunks', starts: [0] }]),
    ).toEqual([
      { file: 0, pos: 10 },
      { file: 0, pos: 40 },
      { file: 2, pos: 0, pending: true },
      { file: 3, pos: 0 },
    ])
  })
})

describe('stepChange', () => {
  const stops = buildStops([{ state: 'chunks', starts: [10, 40] }, { state: 'chunks', starts: [5] }])

  it('starts at the first (next) or last (previous) stop with no cursor', () => {
    expect(stepChange(stops, null, 1)).toBe(0)
    expect(stepChange(stops, null, -1)).toBe(2)
  })

  it('moves within a file and on into the next file', () => {
    expect(stepChange(stops, { file: 0, pos: 10 }, 1)).toBe(1)
    expect(stepChange(stops, { file: 0, pos: 40 }, 1)).toBe(2)
    expect(stepChange(stops, { file: 0, pos: 20 }, 1)).toBe(1)
  })

  it('moves back across files', () => {
    expect(stepChange(stops, { file: 1, pos: 5 }, -1)).toBe(1)
    expect(stepChange(stops, { file: 0, pos: 40 }, -1)).toBe(0)
  })

  it('wraps at both ends', () => {
    expect(stepChange(stops, { file: 1, pos: 5 }, 1)).toBe(0)
    expect(stepChange(stops, { file: 0, pos: 10 }, -1)).toBe(2)
  })

  it('a cursor at -1 (file revealed, not loaded) lands on that file first', () => {
    expect(stepChange(stops, { file: 1, pos: -1 }, 1)).toBe(2)
  })

  it('returns -1 with no stops', () => {
    expect(stepChange([], null, 1)).toBe(-1)
    expect(stepChange([], { file: 0, pos: 0 }, -1)).toBe(-1)
  })

  it('one chunk: next and previous both land on it', () => {
    const one = buildStops([{ state: 'chunks', starts: [7] }])
    expect(stepChange(one, { file: 0, pos: 7 }, 1)).toBe(0)
    expect(stepChange(one, { file: 0, pos: 7 }, -1)).toBe(0)
  })
})

describe('currentChangeNumber', () => {
  const stops = buildStops([{ state: 'chunks', starts: [10, 40] }, { state: 'chunks', starts: [5] }])
  it('counts the last stop at or before the cursor', () => {
    expect(currentChangeNumber(stops, null)).toBe(0)
    expect(currentChangeNumber(stops, { file: 0, pos: 0 })).toBe(0)
    expect(currentChangeNumber(stops, { file: 0, pos: 10 })).toBe(1)
    expect(currentChangeNumber(stops, { file: 0, pos: 39 })).toBe(1)
    expect(currentChangeNumber(stops, { file: 1, pos: 5 })).toBe(3)
  })
})

describe('parseDiffPrefs', () => {
  it('falls back to defaults on junk and keeps valid fields', () => {
    expect(parseDiffPrefs(null)).toEqual(DEFAULT_DIFF_PREFS)
    expect(parseDiffPrefs('{nope')).toEqual(DEFAULT_DIFF_PREFS)
    expect(parseDiffPrefs('"str"')).toEqual(DEFAULT_DIFF_PREFS)
    expect(parseDiffPrefs('{"wrap":true,"collapse":"x"}')).toEqual({ wrap: true, collapse: true })
    expect(parseDiffPrefs('{"wrap":1,"collapse":false}')).toEqual({ wrap: false, collapse: false })
  })
})
