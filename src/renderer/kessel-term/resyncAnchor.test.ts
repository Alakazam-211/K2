// Content seam-match contract (resyncAnchor.ts). The defect it pins:
// a k1 resync snapshot at the daemon's scrollback cap has the SAME
// total row count as the client mirror while the content has shifted
// — the growth heuristic anchors by 0 and the scrolled-up view
// yanks. Seam-matching finds the viewed rows in the new snapshot and
// re-derives scrollPx exactly.

import { describe, expect, it } from 'vitest'
import { computeResyncScrollPx, type AnchorGrid } from './resyncAnchor'

const CELL_H = 20
const VP = 4 // viewport rows

function row(text: string): { text: string }[] {
  return text === '' ? [] : [{ text }]
}

/** Grid with numbered scrollback lines [from..to) + a live grid. */
function gridOf(lines: string[], gridLines: string[] = ['g0', 'g1', 'g2', 'g3']): AnchorGrid {
  return {
    scrollback: lines.map(row),
    grid: gridLines.map(row),
    rows: VP,
  }
}

const LINES = Array.from({ length: 20 }, (_, i) => `line-${i}`)

describe('computeResyncScrollPx', () => {
  // 20 scrollback + 4 grid = 24 total. Scrolled up 5 rows
  // (scrollPx 100): topPx = (24-4)*20 - 100 = 300 → topAbs 15.
  const PREV = gridOf(LINES)
  const SCROLL = 100

  it('identical snapshot → same scrollPx (shift 0)', () => {
    expect(computeResyncScrollPx(PREV, gridOf(LINES), SCROLL, CELL_H)).toBe(
      SCROLL,
    )
  })

  it('cap-shift: 3 trimmed from top, 3 appended, SAME total → +3 rows', () => {
    // The heuristic sees growth 0 here; the seam-match must see the
    // viewed row (line-15) now at index 12 and grow scrollPx by 3ch.
    const shifted = gridOf([...LINES.slice(3), 'new-a', 'new-b', 'new-c'])
    expect(computeResyncScrollPx(PREV, shifted, SCROLL, CELL_H)).toBe(
      SCROLL + 3 * CELL_H,
    )
  })

  it('pure growth: 5 appended → +5 rows (agrees with the heuristic)', () => {
    const grown = gridOf([...LINES, 'n0', 'n1', 'n2', 'n3', 'n4'])
    expect(computeResyncScrollPx(PREV, grown, SCROLL, CELL_H)).toBe(
      SCROLL + 5 * CELL_H,
    )
  })

  it('preserves the sub-row fraction', () => {
    // scrollPx 90 → topPx 310 → topAbs 15, frac 10.
    const shifted = gridOf([...LINES.slice(2), 'x', 'y'])
    expect(computeResyncScrollPx(PREV, shifted, 90, CELL_H)).toBe(
      90 + 2 * CELL_H,
    )
  })

  it('no confident match → null (fallback to heuristic)', () => {
    const unrelated = gridOf(Array.from({ length: 20 }, (_, i) => `other-${i}`))
    expect(computeResyncScrollPx(PREV, unrelated, SCROLL, CELL_H)).toBeNull()
  })

  it('blank-only window → null (never anchor to whitespace)', () => {
    const blanks = gridOf(Array.from({ length: 20 }, () => ''))
    expect(
      computeResyncScrollPx(blanks, gridOf(LINES), SCROLL, CELL_H),
    ).toBeNull()
  })

  it('duplicate content: picks the match nearest the old position', () => {
    // The window (line-15..line-18-ish) duplicated far away at the
    // start; the true (near) occurrence must win.
    const dup = [
      ...LINES.slice(15, 19), // duplicate at index 0..3
      ...LINES.slice(0, 15),
      ...LINES.slice(15), // true occurrence at index 19
    ]
    const next = gridOf(dup)
    // topAbs 15 → nearest occurrence is at 19 (dist 4) vs 0 (dist 15).
    const total = dup.length + VP
    const expected = (total - VP) * CELL_H - 19 * CELL_H
    expect(computeResyncScrollPx(PREV, next, SCROLL, CELL_H)).toBe(expected)
  })

  it('at the bottom (scrollPx 0) → null (no anchoring needed)', () => {
    expect(computeResyncScrollPx(PREV, gridOf(LINES), 0, CELL_H)).toBeNull()
  })
})
