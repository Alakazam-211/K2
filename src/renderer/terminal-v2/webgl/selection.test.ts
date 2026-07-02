import { describe, it, expect } from 'vitest'
import { normalizeSelection, wordRangeAtCol } from './selection'
import type { WireCellRun } from '../gridWire'

function run(text: string, over: Partial<WireCellRun> = {}): WireCellRun {
  return {
    text,
    fg: null,
    bg: null,
    bold: false,
    italic: false,
    underline: false,
    inverse: false,
    dim: false,
    strikeout: false,
    ...over,
  }
}

describe('normalizeSelection', () => {
  it('forward drag passes through', () => {
    expect(
      normalizeSelection({ abs: 1, col: 2 }, { abs: 3, col: 4 }),
    ).toEqual({ startAbs: 1, startCol: 2, endAbs: 3, endCol: 4 })
  })

  it('reversed drag swaps endpoints', () => {
    expect(
      normalizeSelection({ abs: 3, col: 4 }, { abs: 1, col: 2 }),
    ).toEqual({ startAbs: 1, startCol: 2, endAbs: 3, endCol: 4 })
  })

  it('same-row reversed drag swaps by column', () => {
    expect(
      normalizeSelection({ abs: 2, col: 9 }, { abs: 2, col: 3 }),
    ).toEqual({ startAbs: 2, startCol: 3, endAbs: 2, endCol: 9 })
  })

  it('collapsed selection is null', () => {
    expect(normalizeSelection({ abs: 2, col: 3 }, { abs: 2, col: 3 })).toBeNull()
  })
})

describe('wordRangeAtCol', () => {
  const row = [run('ls -la /tmp')]

  it('expands to whitespace boundaries', () => {
    expect(wordRangeAtCol(row, 4)).toEqual({ startCol: 3, endCol: 6 })
    expect(wordRangeAtCol(row, 0)).toEqual({ startCol: 0, endCol: 2 })
    expect(wordRangeAtCol(row, 10)).toEqual({ startCol: 7, endCol: 11 })
  })

  it('clicking whitespace selects nothing', () => {
    expect(wordRangeAtCol(row, 2)).toBeNull()
    expect(wordRangeAtCol(row, 6)).toBeNull()
  })

  it('column past the row content selects nothing', () => {
    expect(wordRangeAtCol(row, 50)).toBeNull()
  })

  it('empty row selects nothing', () => {
    expect(wordRangeAtCol([], 0)).toBeNull()
  })

  it('word boundaries respect wide-char column spans', () => {
    // '日本 x' — 日本 spans cols 0-3, space col 4, x col 5.
    const wide = [run('日本 x', { cols: 6 })]
    expect(wordRangeAtCol(wide, 1)).toEqual({ startCol: 0, endCol: 4 })
    expect(wordRangeAtCol(wide, 3)).toEqual({ startCol: 0, endCol: 4 })
    expect(wordRangeAtCol(wide, 5)).toEqual({ startCol: 5, endCol: 6 })
  })

  it('spans run boundaries (styled mid-word)', () => {
    const styled = [run('fo'), run('o!', { bold: true }), run(' bar')]
    expect(wordRangeAtCol(styled, 1)).toEqual({ startCol: 0, endCol: 4 })
  })
})
