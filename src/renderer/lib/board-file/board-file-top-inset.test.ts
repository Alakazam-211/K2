// A Board tab has no Garden chrome above its frame, so the board's top bars
// sit 12px under the tab bar, like its sides; the Garden's board keeps 60px.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { BOARD_FILE_WIDGET } from './board-file-frame'

const WIDGETS = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const read = (dir: string, file: string): string => readFileSync(resolve(WIDGETS, dir, file), 'utf8')
// The folder of the version the tab runs (`k2:board-file@2` → board-file-2); released versions never change.
const TAB = BOARD_FILE_WIDGET.replace(/^k2:/, '').replace('@', '-')
const topOf = (css: string, sel: string): string | null => {
  const m = new RegExp(`(?:^|\\n)\\${sel} \\{[^}]*?\\btop: ([^;]+);`).exec(css)
  return m ? m[1].trim() : null
}

describe('Board tab: no space reserved for chrome it does not have', () => {
  it('puts the top bars and the hud 12px from the top of the tab', () => {
    const css = read(TAB, 'board.css')
    for (const sel of ['.topleft', '.topright', '.hud']) expect(topOf(css, sel)).toBe('12px')
  })

  it('fits the board and places popovers with no 60px top band', () => {
    expect(read(TAB, 'app.js')).toMatch(/opts\.top : 12\b/)
    expect(read(TAB, 'ui.js')).not.toMatch(/Math\.max\(60, Math\.min\(H - ph/)
  })

  it('hides its own name and starts its bar after K2’s name only when the tab says so', () => {
    const css = read(TAB, 'board.css')
    expect(css).toContain('.topleft { left: calc(12px + var(--zen-board-name-w, 0px)); }')
    expect(css).toContain('.boardname .logo, .boardname .name { display: var(--zen-board-name-host, flex); }')
  })

  it('runs board-file@2; board-file@1 stays as released', () => {
    expect(TAB).toBe('board-file-2')
    const css = read('board-file-1', 'board.css')
    for (const sel of ['.topleft', '.topright', '.hud']) expect(topOf(css, sel)).toBe('60px')
    expect(css).not.toContain('--zen-board-name-w')
  })

  it('leaves the Garden board (board-1) as it was', () => {
    const css = read('board-1', 'board.css')
    for (const sel of ['.topleft', '.topright', '.hud']) expect(topOf(css, sel)).toBe('60px')
  })
})
