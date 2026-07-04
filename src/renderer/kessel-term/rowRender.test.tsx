// @vitest-environment jsdom
//
// Column-anchor contract for the DOM row renderer (rowRender.tsx).
//
// The defect these tests pin: rows used to render as naturally
// flowing inline spans, so any glyph the font falls back for —
// grok's braille-art logo, including its invisible U+2800 BRAILLE
// BLANK padding — advanced at the FALLBACK font's width and pushed
// every later run off its column (~2 chars drift in the wild). The
// fix anchors each run at `left = startCol × cellWidth` computed
// from the MODEL (runColOffsets prefix sums), so a neighbor's
// rendered width can never move a run. jsdom does no glyph layout,
// which is exactly the point: the asserted `left`/`width` come from
// the model, not from flow.

import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { TerminalRow, type RenderRun } from './rowRender'

const CELL_W = 9
const CELL_H = 20

function run(text: string, extra: Partial<RenderRun> = {}): RenderRun {
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
    ...extra,
  }
}

function renderRow(row: RenderRun[], cellWidth = CELL_W, cellHeight = CELL_H) {
  const { container } = render(
    <TerminalRow
      row={row}
      absRow={7}
      defaultFg="rgb(224,224,224)"
      defaultBg="rgb(0,0,0)"
      cellWidth={cellWidth}
      cellHeight={cellHeight}
    />,
  )
  const rowDiv = container.querySelector('[data-abs-row]') as HTMLElement
  expect(rowDiv).not.toBeNull()
  return { rowDiv, spans: Array.from(rowDiv.querySelectorAll('span')) }
}

describe('TerminalRow column anchoring', () => {
  it('anchors the run after braille art at its model column, independent of the braille run\'s natural width', () => {
    // 5 braille code points (incl. U+2800 BRAILLE BLANK padding) = 5
    // columns. The daemon omits `cols` (span == char count); the
    // following run must sit at 5 × cellWidth regardless of how any
    // font would advance the braille glyphs.
    const { spans } = renderRow([run('⠋⠙⠀⠀⠈'), run('MENU')])
    expect(spans).toHaveLength(2)
    expect(spans[0].style.left).toBe('0px')
    expect(spans[0].style.width).toBe(`${5 * CELL_W}px`)
    expect(spans[1].style.left).toBe(`${5 * CELL_W}px`)
    expect(spans[1].style.width).toBe(`${4 * CELL_W}px`)
  })

  it('positions every run absolutely with overflow clipped to its cell rect', () => {
    const { spans } = renderRow([run('ab'), run('cd')])
    for (const s of spans) {
      expect(s.style.position).toBe('absolute')
      expect(s.style.overflow).toBe('hidden')
      expect(s.style.whiteSpace).toBe('pre')
    }
  })

  it('advances by the wire cols span after a wide-char run, not the char count', () => {
    // 日本 = 2 chars, 4 columns → the next run starts at column 4.
    const { spans } = renderRow([run('日本', { cols: 4 }), run('!')])
    expect(spans[0].style.width).toBe(`${4 * CELL_W}px`)
    expect(spans[1].style.left).toBe(`${4 * CELL_W}px`)
  })

  it('accumulates offsets across mixed annotated/unannotated runs', () => {
    // ab | 日本(4) | cd → starts 0, 2, 6.
    const { spans } = renderRow([
      run('ab'),
      run('日本', { cols: 4 }),
      run('cd'),
    ])
    expect(spans.map((s) => s.style.left)).toEqual([
      '0px',
      `${2 * CELL_W}px`,
      `${6 * CELL_W}px`,
    ])
  })

  it('fixes the row box at one cell height with relative positioning', () => {
    const { rowDiv } = renderRow([run('x')])
    expect(rowDiv.style.position).toBe('relative')
    expect(rowDiv.style.height).toBe(`${CELL_H}px`)
    expect(rowDiv.dataset.absRow).toBe('7')
  })

  it('renders the nbsp placeholder for an empty row and keeps the fixed height', () => {
    const { rowDiv, spans } = renderRow([])
    expect(spans).toHaveLength(0)
    expect(rowDiv.textContent).toBe('\u00a0')
    expect(rowDiv.style.height).toBe(`${CELL_H}px`)
  })

  it('does not clip a zero-span run (isolated combining char rides its base cell)', () => {
    const { spans } = renderRow([run('a'), run('́', { cols: 0 })])
    expect(spans[1].style.left).toBe(`${1 * CELL_W}px`)
    expect(spans[1].style.width).toBe('')
    expect(spans[1].style.overflow).toBe('')
  })

  it('falls back to natural flow before cell metrics are measured', () => {
    const { rowDiv, spans } = renderRow([run('ab'), run('日本', { cols: 4 })], 0, 0)
    expect(rowDiv.style.position).toBe('')
    expect(spans[0].style.position).toBe('')
    // Legacy ch-width pinning still applies to annotated runs.
    expect(spans[1].style.width).toBe('4ch')
    expect(spans[1].style.display).toBe('inline-block')
  })
})
