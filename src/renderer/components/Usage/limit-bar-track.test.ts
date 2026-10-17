// @vitest-environment jsdom
//
// Usage limit bars (Rosson 2026-10-16: "exceptionally thin"): 6px tall on
// the palette's control track; the track's ends are chamfered in K2's square
// Style only (rounded Styles round by --radius-selector), and Zen's menu
// keeps its own pill track.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chamferCutFor } from '@/components/Thread/chamfer'
import { LIMIT_BAR_HEIGHT_CLASS, LIMIT_BAR_TRACK_CLASS } from './LimitBar'
import { ZEN_USAGE_CSS } from '@/components/Zen/ZenUsageTool'

const dir = dirname(fileURLToPath(import.meta.url))
const globals = readFileSync(join(dir, '../../globals.css'), 'utf8')
const tokens = readFileSync(join(dir, '../../styles.generated.css'), 'utf8')

function rule(css: string, selector: string): string {
  const at = css.indexOf(`${selector} {`)
  if (at < 0) throw new Error(`missing rule ${selector}`)
  return css.slice(at, css.indexOf('}', at))
}

describe('limit bar track', () => {
  it('is 6px (h-1.5) on the control track, not the border colour', () => {
    expect(LIMIT_BAR_HEIGHT_CLASS).toBe('h-1.5')
    expect(LIMIT_BAR_TRACK_CLASS).toBe('bg-[var(--color-control-track-off)]')
  })

  it('every palette defines the control track colour', () => {
    const palettes = tokens.match(/\[data-style="[a-z]+"\]\[data-palette="[a-z]+"\] \{/g) ?? []
    expect(palettes.length).toBeGreaterThanOrEqual(6)
    for (const p of palettes) {
      const body = rule(tokens, p.slice(0, -2))
      expect(body, p).toContain('--color-control-track-off:')
    }
  })

  it('chamfers the track by chamferCutFor(6) only under html[data-style="square"]', () => {
    const cut = `${chamferCutFor(6)}px`
    expect(cut).toBe('1px')
    const r = rule(globals, 'html[data-style="square"] [data-limit-track]')
    expect(r).toContain(`clip-path: polygon(${cut} 0, calc(100% - ${cut}) 0, 100% ${cut}`)
    expect(r).toMatch(/border-radius:\s*0/)
    expect(globals).not.toMatch(/(^|\n)\s*\[data-limit-track\]/)
  })

  it("Zen's usage menu keeps a pill track (no chamfer)", () => {
    const r = rule(ZEN_USAGE_CSS, '[data-testid="subscription-usage-menu"] [data-limit-track]')
    expect(r).toContain('border-radius: 999px')
    expect(r).toContain('clip-path: none')
    expect(ZEN_USAGE_CSS).not.toContain('[role="progressbar"]')
  })
})
