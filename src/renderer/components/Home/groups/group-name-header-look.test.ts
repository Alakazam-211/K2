// The Home nav's top row (GroupNameHeader): "…" and « share one small hover
// shape round the glyph (a circle in Glass, a chamfered square in Terminal;
// Rosson 2026-11-14), and the count is a tag right after the name that reads
// at 4.5:1 in the four shipped looks. CSS rules are read from globals.css,
// palette colours from the style packages (WCAG formula).
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { DRAWER_HEADER_BUTTON_CLASS, GLYPH_HEADER_BUTTON_CLASS } from '@/components/Layout/DrawerTabBarToggle'
import { chamferCutFor } from '@/components/Thread/chamfer'
import { GROUP_COUNT_TAG_CLASS } from './GroupNameHeader'

const repo = join(__dirname, '..', '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const header = readFileSync(join(__dirname, 'GroupNameHeader.tsx'), 'utf8')
const toggle = readFileSync(join(repo, 'src/renderer/components/Layout/DrawerTabBarToggle.tsx'), 'utf8')

const NOT_ZEN = ':not([data-zen-root] *, [data-k2-thread-zen] *)'
const GLYPH = '[data-k2-glyph-button]'

/** The body of the first rule whose whole selector is exactly `sel`. */
function rule(sel: string): string {
  const at = globals.indexOf(`}\n${sel} {`) + 2
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(2)
  return globals.slice(at, globals.indexOf('}', at))
}

describe('the top row\'s two buttons: one small-shape variant', () => {
  it('"…" and « both take shape="glyph", and nothing else does', () => {
    const uses = header.match(/<DrawerHeaderButton\b[^>]*>/g) ?? []
    expect(uses).toHaveLength(2)
    for (const u of uses) expect(u).toContain('shape="glyph"')
    expect(uses.some((u) => u.includes('data-group-edit-open'))).toBe(true)
    expect(uses.some((u) => u.includes('data-nav-collapse'))).toBe(true)
  })

  it('the variant marks the button and drops the box fill and box ring; the drawers keep theirs', () => {
    expect(toggle).toContain("data-k2-glyph-button={shape === 'glyph' ? '' : undefined}")
    expect(toggle).toContain("shape = 'strip'")
    expect(GLYPH_HEADER_BUTTON_CLASS).not.toMatch(/hover:bg-|focus-visible:ring/)
    // Same box (full strip height, 32px wide), so the hit target is unchanged.
    for (const c of ['h-full', 'w-8', 'self-stretch', 'flex-shrink-0', 'relative', 'isolate']) {
      expect(GLYPH_HEADER_BUTTON_CLASS.split(/\s+/)).toContain(c)
    }
    expect(DRAWER_HEADER_BUTTON_CLASS).toContain('hover:bg-[var(--color-bg-elevated)]')
    expect(DRAWER_HEADER_BUTTON_CLASS).toContain('focus-visible:ring-1')
  })
})

describe('the glyph shape in CSS', () => {
  const base = `${GLYPH}${NOT_ZEN}`

  it('one shared size token, centred on the button (so both buttons match exactly)', () => {
    expect(globals).toMatch(/--k2-glyph-hover-size:\s*calc\(var\(--spacing\) \* 7\);/)
    const r = rule(`${base}::before,\n${base}::after`)
    expect(r).toContain('width: var(--k2-glyph-hover-size)')
    expect(r).toContain('height: var(--k2-glyph-hover-size)')
    expect(r).toContain('top: 50%')
    expect(r).toContain('left: 50%')
    expect(r).toContain('margin: calc(var(--k2-glyph-hover-size) / -2) 0 0 calc(var(--k2-glyph-hover-size) / -2)')
    expect(r).toContain('pointer-events: none')
    // Glass (any rounded Style): a circle.
    expect(r).toContain('border-radius: var(--radius-pill)')
  })

  it('hover fills the shape under the glyph; focus rings the same shape', () => {
    expect(rule(`${base}::before`)).toContain('z-index: -1')
    expect(rule(`${GLYPH}:hover${NOT_ZEN}::before`)).toContain('background: var(--color-row-hover)')
    expect(rule(`${base}::after`)).toContain('z-index: 1')
    expect(rule(`${GLYPH}:focus-visible${NOT_ZEN}::after`)).toContain('box-shadow: inset 0 0 0 2px var(--color-accent-text)')
  })

  it('Terminal: a square with the control\'s chamfer for hover and the focus ring', () => {
    expect(chamferCutFor(28)).toBe(5)
    const hover = rule(`html[data-style="terminal"] ${base}::before`)
    expect(hover).toContain('--k2-ch-cut: var(--chamfer-control)')
    expect(hover).toContain('border-radius: 0')
    expect(hover).toContain('clip-path: polygon(var(--k2-ch-cut) 0, calc(100% - var(--k2-ch-cut)) 0, 100% var(--k2-ch-cut)')
    const ring = rule(`html[data-style="terminal"] ${GLYPH}:focus-visible${NOT_ZEN}::after`)
    expect(ring).toContain('--k2-ch-cut: var(--chamfer-control)')
    expect(ring).toContain('--k2-ch-fr: 2px')
    expect(ring).toContain('box-shadow: none')
    expect(ring).toContain('background: var(--color-accent-text)')
    expect(ring).toContain('clip-path: polygon(evenodd,')
  })
})

type Rgb = [number, number, number]
const LOOKS = [['terminal', 'charcoal'], ['terminal', 'paper'], ['glass', 'obsidian'], ['glass', 'veil']] as const

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function rgba(v: string): [Rgb, number] {
  const hex = /^#([0-9a-f]{6})$/i.exec(v.trim())
  if (hex) return [[0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)) as Rgb, 1]
  const m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(v.trim())
  if (!m) throw new Error(`can't read ${v}`)
  return [[Number(m[1]), Number(m[2]), Number(m[3])], Number(m[4])]
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

describe('the count tag', () => {
  it('a quiet wash and secondary text on the tokens, cut in Terminal, never shrinking', () => {
    const c = GROUP_COUNT_TAG_CLASS.split(/\s+/)
    expect(c).toContain('flex-shrink-0')
    expect(c).toContain('bg-[var(--color-wash-2)]')
    expect(c).toContain('text-[var(--color-text-secondary)]')
    expect(c).toContain('rounded-[var(--radius-pill)]')
    expect(c).toContain('border-transparent')
    expect(c).toContain('tabular-nums')
    expect(GROUP_COUNT_TAG_CLASS).not.toMatch(/#[0-9a-f]{3,6}\b|white\/|black\//i)
    expect(header).toMatch(/<span className=\{GROUP_COUNT_TAG_CLASS\} data-k2-chamfer="chip"/)
  })

  it.each(LOOKS)('%s / %s: its text reads at 4.5:1 on the wash, on every nav ground', (style, id) => {
    const p = palette(style, id)
    const [ink] = rgba(p['text-secondary'])
    const [wash, a] = rgba(p['wash-2'])
    for (const g of ['bg', 'bg-surface', 'bg-elevated', 'bg-canvas']) {
      if (!p[g] || !p[g].startsWith('#')) continue
      const [ground] = rgba(p[g])
      const under = wash.map((v, i) => v * a + ground[i] * (1 - a)) as Rgb
      expect(contrast(ink, under), `${g}`).toBeGreaterThanOrEqual(4.5)
    }
  })
})
