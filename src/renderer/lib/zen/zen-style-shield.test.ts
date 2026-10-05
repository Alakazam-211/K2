// Zen is separate from Styles (Z20). The shield (`zen-style-shield.ts`)
// re-points every Styles variable at a Zen token inside the Zen root. These
// ratchets fail when a Styles variable exists that the shield doesn't cover
// (it would leak the user's Style into Zen), when the shield points at
// anything but Zen's own variables, or when the basic theme's text and
// fill pairs drop under WCAG AA (4.5:1).

import { describe, expect, it } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ZEN_STYLE_SHIELD, zenBubbleShield } from './zen-style-shield'
import { ZEN_BUNDLE_VARS, ZEN_DEFAULT_COLORS, ZEN_THEME_VARS, type ZenColorToken } from './zen-tokens'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const REPO = join(RENDERER, '..', '..')

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) {
      if (e.name !== 'node_modules' && e.name !== 'public') walk(p, out)
    } else if (/\.(ts|tsx|css)$/.test(e.name) && !/\.test\.(ts|tsx)$/.test(e.name)) out.push(p)
  }
  return out
}

/** Styles variables Zen must cover: colours, the UI fonts, radii, terminal palette, dividers. */
const isStylesVar = (name: string): boolean =>
  /^--(color-|radius-|term-)/.test(name) || name === '--font-ui' || name === '--font-display' || name === '--divider-color'

describe('Styles shield coverage', () => {
  it('covers every Styles variable a Style declares', () => {
    const css = readFileSync(join(RENDERER, 'styles.generated.css'), 'utf8')
    const declared = new Set(Array.from(css.matchAll(/^\s*(--[a-z0-9-]+)\s*:/gm), (m) => m[1]).filter(isStylesVar))
    expect(declared.size).toBeGreaterThan(80)
    const missing = [...declared].filter((n) => !(n in ZEN_STYLE_SHIELD)).sort()
    expect(missing).toEqual([])
  })

  it('covers every Styles colour variable any renderer code reads', () => {
    const read = new Set<string>()
    for (const f of walk(RENDERER, [])) {
      for (const m of readFileSync(f, 'utf8').matchAll(/var\((--color-[a-z0-9-]+)/g)) read.add(m[1])
    }
    expect(read.has('--color-text-primary')).toBe(true)
    const missing = [...read].filter((n) => !(n in ZEN_STYLE_SHIELD)).sort()
    expect(missing).toEqual([])
  })

  it('points only at Zen variables the theme engine writes', () => {
    const known = new Set([...ZEN_THEME_VARS, ...ZEN_BUNDLE_VARS])
    const values = [...Object.values(ZEN_STYLE_SHIELD), ...Object.values(zenBubbleShield('me')), ...Object.values(zenBubbleShield('agent'))]
    const refs = values.flatMap((v) => Array.from(v.matchAll(/var\((--[a-z0-9-]+)/g), (m) => m[1]))
    expect(refs.length).toBeGreaterThan(100)
    expect(refs.filter((r) => !known.has(r))).toEqual([])
  })

  it('bubbles use their own text colour for text and code, and the accent only on the agent side', () => {
    const agent = zenBubbleShield('agent')
    const me = zenBubbleShield('me')
    for (const k of ['--color-text', '--color-text-primary', '--color-text-secondary', '--color-code-inline']) {
      expect([k, agent[k], me[k]]).toEqual([k, 'var(--zen-bubble-agent-text)', 'var(--zen-bubble-me-text)'])
    }
    expect(agent['--color-accent']).toBe('var(--zen-accent)')
    // My bubble IS the accent: a link in the accent would vanish.
    expect(me['--color-accent']).toBe('var(--zen-bubble-me-text)')
  })
})

// WCAG 2.x relative luminance and contrast ratio.
function luminance(hex: string): number {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!m) throw new Error(`not a #rrggbb colour: ${hex}`)
  const ch = [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255)
  const [r, g, b] = ch.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** `[colors.<scheme>]` from the daemon's built-in `basic` theme. */
function tomlColors(scheme: 'light' | 'dark'): Record<string, string> {
  const toml = readFileSync(join(REPO, 'crates', 'k2-core', 'src', 'zen', 'themes', 'basic.toml'), 'utf8')
  const start = toml.indexOf(`[colors.${scheme}]`)
  if (start < 0) throw new Error(`basic.toml has no [colors.${scheme}]`)
  const body = toml.slice(start).split('\n').slice(1)
  const out: Record<string, string> = {}
  for (const line of body) {
    if (line.startsWith('[')) break
    const m = /^([a-z-]+)\s*=\s*"(#[0-9a-fA-F]{6})"/.exec(line)
    if (m) out[m[1]] = m[2]
  }
  return out
}

describe('default Zen theme contrast (WCAG AA, 4.5:1)', () => {
  const PAIRS: Array<[ZenColorToken, ZenColorToken]> = [
    ['bubble-agent', 'bubble-agent-text'],
    ['bubble-me', 'bubble-me-text'],
    ['canvas', 'text'],
    ['surface', 'text'],
    ['canvas', 'text-muted'],
    ['accent', 'accent-text'],
  ]

  for (const scheme of ['light', 'dark'] as const) {
    it(`${scheme}: bubbles and text read at 4.5:1 or better`, () => {
      const toml = tomlColors(scheme)
      for (const [bg, fg] of PAIRS) {
        // The renderer's copy is the daemon's file.
        expect([bg, ZEN_DEFAULT_COLORS[scheme][bg]]).toEqual([bg, toml[bg]])
        expect([fg, ZEN_DEFAULT_COLORS[scheme][fg]]).toEqual([fg, toml[fg]])
        const ratio = contrast(toml[bg], toml[fg])
        expect([scheme, bg, fg, ratio >= 4.5]).toEqual([scheme, bg, fg, true])
      }
    })
  }

  it('the WCAG maths is right (black on white is 21:1)', () => {
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 5)
    expect(contrast('#777777', '#ffffff')).toBeCloseTo(4.48, 2)
  })
})
