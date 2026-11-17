// A message jumped to (a reply's quote clicked) gets a soft accent wash. The
// words on it, its name and time line among them, must still read at 4.5:1
// (WCAG 1.4.3) in every look: the wash is the accent at JUMP_WASH_PCT over
// the page, and the line reads the Style's secondary text while washed.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { JUMP_WASH_PCT } from './HomeChatMessageRow'

const repo = join(__dirname, '..', '..', '..', '..')

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

type Rgb = [number, number, number]

/** Glass's one accent line (`--glass-accent`), which its palettes point at. */
function glassAccent(): string {
  const css = readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8')
  const m = /--glass-accent:\s*(#[0-9a-f]{6})/i.exec(css)
  if (!m) throw new Error('no --glass-accent in styles.generated.css')
  return m[1]
}

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function slot(p: Record<string, string>, name: string): string {
  const v = p[name]
  if (v === undefined) throw new Error(`no slot ${name}`)
  if (v === 'var(--glass-accent)') return glassAccent()
  const ref = /^var\(--color-([a-z0-9-]+)\)$/.exec(v)
  return ref ? slot(p, ref[1]) : v
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

/** `color-mix(in srgb, top pct%, transparent)` over an opaque page. */
function over(top: Rgb, page: Rgb, pct: number): Rgb {
  const a = pct / 100
  return [0, 1, 2].map((i) => top[i] * a + page[i] * (1 - a)) as Rgb
}

describe("a jumped-to message's wash", () => {
  it.each(LOOKS)('its name and time line reads 4.5:1 on the wash (%s %s)', (style, id) => {
    const p = palette(style, id)
    const wash = over(hex(slot(p, 'accent')), hex(slot(p, 'bg')), JUMP_WASH_PCT)
    expect(contrast(hex(slot(p, 'text-secondary')), wash)).toBeGreaterThanOrEqual(4.5)
  })

  it('is still a wash you can see: at least a tenth of the accent', () => {
    expect(JUMP_WASH_PCT).toBeGreaterThanOrEqual(10)
  })
})
