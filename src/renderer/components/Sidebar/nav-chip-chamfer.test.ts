// Agent-row line-2 chips (project tags, the Home machine chip) are
// chamfered rectangles in K2's square Style, like the presence chips and the
// Thread (Rosson 2026-10-16); rounded Styles keep their look.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chamferCutFor } from '@/components/Thread/chamfer'

const dir = dirname(fileURLToPath(import.meta.url))
const globals = readFileSync(join(dir, '../../globals.css'), 'utf8')
const tags = readFileSync(join(dir, 'NavProjectTags.tsx'), 'utf8')
const machine = readFileSync(join(dir, '../Home/MachineChip.tsx'), 'utf8')

function rule(selector: string): string {
  const at = globals.indexOf(`${selector} {`)
  if (at < 0) throw new Error(`missing rule ${selector}`)
  return globals.slice(at, globals.indexOf('}', at))
}

describe('chamfered line-2 chips (square Style only)', () => {
  const cut = `${chamferCutFor(14)}px`

  it('the cut is the Thread rule for a 14px control (2–3px)', () => {
    const px = chamferCutFor(14)
    expect(px).toBeGreaterThanOrEqual(2)
    expect(px).toBeLessThanOrEqual(3)
  })

  it('clips the chip to the chamfer and hides its square border, only under html[data-style="square"]', () => {
    const r = rule('html[data-style="square"] [data-nav-chip]')
    expect(r).toContain(`clip-path: polygon(${cut} 0, calc(100% - ${cut}) 0, 100% ${cut}`)
    expect(r).toMatch(/border-color:\s*transparent !important/)
    expect(r).toMatch(/border-radius:\s*0 !important/)
    // No unscoped [data-nav-chip] rule: Bezel and Glass are untouched.
    expect(globals).not.toMatch(/(^|\n)\s*\[data-nav-chip\]/)
  })

  it('draws the 1px edge as an even-odd ring in the chip edge colour (accent for tags)', () => {
    const r = rule('html[data-style="square"] [data-nav-chip]::before')
    expect(r).toContain('clip-path: polygon(evenodd,')
    expect(r).toContain('background: var(--nav-chip-edge, var(--color-accent))')
    // Inner contour: 1px in, cut less (√2 − 1)px.
    expect(r).toContain(`${(chamferCutFor(14) + 1 - 0.414).toFixed(3)}px 1px`)
  })

  it('project tags (and their +N) and the machine chip opt in', () => {
    expect(tags.match(/data-nav-chip="tag"/g)).toHaveLength(2)
    expect(machine).toContain('data-nav-chip="machine"')
    expect(machine).toContain("'--nav-chip-edge': 'var(--color-border-strong)'")
  })
})
