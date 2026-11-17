// @vitest-environment jsdom
// Contrast floors across every palette the two Styles ship (Terminal:
// charcoal, paper, yellow; Glass: obsidian, veil, yellow/Dusk):
//  1. Glass focus rings: 3:1 on the page, a card and the control's fill.
//  2. Status colours as text read through their text tokens, 4.5:1 on every
//     surface; fills and dots keep the status colour.
//  3. Danger buttons: the label (--color-on-danger) reads 4.5:1 on each fill.
//  4. Accent text holds 4.5:1 on the hover fills; the control edge 3:1.
//  5. The off switch in Veil: its knob carries the control edge.
//  6. The Tickets count: ink on the needs-you fill at 4.5:1.
//  7. Yellow and Dusk: secondary text clears 4.5:1 on hover and raised
//     surfaces and stays clearly above muted.
// Palette colours come from the style packages, per-palette overrides from
// styles.generated.css; contrast is the WCAG formula.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, sep } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { Button } from './Button'

afterEach(cleanup)

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = strip(readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8'))

type Rgb = [number, number, number]
type Rgba = [number, number, number, number]

const SHIPPED = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['terminal', 'yellow'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
  ['glass', 'yellow'],
] as const
const GLASS = SHIPPED.filter(([s]) => s === 'glass')
const SURFACES = ['bg', 'bg-surface', 'bg-elevated', 'bg-inset', 'bg-stripe', 'bg-canvas'] as const

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function oklch(l: number, c: number, hDeg: number): Rgb {
  const h = (hDeg * Math.PI) / 180
  const a = c * Math.cos(h)
  const b = c * Math.sin(h)
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3
  const lin = [
    4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
    -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
  ]
  return lin.map((v) => {
    const x = Math.min(1, Math.max(0, v))
    return 255 * (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055)
  }) as Rgb
}

const mixRgb = (a: Rgb, b: Rgb, t: number): Rgb => a.map((c, i) => c * t + b[i] * (1 - t)) as Rgb

/** A palette-level override of a slot, from the generated CSS (a Style's
 *  overrides.css), e.g. the accent as text per palette. */
function override(style: string, id: string, slot: string): string | null {
  const m = new RegExp(`\\[data-style="${style}"\\]\\[data-palette="${id}"\\] \\{\\s*--color-${slot}: ([^;]+);`).exec(generated)
  return m ? m[1] : null
}

const DEFAULTS: Record<string, string> = {
  'accent-fill': 'color-mix(in srgb, var(--color-accent) 82%, black)',
  'accent-text': 'var(--color-accent)',
}

function colour(style: string, id: string, value: string): Rgb | Rgba {
  const v = value.trim()
  let m: RegExpExecArray | null
  if ((m = /^#([0-9a-f]{6})$/i.exec(v))) return [0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16)) as Rgb
  if (v === 'black') return [0, 0, 0]
  if (v === 'white') return [255, 255, 255]
  if (v === 'var(--glass-accent)') {
    const overrides = readFileSync(join(repo, 'styles', style, 'overrides.css'), 'utf8')
    return colour(style, id, /--glass-accent:\s*(#[0-9a-f]{6})/i.exec(overrides)![1])
  }
  if ((m = /^var\(--color-([a-z0-9-]+)\)$/.exec(v))) return slot(style, id, m[1])
  if ((m = /^oklch\(([\d.]+)%\s+([\d.]+)\s+([\d.]+)\)$/.exec(v))) return oklch(Number(m[1]) / 100, Number(m[2]), Number(m[3]))
  if ((m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(v))) return [Number(m[1]), Number(m[2]), Number(m[3]), Number(m[4])]
  if ((m = /^color-mix\(in srgb, (.+) (\d+)%, (.+)\)$/.exec(v))) {
    return mixRgb(solid(colour(style, id, m[1])), solid(colour(style, id, m[3])), Number(m[2]) / 100)
  }
  throw new Error(`can't resolve ${value}`)
}
function solid(c: Rgb | Rgba): Rgb {
  if (c.length === 4) throw new Error('a translucent colour where a solid one is needed')
  return c
}
/** A slot's colour; translucent slots come back with their alpha. */
function slot(style: string, id: string, name: string): Rgb | Rgba {
  const value = override(style, id, name) ?? palette(style, id)[name] ?? DEFAULTS[name]
  expect(value, `${style}/${id} defines ${name}`).toBeDefined()
  return colour(style, id, value)
}
/** A slot painted on a ground (a translucent slot composited over it). */
function on(style: string, id: string, name: string, ground: Rgb): Rgb {
  const c = slot(style, id, name)
  return c.length === 4 ? mixRgb([c[0], c[1], c[2]], ground, c[3]) : c
}
const solidSlot = (style: string, id: string, name: string): Rgb => solid(slot(style, id, name))

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

/** The fills a hovered row, button or list item paints. */
function hoverFills(style: string, id: string): [string, Rgb][] {
  const out: [string, Rgb][] = []
  for (const ground of ['bg', 'bg-surface'] as const) {
    const g = solidSlot(style, id, ground)
    for (const fill of ['row-hover', 'wash-1', 'bg-hover']) out.push([`${fill} on ${ground}`, on(style, id, fill, g)])
  }
  const card = solidSlot(style, id, 'bg-surface')
  for (const fill of ['control-hover', 'control-hover-quiet']) out.push([fill, on(style, id, fill, card)])
  return out
}

/** Every .ts/.tsx source under the renderer's components and lib, Gardens
 *  (components/Zen) and tests left out. */
function sources(): [string, string][] {
  const out: [string, string][] = []
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      if (statSync(p).isDirectory()) {
        if (name !== 'Zen') walk(p)
      } else if (/\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name)) {
        // `/`-separated on every OS: the ICONS list names files that way.
        out.push([p.slice(repo.length + 1).split(sep).join('/'), readFileSync(p, 'utf8')])
      }
    }
  }
  walk(join(repo, 'src/renderer/components'))
  walk(join(repo, 'src/renderer/lib'))
  return out
}

// ── 1 ───────────────────────────────────────────────────────────────────

describe('1. Glass focus rings', () => {
  it('--ring-focus is 2px of the accent as text, not a 40% wash', () => {
    const glass = /\[data-style="glass"\] \{[^}]*--ring-focus: ([^;]+);/.exec(generated)
    expect(glass?.[1]).toBe('0 0 0 2px var(--color-accent-text)')
  })

  it('no focus ring outside Terminal paints the raw accent', () => {
    for (const css of [globals, generated]) {
      for (const m of css.matchAll(/([^{}]*:focus(?:-visible|-within)?[^{}]*)\{([^}]*)\}/g)) {
        if (m[1].includes('data-style="terminal"')) continue
        expect(m[2], `${m[1].trim()}`).not.toMatch(/outline(?:-color)?:[^;]*var\(--color-accent\)/)
      }
    }
    for (const [file, text] of sources()) {
      expect(text, file).not.toMatch(/focus(?:-visible)?:(?:outline|ring)-\[var\(--color-accent\)\]/)
    }
  })

  it('Terminal draws the same shadow ring (it was off, so a focused scroll area showed nothing)', () => {
    const terminal = /\[data-style="terminal"\] \{[^}]*--ring-focus: ([^;]+);/.exec(generated)
    expect(terminal?.[1]).toBe('0 0 0 2px var(--color-accent-text)')
  })

  it('a page tab, in the top bar or a tile, rings inside itself; a filled Glass tab in its label colour', () => {
    expect(globals).toContain('[data-page-tabs] > [data-page-tab]:focus-visible:not([data-zen-root] *) { outline: 2px solid var(--color-accent-text); outline-offset: -2px; }')
    expect(generated).toContain('[data-style="glass"]:not([data-palette="veil"]) [data-page-tabs] > [data-page-tab][data-selected]:focus-visible:not([data-zen-root] *) { outline-color: var(--color-on-accent); }')
    for (const [style, id] of GLASS.filter(([, p]) => p !== 'veil')) {
      // The filled tab is the accent; its label colour rings it at 3:1.
      expect(contrast(solidSlot(style, id, 'on-accent'), solidSlot(style, id, 'accent')), `${id} ring on the filled tab`).toBeGreaterThanOrEqual(3)
    }
  })

  it.each(GLASS)('%s %s: the ring reads 3:1 on the page, a card, a raised card and a selected row', (style, id) => {
    const ring = solidSlot(style, id, 'accent-text')
    for (const g of ['bg', 'bg-surface', 'bg-elevated'] as const) {
      expect(contrast(ring, solidSlot(style, id, g)), `${id} ring on ${g}`).toBeGreaterThanOrEqual(3)
    }
    const card = solidSlot(style, id, 'bg-surface')
    const selected = mixRgb(solidSlot(style, id, 'accent'), card, 0.15)
    expect(contrast(ring, selected), `${id} ring on a selected row`).toBeGreaterThanOrEqual(3)
  })
})

// ── 2 ───────────────────────────────────────────────────────────────────

describe('2. Status colours as text', () => {
  const TEXT = ['status-error-text', 'status-warn-text', 'status-ok-soft'] as const

  it.each(SHIPPED)('%s %s: error, warning and success text read 4.5:1 on every surface and its own tint', (style, id) => {
    for (const t of TEXT) {
      const ink = solidSlot(style, id, t)
      for (const g of SURFACES) expect(contrast(ink, solidSlot(style, id, g)), `${id} ${t} on ${g}`).toBeGreaterThanOrEqual(4.5)
    }
    // The tinted badges: needs-you at 12% and the ok tint at 10%, on a card and a raised card.
    for (const g of ['bg-surface', 'bg-elevated'] as const) {
      const ground = solidSlot(style, id, g)
      const warnTint = mixRgb(solidSlot(style, id, 'status-needs-you'), ground, 0.12)
      expect(contrast(solidSlot(style, id, 'status-warn-text'), warnTint), `${id} warning badge on ${g}`).toBeGreaterThanOrEqual(4.5)
      const okTint = mixRgb(solidSlot(style, id, 'status-ok-soft'), ground, 0.1)
      expect(contrast(solidSlot(style, id, 'status-ok-soft'), okTint), `${id} success badge on ${g}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it('text never takes a status fill colour; icons and dots keep it', () => {
    // Glyph-only uses (an icon or a mark, 3:1) that keep the status colour.
    const ICONS = new Set([
      'src/renderer/components/Settings/shared/PhonePairing.tsx',
      'src/renderer/components/Mobile/MobileNotice.tsx',
      'src/renderer/components/AIFileEditor/AIFileEditor.tsx',
      'src/renderer/components/Voice/CallControls.tsx',
    ])
    const fill = /(?<![a-z-])(?:[a-z-]+:)*!?text-\[var\(--color-(?:status-(?:error|ok|warn|warn-soft|warn-amber|needs-you)|danger)\)\]/
    for (const [file, text] of sources()) {
      if (ICONS.has(file)) continue
      expect(text, file).not.toMatch(fill)
      expect(text, file).not.toMatch(/style=\{\{[^}]*\bcolor: ?'var\(--color-(?:status-(?:warn|needs-you)|danger)\)'/)
    }
  })

  it('the success tag inks with the success text colour', () => {
    expect(globals).toContain('--k2-tag-ink: color-mix(in srgb, var(--color-status-ok-soft) 75%, var(--color-text-primary));')
  })
})

// ── 3 ───────────────────────────────────────────────────────────────────

describe('3. Danger buttons', () => {
  it.each(SHIPPED)('%s %s: the label reads 4.5:1 on the danger, pressed and muted fills', (style, id) => {
    const label = solidSlot(style, id, 'on-danger')
    for (const fill of ['danger', 'danger-active', 'danger-muted']) {
      expect(contrast(label, solidSlot(style, id, fill)), `${id} label on ${fill}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it('every filled danger Button labels with --color-on-danger', () => {
    for (const variant of ['danger', 'danger-muted', 'danger-quiet']) {
      render(<Button variant={variant}>{variant}</Button>)
      const cls = screen.getByText(variant).className
      expect(cls, variant).toContain('var(--color-on-danger)')
      expect(cls, variant).not.toContain('var(--color-on-accent)')
    }
  })
})

// ── 4 ───────────────────────────────────────────────────────────────────

describe('4. Hover fills', () => {
  it.each(SHIPPED)('%s %s: accent text reads 4.5:1 and the control edge 3:1 on every hover fill', (style, id) => {
    const text = solidSlot(style, id, 'accent-text')
    const edge = solidSlot(style, id, 'control-edge')
    for (const [where, fill] of hoverFills(style, id)) {
      expect(contrast(text, fill), `${id} accent text on ${where}`).toBeGreaterThanOrEqual(4.5)
      expect(contrast(edge, fill), `${id} control edge on ${where}`).toBeGreaterThanOrEqual(3)
    }
    for (const g of ['bg', 'bg-surface', 'bg-elevated'] as const) {
      expect(contrast(text, solidSlot(style, id, g)), `${id} accent text on ${g}`).toBeGreaterThanOrEqual(4.5)
    }
    for (const g of SURFACES) expect(contrast(edge, solidSlot(style, id, g)), `${id} control edge on ${g}`).toBeGreaterThanOrEqual(3)
  })
})

describe('4. Glass: a hovered outlined button', () => {
  it('its edge lightens to muted text with the fill', () => {
    expect(generated).toMatch(/:not\(\[class\*="hover:bg-\[var\(--color-danger"\]\):hover:not\(:disabled\):not\(\[data-zen-root\] \*, \[data-k2-thread-zen\] \*\) \{\s*background: color-mix\(in srgb, var\(--tile-raised\) 90%, var\(--color-text-primary\)\);\s*border-color: var\(--color-text-muted\);/)
  })

  it.each(GLASS)('%s %s: that edge reads 3:1 on the hovered fill, the page and a card', (style, id) => {
    const edge = solidSlot(style, id, 'text-muted')
    const fill = mixRgb(solidSlot(style, id, 'bg-elevated'), solidSlot(style, id, 'text-primary'), 0.9)
    expect(contrast(edge, fill), `${id} edge on the hovered fill`).toBeGreaterThanOrEqual(3)
    for (const g of ['bg', 'bg-surface'] as const) expect(contrast(edge, solidSlot(style, id, g)), `${id} edge on ${g}`).toBeGreaterThanOrEqual(3)
  })
})

// ── 5 ───────────────────────────────────────────────────────────────────

describe('5. The off switch in Veil', () => {
  it('the knob takes the control edge as its rim; the track keeps its edge', () => {
    expect(generated).toMatch(/\[data-style="glass"\]\[data-palette="veil"\] \.k2-switch\[aria-checked="false"\]:not\(:disabled\) \.k2-switch-knob:not\(\[data-zen-root\] \*\) \{\s*box-shadow: [^;]*0 0 0 1px var\(--color-control-edge\);/)
    expect(generated).toContain('[data-style="glass"] .k2-switch[aria-checked="false"]:not([data-zen-root] *) { box-shadow: inset 0 0 0 1px var(--color-control-edge); }')
  })

  it('the rim and the track edge read 3:1 on the track, the page and a card', () => {
    const edge = solidSlot('glass', 'veil', 'control-edge')
    for (const g of ['bg', 'bg-surface'] as const) {
      const ground = solidSlot('glass', 'veil', g)
      const track = on('glass', 'veil', 'control-track-off', ground)
      expect(contrast(edge, track), `rim on the track over ${g}`).toBeGreaterThanOrEqual(3)
      expect(contrast(edge, ground), `edge on ${g}`).toBeGreaterThanOrEqual(3)
    }
  })
})

// ── 6 ───────────────────────────────────────────────────────────────────

describe('6. The Tickets count in the top bar', () => {
  it('inks with --color-on-warn on the needs-you fill', () => {
    const tabs = readFileSync(join(repo, 'src/renderer/components/TopBar/PageTabs.tsx'), 'utf8')
    expect(tabs).toContain("badgeClass: 'bg-[var(--color-status-needs-you)] !text-[var(--color-on-warn)]',")
  })

  it.each(SHIPPED)('%s %s: 4.5:1', (style, id) => {
    expect(contrast(solidSlot(style, id, 'on-warn'), solidSlot(style, id, 'status-needs-you'))).toBeGreaterThanOrEqual(4.5)
  })
})

// ── 7 ───────────────────────────────────────────────────────────────────

describe('7. Secondary text in Yellow and Dusk', () => {
  it.each(SHIPPED.filter(([, id]) => id === 'yellow'))('%s %s: 4.5:1 on hover and raised surfaces, and clearly above muted', (style, id) => {
    const secondary = solidSlot(style, id, 'text-secondary')
    for (const g of SURFACES) expect(contrast(secondary, solidSlot(style, id, g)), `on ${g}`).toBeGreaterThanOrEqual(4.5)
    for (const g of ['bg', 'bg-surface', 'bg-elevated'] as const) {
      expect(contrast(secondary, on(style, id, 'row-hover', solidSlot(style, id, g))), `on the row band over ${g}`).toBeGreaterThanOrEqual(4.5)
    }
    const page = solidSlot(style, id, 'bg')
    expect(contrast(secondary, page) / contrast(solidSlot(style, id, 'text-muted'), page), 'secondary over muted').toBeGreaterThanOrEqual(1.2)
  })
})
