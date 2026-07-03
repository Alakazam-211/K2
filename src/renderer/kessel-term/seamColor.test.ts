import { describe, expect, it } from 'vitest'

import { pickSeamColor, type SeamRun } from './seamColor'

const DARK = 0x1e1e2e // a TUI's own background (Grok/Claude style)
const BLUE = 0x0000aa
const RED = 0xaa0000

function run(text: string, bg: number | null, extra?: Partial<SeamRun>): SeamRun {
  return { text, fg: null, bg, inverse: false, ...extra }
}

/** rows×cols grid where every cell carries one explicit bg. */
function tuiGrid(rows: number, cols: number, bg: number): SeamRun[][] {
  return Array.from({ length: rows }, () => [run('x'.repeat(cols), bg)])
}

describe('pickSeamColor — edge cases', () => {
  it('empty rows array → null', () => {
    expect(pickSeamColor([])).toBeNull()
  })

  it('rows of empty runs (blank grid) → null', () => {
    expect(pickSeamColor([[], [], []])).toBeNull()
    expect(pickSeamColor([[], [], []], 80)).toBeNull()
  })

  it('single row with uniform explicit bg → that color', () => {
    expect(pickSeamColor([[run('hello', DARK)]])).toBe(DARK)
  })

  it('single row with default bg → null', () => {
    expect(pickSeamColor([[run('hello', null)]])).toBeNull()
  })
})

describe('pickSeamColor — plain shell (default backgrounds)', () => {
  it('all-default rows → null (theme background unchanged)', () => {
    const rows = Array.from({ length: 24 }, () => [run('$ ls -la', null)])
    expect(pickSeamColor(rows, 80)).toBeNull()
  })

  it('shell with a colored prompt segment but default edges → null', () => {
    const rows: SeamRun[][] = [
      [run('branch', BLUE), run(' main $ ', null)],
      ...Array.from({ length: 10 }, () => [run('output text', null)]),
    ]
    expect(pickSeamColor(rows, 80)).toBeNull()
  })
})

describe('pickSeamColor — fullscreen TUI (explicit backgrounds)', () => {
  it('uniform explicit bg over the whole grid → that color', () => {
    expect(pickSeamColor(tuiGrid(24, 80, DARK), 80)).toBe(DARK)
  })

  it('works without cols hint too', () => {
    expect(pickSeamColor(tuiGrid(24, 80, DARK))).toBe(DARK)
  })

  it('TUI body bg wins over a differently-colored status bar', () => {
    // 23 body rows (DARK) + a 1-row BLUE status bar at the bottom:
    // boundary = 80 BLUE bottom cells + 23 DARK right-edge cells…
    // BLUE has the strict majority here, and that IS the color that
    // touches the bottom seam.
    const rows = [...tuiGrid(23, 80, DARK), [run('x'.repeat(80), BLUE)]]
    expect(pickSeamColor(rows, 80)).toBe(BLUE)
  })

  it('multi-run boundary rows weight by run col-span', () => {
    // Bottom row: 60 cols DARK + 20 cols BLUE → DARK majority.
    const rows = [
      ...tuiGrid(5, 80, DARK),
      [run('x'.repeat(60), DARK), run('y'.repeat(20), BLUE)],
    ]
    expect(pickSeamColor(rows, 80)).toBe(DARK)
  })

  it('explicit cols field (wide chars) drives the weighting', () => {
    // "日"×10 = 10 chars spanning 20 columns of BLUE at the end of
    // the bottom row; 60 cols of DARK before it → DARK majority.
    const rows = [
      ...tuiGrid(5, 80, DARK),
      [run('x'.repeat(60), DARK), run('日'.repeat(10), BLUE, { cols: 20 })],
    ]
    expect(pickSeamColor(rows, 80)).toBe(DARK)
  })

  it('inverse run paints its fg as background', () => {
    const inverseRow: SeamRun[] = [
      { text: 'x'.repeat(80), fg: DARK, bg: null, inverse: true },
    ]
    const rows = [...tuiGrid(23, 80, DARK), inverseRow]
    expect(pickSeamColor(rows, 80)).toBe(DARK)
  })
})

describe('pickSeamColor — no majority', () => {
  it('50/50 split (two colors) → null (strict majority required)', () => {
    // Bottom row half RED half BLUE; right edges alternate evenly.
    const rows = [
      [run('x'.repeat(80), RED)],
      [run('x'.repeat(80), BLUE)],
      [run('x'.repeat(40), RED), run('y'.repeat(40), BLUE)],
    ]
    // Boundary: 40 RED + 40 BLUE (bottom) + 1 RED + 1 BLUE (right
    // edges) = 41 vs 41 → no strict majority.
    expect(pickSeamColor(rows, 80)).toBeNull()
  })

  it('explicit color at exactly half against defaults → null', () => {
    const rows = [[run('x'.repeat(40), DARK), run('y'.repeat(40), null)]]
    expect(pickSeamColor(rows, 80)).toBeNull()
  })

  it('mixed many-color boundary with no >50% winner → null', () => {
    const rows = [
      [
        run('a'.repeat(30), RED),
        run('b'.repeat(30), BLUE),
        run('c'.repeat(20), DARK),
      ],
    ]
    expect(pickSeamColor(rows, 80)).toBeNull()
  })
})

describe('pickSeamColor — trimmed rows (cols hint)', () => {
  it('rows trimmed short of cols count default right edges', () => {
    // Every row paints 40 DARK cols then is trimmed: right edge and
    // the bottom remainder are default → DARK is 40 of 80 bottom
    // cells and 0 of the right-edge cells → no majority.
    const rows = Array.from({ length: 10 }, () => [run('x'.repeat(40), DARK)])
    expect(pickSeamColor(rows, 80)).toBeNull()
  })

  it('without the cols hint the last run is assumed to reach the edge', () => {
    const rows = Array.from({ length: 10 }, () => [run('x'.repeat(40), DARK)])
    expect(pickSeamColor(rows)).toBe(DARK)
  })
})
