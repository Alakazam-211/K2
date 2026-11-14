// @vitest-environment jsdom
// Two contrast rules from globals.css, in every palette the two Styles ship:
//  1. Muted text on a hovered or selected row steps up to secondary text,
//     which reads at 4.5:1 on the row's band where muted did not.
//  2. In Glass, a focused filled button draws a 1.5px line of its label
//     colour inside its edge (Terminal's cut ring already does), since the
//     ring alone, the accent as text, barely shows against the accent fill.
// Rules are read from globals.css with comments removed; selectors are
// matched against real elements; colours come from the style packages and
// are measured with the WCAG formula.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { Button } from './Button'

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = strip(readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8'))

afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
  document.documentElement.removeAttribute('data-style')
})

/** The selector of the one rule whose whole body is `body`. */
function selectorOf(body: string): string {
  const at = globals.indexOf(`{\n  ${body}\n}`)
  expect(at, `a rule with the body ${body}`).toBeGreaterThanOrEqual(0)
  expect(globals.indexOf(`{\n  ${body}\n}`, at + 1), `only one rule with the body ${body}`).toBe(-1)
  return globals.slice(globals.lastIndexOf('}', at) + 1, at).trim()
}

// ── Colours ─────────────────────────────────────────────────────────────

type Rgb = [number, number, number]

const SHIPPED = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['terminal', 'yellow'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
  ['glass', 'yellow'],
] as const

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

const mixOver = (top: Rgb, ground: Rgb, t: number): Rgb => top.map((c, i) => c * t + ground[i] * (1 - t)) as Rgb

/** A slot's colour: #rrggbb, a var() of another slot or Glass's accent,
 *  color-mix(in srgb, <colour> N%, black|white). A slot the palette leaves
 *  out takes the generated default (accent-fill: the accent 82% to black). */
function resolve(style: string, id: string, value: string | undefined, slot = ''): Rgb {
  if (value === undefined) {
    expect(slot, `${id} leaves out ${slot}`).toBe('accent-fill')
    return resolve(style, id, 'color-mix(in srgb, var(--color-accent) 82%, black)')
  }
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
  if (ref) return slotColour(style, id, ref[1])
  const m = /^color-mix\(in srgb, (.+) (\d+)%, (black|white)\)$/.exec(v)
  if (!m) throw new Error(`can't resolve ${value}`)
  return mixOver(resolve(style, id, m[1]), resolve(style, id, m[3]), Number(m[2]) / 100)
}
function slotColour(style: string, id: string, slot: string): Rgb {
  return resolve(style, id, palette(style, id)[slot], slot)
}

/** --color-accent-text: the accent, unless the Style's overrides darken or
 *  lighten it for this palette (styles.generated.css). */
function accentText(style: string, id: string): Rgb {
  const m = new RegExp(`\\[data-style="${style}"\\]\\[data-palette="${id}"\\] \\{\\s*--color-accent-text: (color-mix\\([^;]+\\));`).exec(generated)
  return m ? resolve(style, id, m[1]) : slotColour(style, id, 'accent')
}

