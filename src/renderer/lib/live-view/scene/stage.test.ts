// The dark stage in the light looks: the stage re-scopes the look's dark
// palette on its own element, and that palette's tokens (the ones the
// overlays read) are dark in the generated Style CSS. Read from the real
// styles.generated.css, so a palette change that breaks the stage fails here.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { stageScope } from './stage'

const CSS = readFileSync(join(__dirname, '../../../styles.generated.css'), 'utf8')

/** The token values a `[data-style][data-palette]` block sets. */
function block(style: string, palette: string): Record<string, string> {
  const head = `[data-style="${style}"][data-palette="${palette}"] {`
  const out: Record<string, string> = {}
  let at = CSS.indexOf(head)
  expect(at, head).toBeGreaterThanOrEqual(0)
  while (at >= 0) {
    const body = CSS.slice(at + head.length, CSS.indexOf('}', at))
    for (const m of body.matchAll(/(--[\w-]+):\s*([^;]+);/g)) out[m[1]] = m[2].trim()
    at = CSS.indexOf(head, at + 1)
  }
  return out
}

const lum = (hex: string): number => {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  expect(m, hex).not.toBeNull()
  const c = [0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
const ratio = (a: string, b: string): number => {
  const [x, y] = [lum(a), lum(b)]
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)
}

describe('the stage in every look', () => {
  it('takes the look’s dark palette in the light looks', () => {
    expect(stageScope('glass', 'veil', 'light')).toEqual({ 'data-style': 'glass', 'data-palette': 'obsidian', 'data-scheme': 'dark' })
    expect(stageScope('terminal', 'paper', 'light')).toEqual({ 'data-style': 'terminal', 'data-palette': 'charcoal', 'data-scheme': 'dark' })
  })

  it('keeps a dark palette already on the page (Charcoal, Obsidian, Dusk)', () => {
    expect(stageScope('terminal', 'charcoal', 'dark')?.['data-palette']).toBe('charcoal')
    expect(stageScope('glass', 'obsidian', 'dark')?.['data-palette']).toBe('obsidian')
    expect(stageScope('glass', 'yellow', 'dark')?.['data-palette']).toBe('yellow')
  })

  it('leaves an unknown style alone', () => {
    expect(stageScope('nope', 'veil', 'light')).toBeNull()
    expect(stageScope(null, null, null)).toBeNull()
  })

  for (const [style, light] of [['glass', 'veil'], ['terminal', 'paper']] as const) {
    it(`${style} ${light}: every overlay token resolves dark on the stage, and its text reads`, () => {
      const scope = stageScope(style, light, 'light')!
      const t = block(style, scope['data-palette'])
      const lit = block(style, light)
      // The surfaces the stage and its plates use are dark, the light look's are light.
      for (const k of ['--color-bg', '--color-bg-surface', '--color-bg-elevated', '--color-bg-inset']) {
        expect(lum(t[k]), `${k} on the stage`).toBeLessThan(0.05)
        expect(lum(lit[k]), `${k} in ${light}`).toBeGreaterThan(0.5)
      }
      // Text on the stage's plates reads at 4.5:1 or more; edges at 3:1.
      for (const k of ['--color-text-primary', '--color-text-secondary', '--color-text-muted']) expect(ratio(t[k], t['--color-bg-elevated']), k).toBeGreaterThanOrEqual(4.5)
      expect(ratio(t['--color-control-edge'], t['--color-bg-elevated'])).toBeGreaterThanOrEqual(3)
    })
  }

  it('Terminal’s stage is #1a1a1a (Charcoal’s inset), as the mock draws it', () => {
    expect(block('terminal', 'charcoal')['--color-bg-inset']).toBe('#1a1a1a')
  })
})
