// @vitest-environment jsdom
//
// The DOM painter's per-row canvases (box drawing, blocks) are freed when
// their row leaves. Rows mount and unmount with every line of scrolled
// output; a canvas left to the collector keeps its backing store in WebKit's
// GPU process meanwhile. 100 lines scrolled through a window must leave only
// the rows still on screen holding pixels (test-utils/gpu-ledger).

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { TerminalRow } from './rowRender'
import { installGpuLedger, type GpuLedger } from '@/test-utils/gpu-ledger'

const run = (text: string) => ({
  text,
  fg: null,
  bg: null,
  bold: false,
  italic: false,
  underline: false,
  inverse: false,
  dim: false,
  strikeout: false,
})

function Window({ first, rows }: { first: number; rows: number }) {
  return (
    <div>
      {Array.from({ length: rows }, (_, i) => {
        const abs = first + i
        return (
          <TerminalRow
            key={`abs-${abs}`}
            row={[run(`│ line ${abs} ├────────┤ █▌`)]}
            absRow={abs}
            defaultFg="rgb(224,224,224)"
            defaultBg="rgb(0,0,0)"
            cellWidth={9}
            cellHeight={20}
            dpr={2}
          />
        )
      })}
    </div>
  )
}

let ledger: GpuLedger
beforeEach(() => {
  ledger = installGpuLedger()
})
afterEach(() => {
  cleanup()
  ledger.restore()
})

describe('DOM painter row canvases', () => {
  it('scrolling 100 lines through a 24-row window: only the rows on screen hold pixels', () => {
    const ROWS = 24
    const { rerender, unmount } = render(<Window first={0} rows={ROWS} />)
    const canvases = () => [...document.querySelectorAll<HTMLCanvasElement>('[data-synth-canvas]')]
    expect(canvases()).toHaveLength(ROWS)
    const onScreen = () => canvases().reduce((n, c) => n + c.width * c.height * 4, 0)
    expect(onScreen()).toBeGreaterThan(0)
    for (let i = 1; i <= 100; i++) rerender(<Window first={i} rows={ROWS} />)
    expect(canvases()).toHaveLength(ROWS)
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(onScreen())
    unmount()
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(0)
  })
})
