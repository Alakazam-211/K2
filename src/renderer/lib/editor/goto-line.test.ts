// Go to line (build-editors.md E3): ⌘L or the status line's Ln, Col. It
// takes "12" or "L12", and a column after it ("12:4", "L12:C4", "L12 C4").
// Out of range says so in words. Pure.

import { describe, expect, it } from 'vitest'
import { parseLineTarget } from './goto-line'

describe('go to line', () => {
  it('a plain number, or L and a number', () => {
    expect(parseLineTarget('12', 40)).toEqual({ line: 12, col: null })
    expect(parseLineTarget('L12', 40)).toEqual({ line: 12, col: null })
    expect(parseLineTarget(' l12 ', 40)).toEqual({ line: 12, col: null })
  })

  it('a column after it', () => {
    expect(parseLineTarget('12:4', 40)).toEqual({ line: 12, col: 4 })
    expect(parseLineTarget('L12:C4', 40)).toEqual({ line: 12, col: 4 })
    expect(parseLineTarget('L12 C4', 40)).toEqual({ line: 12, col: 4 })
    expect(parseLineTarget('Ln 12, Col 4', 40)).toEqual({ line: 12, col: 4 })
  })

  it('empty asks for nothing; anything else says what it takes', () => {
    expect(parseLineTarget('', 40)).toBeNull()
    expect(parseLineTarget('twelve', 40)).toEqual({ error: 'Type a line number, like 12 or L12' })
  })

  it('past the end, or line 0, says how many lines there are', () => {
    expect(parseLineTarget('900', 40)).toEqual({ error: 'This file has 40 lines' })
    expect(parseLineTarget('0', 1)).toEqual({ error: 'This file has 1 line' })
  })
})