/** An rgba() wash over a solid ground. */
function wash(value: string, ground: Rgb): Rgb {
  const m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(value)
  if (!m) throw new Error(`not an rgba() wash: ${value}`)
  return mixOver([Number(m[1]), Number(m[2]), Number(m[3])], ground, Number(m[4]))
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

// ── 1. Rows ─────────────────────────────────────────────────────────────

describe('muted text on a hovered or selected row', () => {
  const sel = selectorOf('--color-text-muted: var(--color-text-secondary);')

  /** Does the rule reach `el`? Hover and keyboard focus can't be set in
   *  jsdom, so they are checked as written; the other states are attributes. */
  const steps = (el: Element): boolean => el.matches(sel)
  const make = (html: string): HTMLElement => {
    document.body.innerHTML = html
    return document.querySelector('[data-t]') as HTMLElement
  }

  it('is one shared rule for hover, keyboard focus, highlight, pick, current and selected', () => {
    for (const part of [':hover', ':focus-visible', '[data-highlighted]', '[data-selected]', '[aria-current="true"]', '[aria-current="page"]', '[aria-selected="true"]']) {
      expect(sel, part).toContain(part)
    }
    expect(sel).toContain(':not(:disabled, [aria-disabled="true"])')
  })

  it('reaches the selected agent row, a current list item, a selected option and a lit menu item', () => {
    expect(steps(make('<button data-t data-agent-row aria-current="true"></button>'))).toBe(true)
    expect(steps(make('<button data-t data-person-row aria-current="true"></button>'))).toBe(true)
    expect(steps(make('<div role="listbox"><div data-t role="option" aria-selected="true"></div></div>'))).toBe(true)
    expect(steps(make('<div data-context-menu><button data-t data-context-menu-item="a" data-highlighted></button></div>'))).toBe(true)
    expect(steps(make('<div data-setting-dropdown-menu><button data-t data-value="a" data-selected></button></div>'))).toBe(true)
    expect(steps(make('<div role="menu"><button data-t role="menuitem" data-highlighted></button></div>'))).toBe(true)
  })

  it('leaves a resting row, a disabled item, a selected tab and Gardens alone', () => {
    expect(steps(make('<button data-t data-agent-row></button>'))).toBe(false)
    expect(steps(make('<div data-context-menu><button data-t data-context-menu-item="a"></button></div>'))).toBe(false)
    expect(steps(make('<div data-context-menu><button data-t aria-disabled="true" data-highlighted></button></div>'))).toBe(false)
    expect(steps(make('<div data-setting-dropdown-menu><button data-t disabled data-selected></button></div>'))).toBe(false)
    expect(steps(make('<div role="tablist"><button data-t role="tab" aria-selected="true" data-selected></button></div>'))).toBe(false)
    expect(steps(make('<div data-zen-root><button data-t data-agent-row aria-current="true"></button></div>'))).toBe(false)
    expect(steps(make('<div data-k2-thread-zen><div role="menu"><button data-t data-highlighted></button></div></div>'))).toBe(false)
  })

  it('the context menu marks its lit row (pointer or arrow keys) for the rule', () => {
    const menu = readFileSync(join(repo, 'src/renderer/components/ContextMenu/ContextMenu.tsx'), 'utf8')
    expect(menu).toContain("data-highlighted={focused || open ? '' : undefined}")
    expect(menu).toContain("background: focused || open ? 'var(--color-overlay-soft-border)' : 'transparent'")
  })

  // The bands a row paints, over the ground it sits on (lists sit on the
  // card, bg-surface, in both Styles; the agent rows on the nav, the same).
  const bands = (style: string, id: string): [string, Rgb][] => {
    const p = palette(style, id)
    const card = slotColour(style, id, 'bg-surface')
    const light = style === 'terminal' ? id === 'paper' : id === 'veil'
    return [
      ['a selected Settings row (the accent at 15%)', mixOver(slotColour(style, id, 'accent'), card, 0.15)],
      ['a selected agent row', light ? wash(p['wash-3'], card) : mixOver([255, 255, 255], card, 0.06)],
      ['a hovered agent row', light ? wash(p['wash-1'], card) : mixOver([255, 255, 255], card, 0.04)],
      ['a hovered Settings row', wash(p['row-hover'], card)],
    ]
  }

  it.each(SHIPPED)('%s %s: secondary text reads at 4.5:1 on every row band', (style, id) => {
    const secondary = slotColour(style, id, 'text-secondary')
    for (const [band, colour] of bands(style, id)) {
      expect(contrast(secondary, colour), `${id}: secondary text on ${band}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it('muted text alone fell under 4.5:1 on a band in Charcoal, Paper and Veil (why the rule exists)', () => {
    for (const [style, id] of [['terminal', 'charcoal'], ['terminal', 'paper'], ['glass', 'veil']] as const) {
      const muted = slotColour(style, id, 'text-muted')
      const worst = Math.min(...bands(style, id).map(([, c]) => contrast(muted, c)))
      expect(worst, `${id}: muted text on its worst band`).toBeLessThan(4.5)
    }
  })
})

// ── 2. Glass focus on filled buttons ───────────────────────────────────

describe('Glass: keyboard focus on a filled button', () => {
  const sel = selectorOf('box-shadow: inset 0 0 0 1.5px var(--k2-ch-focus-inner), var(--ring-field);')

  it('every Style but Terminal, on keyboard focus only, never in a Garden', () => {
    expect(sel).toBe('html:not([data-style="terminal"]) .k2-focus[class*="[--k2-ch-focus-inner:"]:focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *)')
    // The ring itself is unchanged: 2px of the accent as text, 2px out.
    expect(globals).toContain('html:not([data-style="terminal"]) :is(.k2-focus, .k2-checkbox, .k2-radio):focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *) {\n  outline: 2px solid var(--color-accent-text);\n  outline-offset: 2px;\n}')
  })

  it('reaches the filled variants (the ones Terminal gives an inner line) and no other', () => {
    document.documentElement.setAttribute('data-style', 'glass')
    const atRest = sel.replace(':focus-visible', '')
    for (const variant of ['accent', 'danger', 'danger-muted', 'default', 'ghost', 'quiet', 'danger-quiet']) {
      render(<Button variant={variant}>{variant}</Button>)
      const el = screen.getByText(variant)
      const filled = /\[--k2-ch-focus-inner:var\(--color-on-(?:accent|danger)\)\]/.test(el.className)
      expect(filled, `${variant} sets Terminal's inner line`).toBe(['accent', 'danger', 'danger-muted'].includes(variant))
      expect(el.matches(atRest), `${variant} takes Glass's inner line`).toBe(filled)
    }
    document.documentElement.setAttribute('data-style', 'terminal')
    expect(screen.getByText('accent').matches(atRest), 'Terminal keeps its cut ring').toBe(false)
  })

  it.each(SHIPPED.filter(([s]) => s === 'glass'))('%s %s: the line reads 3:1 on each fill, the ring 3:1 on the page and a card', (style, id) => {
    const line = slotColour(style, id, 'on-accent')
    const ring = accentText(style, id)
    for (const fill of ['accent-fill', 'danger']) {
      expect(contrast(line, slotColour(style, id, fill)), `${id}: the line on ${fill}`).toBeGreaterThanOrEqual(3)
    }
    for (const ground of ['bg', 'bg-surface']) {
      expect(contrast(ring, slotColour(style, id, ground)), `${id}: the ring on ${ground}`).toBeGreaterThanOrEqual(3)
    }
    // Why the line: the ring alone is under 3:1 against the accent fill.
    expect(contrast(ring, slotColour(style, id, 'accent-fill')), `${id}: the ring against the fill`).toBeLessThan(3)
  })
})
