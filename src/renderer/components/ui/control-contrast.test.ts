// Contrast floors for the shared controls, in the four shipped looks
// (Terminal: charcoal, paper; Glass: obsidian, veil). Palette colours are
// read from the style packages and measured with the WCAG formula; the CSS
// rules are read from globals.css and the generated styles, comments removed.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = strip(readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8'))

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

type Rgb = [number, number, number]

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function hex(value: string): Rgb {
  const m = /^#([0-9a-f]{6})$/i.exec(value)
  if (!m) throw new Error(`not a #rrggbb colour: ${value}`)
  return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) as Rgb
}

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number): number => {
    const s = v / 255
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** The body of the first rule whose selector is exactly `sel`. */
function rule(css: string, sel: string): string {
  const at = css.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return css.slice(at, css.indexOf('}', at))
}

describe('Slider', () => {
  it('draws the unfilled track in the control edge', () => {
    expect(rule(globals, '.k2-slider::-webkit-slider-runnable-track')).toMatch(
      /linear-gradient\(to right, var\(--color-accent\) var\(--k2-slider-pct, 0%\), var\(--color-control-edge\) /,
    )
  })

  it.each(LOOKS)('the control edge holds 3:1 on a card and a raised control (%s %s)', (style, id) => {
    const p = palette(style, id)
    for (const ground of ['bg-surface', 'bg-elevated', 'bg']) {
      expect(contrast(hex(p['control-edge']), hex(p[ground])), `${id} control-edge on ${ground}`).toBeGreaterThanOrEqual(3)
    }
  })

  it('a white Glass thumb on a white Veil card carries the control edge', () => {
    const body = rule(generated, '[data-style="glass"][data-palette="veil"] .k2-slider:not([data-zen-root] *)::-webkit-slider-thumb')
    expect(body).toContain('0 0 0 1px var(--color-control-edge)')
  })
})
