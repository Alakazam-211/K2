// Contrast floors for keyboard focus on the shared Segmented and for the Usage
// page's pressable total tiles, in the four shipped looks (Terminal: charcoal,
// paper; Glass: obsidian, veil). Palette colours come from the style packages
// and are measured with the WCAG formula; CSS rules are read from globals.css
// and the generated styles, comments removed. (Companion to
// control-contrast.test.ts.)
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = strip(readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8'))
const usage = readFileSync(join(repo, 'src/renderer/components/Settings/sections/TokenUsageSection.tsx'), 'utf8')

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

/** The colour a palette slot paints: #rrggbb, var() of another slot or
 *  Glass's accent, and color-mix(in srgb, <colour> N%, black|white). */
function resolve(style: string, id: string, value: string): Rgb {
  const v = value.trim()
  const hex = /^#([0-9a-f]{6})$/i.exec(v)
  if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)) as Rgb
  if (v === 'black') return [0, 0, 0]
  if (v === 'white') return [255, 255, 255]
  if (v === 'var(--glass-accent)') {
    const overrides = readFileSync(join(repo, 'styles', style, 'overrides.css'), 'utf8')
    return resolve(style, id, /--glass-accent:\s*(#[0-9a-f]{6})/i.exec(overrides)![1])
  }
  const ref = /^var\(--color-([a-z-]+)\)$/.exec(v)
  if (ref) return resolve(style, id, palette(style, id)[ref[1]])
  const m = /^color-mix\(in srgb, (.+) (\d+)%, (black|white)\)$/.exec(v)
  if (!m) throw new Error(`can't resolve ${value}`)
  const [a, b, t] = [resolve(style, id, m[1]), resolve(style, id, m[3]), Number(m[2]) / 100]
  return a.map((c, i) => c * t + b[i] * (1 - t)) as Rgb
}

/** --color-accent-text: the accent, darkened 20% on Veil (styles/glass/overrides.css). */
function accentText(style: string, id: string): Rgb {
  const veil = /\[data-palette="veil"\] \{\s*--color-accent-text: (color-mix\([^;]+\));/.exec(generated)
  expect(veil, 'Veil sets --color-accent-text').not.toBeNull()
  return resolve(style, id, id === 'veil' ? veil![1] : 'var(--color-accent)')
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

const over = (top: Rgb, ground: Rgb, alpha: number): Rgb => top.map((c, i) => c * alpha + ground[i] * (1 - alpha)) as Rgb

/** The body of the first rule whose selector is exactly `sel`. */
function rule(css: string, sel: string): string {
  const at = css.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return css.slice(at, css.indexOf('}', at))
}

const GROUNDS = ['bg', 'bg-surface', 'bg-elevated'] as const

describe('Segmented: keyboard focus on the selected segment', () => {
  it('rings outside the fill with 1px of track between, never in white', () => {
    expect(rule(globals, '.k2-segment:focus-visible')).toContain('outline: 2px solid var(--color-accent-text)')
    expect(rule(globals, '.k2-segment[aria-checked="true"]:focus-visible')).toContain('outline-offset: 1px')
    expect(globals).not.toMatch(/\.k2-segment[^{]*:focus-visible[^{]*\{[^}]*on-accent/)
  })

  it('the 1px gap and 2px ring fit inside Glass\'s 3px track padding, so the pill does not clip them', () => {
    expect(rule(generated, '[data-style="glass"] .k2-segmented[data-material="frost"]:not([data-zen-root] *)')).toContain('padding: 3px')
    expect(rule(generated, '[data-style="glass"] .k2-segment-thumb:not([data-zen-root] *)')).toMatch(/top: 3px;\s*bottom: 3px/)
  })

  it.each(LOOKS)('the ring reads at 3:1 on the track it sits on, whatever the fill (%s %s)', (style, id) => {
    const p = palette(style, id)
    const ring = accentText(style, id)
    // The track is clear, so it is the card or page under it.
    for (const ground of GROUNDS) {
      expect(contrast(ring, resolve(style, id, p[ground])), `${id} ring on ${ground}`).toBeGreaterThanOrEqual(3)
    }
    // The fill meets the gap, not the ring, and stays 3:1 against it.
    const fill = resolve(style, id, 'var(--color-accent-fill)')
    expect(contrast(fill, resolve(style, id, p['bg-surface'])), `${id} fill against the gap`).toBeGreaterThanOrEqual(3)
  })
})

describe('Segmented: Terminal\'s focus ring follows the cut', () => {
  const sel = 'html[data-style="terminal"] .k2-segment:focus-visible:not([data-zen-root] *)'

  it('drops the square outline for a 2px cut ring in the readable accent, inside an unselected segment', () => {
    expect(rule(globals, sel)).toContain('outline: none')
    const ring = rule(globals, `${sel}::after`)
    // Parallel to the thumb, which sits 3px inside the track (3 × 0.586).
    expect(ring).toContain('--k2-ch-cut: calc(var(--chamfer-control) - 1.757px)')
    expect(ring).toContain('--k2-ch-fr: 2px')
    expect(ring).toContain('inset: 0')
    expect(ring).toContain('background: var(--color-accent-text)')
    expect(ring).toMatch(/clip-path: polygon\(evenodd, /)
  })

  it('outside the selected segment, 3px out (the track\'s edge and padding), cut like the track', () => {
    const ring = rule(globals, 'html[data-style="terminal"] .k2-segment[aria-checked="true"]:focus-visible:not([data-zen-root] *)::after')
    expect(ring).toContain('--k2-ch-cut: var(--chamfer-control)')
    expect(ring).toContain('inset: -3px')
    // Border 1px + padding 2px, the 3px the ring and its gap fill.
    const track = rule(globals, '.k2-segmented')
    expect(track).toContain('padding: 2px')
    expect(track).toContain('border: 1px solid var(--color-control-edge)')
  })
})

describe('Usage total tiles', () => {
  it('a pressable tile is edged in the control edge, the charted one in the readable accent', () => {
    expect(usage).toContain("style={{ borderColor: on ? 'var(--color-accent-text)' : 'var(--color-control-edge)' }}")
    // Light glass drops the tile's soft drop on a pressable tile, so the page beside the edge stays the page.
    expect(rule(generated, '[data-style="glass"][data-palette="veil"] button[data-material="tile"]:not([data-floating]):not([data-zen-root] *)')).toContain('box-shadow: none')
  })

  it('the small count steps up to secondary text on the charted tile', () => {
    expect(usage).toContain("${on ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}")
    expect(usage).toContain("on ? 'bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)]'")
  })

  it.each(LOOKS)('edges hold 3:1 on the page and the tile, and every line of text 4.5:1 on its ground (%s %s)', (style, id) => {
    const p = palette(style, id)
    const edge = resolve(style, id, p['control-edge'])
    const accent = accentText(style, id)
    const tile = resolve(style, id, p['bg-surface'])
    const wash = over(resolve(style, id, 'var(--color-accent)'), tile, 0.12)
    for (const ground of ['bg', 'bg-surface'] as const) {
      const g = resolve(style, id, p[ground])
      expect(contrast(edge, g), `${id} tile edge on ${ground}`).toBeGreaterThanOrEqual(3)
      expect(contrast(accent, g), `${id} charted edge on ${ground}`).toBeGreaterThanOrEqual(3)
    }
    expect(contrast(accent, wash), `${id} charted edge on its wash`).toBeGreaterThanOrEqual(3)
    expect(contrast(resolve(style, id, p['text-muted']), tile), `${id} count on a tile`).toBeGreaterThanOrEqual(4.5)
    for (const slot of ['text-secondary', 'text-primary']) {
      expect(contrast(resolve(style, id, p[slot]), wash), `${id} ${slot} on the charted wash`).toBeGreaterThanOrEqual(4.5)
    }
  })
})

describe('Settings → Styles list in Glass: the Usage tile, pressable', () => {
  const sel = (tail: string): string => `[data-style="glass"] [data-style-row][data-style-row][data-material]${tail}:not([data-zen-root] *)`

  it('edges the tile in the control edge, lighter on hover, the selected one in the readable accent, and drops Terminal’s bar', () => {
    expect(rule(generated, sel(''))).toContain('border-color: var(--color-control-edge)')
    expect(rule(generated, sel(':hover'))).toContain('border-color: var(--color-text-muted)')
    expect(rule(generated, sel('[aria-current="true"]'))).toContain('border-color: var(--color-accent-text)')
    expect(rule(generated, '[data-style="glass"] [data-style-row] > [data-row-bar]:not([data-zen-root] *)')).toContain('display: none')
    // The selected rule comes after hover, so a hovered selected tile keeps the accent.
    expect(generated.indexOf(sel('[aria-current="true"]'))).toBeGreaterThan(generated.indexOf(sel(':hover')))
  })

  it.each(LOOKS.filter(([s]) => s === 'glass'))('every edge holds 3:1 on the page and a card (%s %s)', (style, id) => {
    const p = palette(style, id)
    for (const ground of ['bg', 'bg-surface'] as const) {
      const g = resolve(style, id, p[ground])
      expect(contrast(resolve(style, id, p['control-edge']), g), `${id} edge on ${ground}`).toBeGreaterThanOrEqual(3)
      expect(contrast(resolve(style, id, p['text-muted']), g), `${id} hover edge on ${ground}`).toBeGreaterThanOrEqual(3)
      expect(contrast(accentText(style, id), g), `${id} selected edge on ${ground}`).toBeGreaterThanOrEqual(3)
    }
  })
})
