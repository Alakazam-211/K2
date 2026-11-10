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

/** The colour a palette slot paints, for the few forms the shipped palettes
 *  use: #rrggbb, var() of another slot or Glass's accent, and
 *  color-mix(in srgb, <colour> N%, black|white). */
function resolve(style: string, id: string, value: string): Rgb {
  const p = palette(style, id)
  const v = value.trim()
  if (v.startsWith('#')) return hex(v)
  if (v === 'black') return [0, 0, 0]
  if (v === 'white') return [255, 255, 255]
  if (v === 'var(--glass-accent)') {
    const overrides = readFileSync(join(repo, 'styles', style, 'overrides.css'), 'utf8')
    return hex(/--glass-accent:\s*(#[0-9a-f]{6})/i.exec(overrides)![1])
  }
  const ref = /^var\(--color-([a-z-]+)\)$/.exec(v)
  if (ref) return resolve(style, id, p[ref[1]])
  const m = /^color-mix\(in srgb, (.+) (\d+)%, (black|white)\)$/.exec(v)
  if (!m) throw new Error(`can't resolve ${value}`)
  const [a, b, t] = [resolve(style, id, m[1]), resolve(style, id, m[3]), Number(m[2]) / 100]
  return a.map((c, i) => c * t + b[i] * (1 - t)) as Rgb
}

const over = (top: Rgb, ground: Rgb, alpha: number): Rgb => top.map((c, i) => c * alpha + ground[i] * (1 - alpha)) as Rgb

describe('Filled accent controls', () => {
  it.each(LOOKS)('the label reads at 4.5:1 on the accent fill, also at a 90%% hover (%s %s)', (style, id) => {
    const p = palette(style, id)
    const fill = resolve(style, id, 'var(--color-accent-fill)')
    const label = hex(p['on-accent'])
    expect(contrast(label, fill)).toBeGreaterThanOrEqual(4.5)
    for (const ground of ['bg-surface', 'bg-elevated']) {
      const g = hex(p[ground])
      expect(contrast(over(label, g, 0.9), over(fill, g, 0.9)), `${id} hover on ${ground}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it('the Button, the selected segment and Glass\'s segment thumb paint the fill', () => {
    expect(readFileSync(join(__dirname, 'Button.tsx'), 'utf8')).toContain("'bg-[var(--color-accent-fill)] border border-[var(--color-accent-fill)] text-[var(--color-on-accent)]")
    expect(rule(globals, '.k2-segment-thumb')).toContain('background: var(--color-accent-fill)')
    expect(rule(generated, '[data-style="glass"] .k2-segment-thumb:not([data-zen-root] *)')).toContain('var(--color-accent-fill) 92%')
  })
})
