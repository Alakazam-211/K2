// A Board tab has no Garden chrome above its frame, so the board's top bars
// sit 12px under the tab bar, like its sides; the Garden's board keeps 60px.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const WIDGETS = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const read = (dir: string, file: string): string => readFileSync(resolve(WIDGETS, dir, file), 'utf8')
const topOf = (css: string, sel: string): string | null => {
  const m = new RegExp(`(?:^|\\n)\\${sel} \\{[^}]*?\\btop: ([^;]+);`).exec(css)
  return m ? m[1].trim() : null
}

describe('Board tab: no space reserved for chrome it does not have', () => {
  it('puts the top bars and the hud 12px from the top of the tab', () => {
    const css = read('board-file-1', 'board.css')
    for (const sel of ['.topleft', '.topright', '.hud']) expect(topOf(css, sel)).toBe('12px')
  })

  it('fits the board and places popovers with no 60px top band', () => {
    expect(read('board-file-1', 'app.js')).toMatch(/opts\.top : 12\b/)
    expect(read('board-file-1', 'ui.js')).not.toMatch(/Math\.max\(60, Math\.min\(H - ph/)
  })

  it('leaves the Garden board (board-1) as it was', () => {
    const css = read('board-1', 'board.css')
    for (const sel of ['.topleft', '.topright', '.hud']) expect(topOf(css, sel)).toBe('60px')
  })
})
