// The list-row hover band (--color-row-hover) in the four shipped looks:
// visibly different from the resting row (the card's fill), and the row's
// text still reads on it. Palette colours are read from the style packages
// and measured with the WCAG formula.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { SETTING_LIST_ROW_BAND, SETTING_LIST_ROW_HOVER } from '../Settings/controls/SettingControls'

const repo = join(__dirname, '..', '..', '..', '..')

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

/** An rgba() wash painted over a solid ground. */
function wash(value: string, ground: Rgb): Rgb {
  const m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(value)
  if (!m) throw new Error(`not an rgba() wash: ${value}`)
  const a = Number(m[4])
  return [1, 2, 3].map((i, k) => Number(m[i]) * a + ground[k] * (1 - a)) as Rgb
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

// [style, palette, the band's floor against the resting row]
const LOOKS = [
  ['terminal', 'charcoal', 1.3],
  ['terminal', 'paper', 1.2],
  ['glass', 'obsidian', 1.3],
  ['glass', 'veil', 1.2],
] as const

describe('List-row hover band', () => {
  it.each(LOOKS)('%s %s: the band stands out from the resting row and the row text reads on it', (style, id, floor) => {
    const p = palette(style, id)
    // Settings rows rest on the card, which is bg-surface in both styles.
    const row = hex(p['bg-surface'])
    const band = wash(p['row-hover'], row)
    expect(contrast(band, row), `${id} band against the resting row`).toBeGreaterThanOrEqual(floor)
    // On hover the row's muted text steps up to secondary (SETTING_LIST_ROW_HOVER).
    for (const slot of ['text-primary', 'text-secondary']) {
      expect(contrast(hex(p[slot]), band), `${id} ${slot} on the band`).toBeGreaterThanOrEqual(4.5)
    }
    // The resting muted text is still at 4.5:1 off the band.
    expect(contrast(hex(p['text-muted']), row), `${id} muted text at rest`).toBeGreaterThanOrEqual(4.5)
  })

  it('every style that ships a palette defines the band (the build requires it)', () => {
    expect(readFileSync(join(repo, 'scripts', 'build-styles.mjs'), 'utf8')).toMatch(/'wash-1', 'wash-2', 'wash-3', 'scrim', 'danger-muted', 'row-hover'/)
  })

  it('the shared row classes paint the band and step muted text up on the row\'s own hover', () => {
    // The row's own :hover holds while a control inside the row is hovered,
    // so hovering Remove or a menu button keeps the text bright too.
    expect(SETTING_LIST_ROW_HOVER).toContain('hover:bg-[var(--color-row-hover)]')
    expect(SETTING_LIST_ROW_HOVER).toContain('hover:[--color-text-muted:var(--color-text-secondary)]')
    expect(SETTING_LIST_ROW_BAND).toContain('bg-[var(--color-row-hover)]')
    expect(SETTING_LIST_ROW_BAND).toContain('[--color-text-muted:var(--color-text-secondary)]')
  })
})
