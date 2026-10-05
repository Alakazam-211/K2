// Rosson 2026-10-04 — one glass for every Zen tile (`zen-glass.ts`): WebKit
// terms, Zen tokens only, a solid fallback under reduced transparency, and
// the built-in themes (basic / midnight / paper, light and dark) give it
// every token it reads. The tiles themselves (and the Tickets / usage
// stylesheets built on it) are checked in `ZenWidgets.test.tsx`.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ZEN_GLASS_ATTR, ZEN_GLASS_CSS, ZEN_GLASS_PROPS, ZEN_GLASS_TOKENS_CSS, zenGlassRule } from './zen-glass'
import { buildZenTheme } from './zen-theme-engine'
import { BUILTIN_TEXTING_PAGE, type ZenResolvedPage } from './zen-page'
import { ZEN_COLOR_TOKENS } from './zen-tokens'

const REDUCED = '@media (prefers-reduced-transparency: reduce)'

/** The `--zen-*` names a stylesheet reads. */
function zenVarsRead(css: string): string[] {
  return [...new Set(Array.from(css.matchAll(/var\((--[a-z0-9-]+)/g), (m) => m[1]))].sort()
}

/** The glass tokens' own names (`--zen-glass…`). */
const GLASS_TOKENS = Array.from(ZEN_GLASS_TOKENS_CSS.matchAll(/(--zen-glass[a-z-]*):/g), (m) => m[1])

describe('the shared Zen glass', () => {
  it('the marker: one attribute every tile spreads', () => {
    expect(ZEN_GLASS_ATTR).toBe('data-zen-glass')
    expect(ZEN_GLASS_PROPS).toEqual({ 'data-zen-glass': '' })
  })

  it('the Tickets panels’ and usage chip’s recipe: solid surface inside, no backdrop blur, soft edge, top highlight + shadow', () => {
    expect(GLASS_TOKENS.sort()).toEqual(
      ['--zen-glass', '--zen-glass-blur', '--zen-glass-edge', '--zen-glass-raised', '--zen-glass-shadow', '--zen-glass-sheen'].sort(),
    )
    // Flat inside (Rosson 2026-10-04): the fill is the solid surface.
    expect(ZEN_GLASS_TOKENS_CSS).toContain('--zen-glass: var(--zen-surface);')
    expect(ZEN_GLASS_TOKENS_CSS).toContain('--zen-glass-raised: var(--zen-surface-raised);')
    // No backdrop blur (stale WebKit layers after a theme switch, Rosson 2026-10-04).
    expect(ZEN_GLASS_TOKENS_CSS).toContain('--zen-glass-blur: none;')
    expect(ZEN_GLASS_TOKENS_CSS).not.toMatch(/blur\(/)
    expect(ZEN_GLASS_TOKENS_CSS).toContain(
      '--zen-glass-edge: color-mix(in srgb, var(--zen-border) 55%, color-mix(in srgb, var(--zen-text) 14%, transparent));',
    )
    expect(ZEN_GLASS_TOKENS_CSS).toContain('--zen-glass-sheen: inset 0 1px 0 color-mix(in srgb, white 22%, transparent);')
    const tile = ZEN_GLASS_CSS.slice(0, ZEN_GLASS_CSS.indexOf(REDUCED))
    expect(tile).toContain('[data-zen-root] [data-zen-glass] {')
    expect(tile).toContain('background: var(--zen-glass);')
    expect(tile).toContain('-webkit-backdrop-filter: var(--zen-glass-blur);')
    expect(tile).toContain('backdrop-filter: var(--zen-glass-blur);')
    expect(tile).toContain('border: 1px solid var(--zen-glass-edge);')
    expect(tile).toContain('box-shadow: var(--zen-glass-sheen), var(--zen-glass-shadow);')
  })

  it('WebKit-safe and subtle: no SVG filter, no url(, no gradient; Zen tokens only', () => {
    expect(ZEN_GLASS_CSS).not.toMatch(/url\(/)
    expect(ZEN_GLASS_CSS).not.toMatch(/gradient\(/)
    expect(ZEN_GLASS_CSS).not.toMatch(/var\(--color-/)
    // Every -webkit-backdrop-filter has its unprefixed twin, and back.
    expect(ZEN_GLASS_CSS.match(/-webkit-backdrop-filter:/g)?.length).toBe(
      ZEN_GLASS_CSS.match(/(?<!-webkit-)backdrop-filter:/g)?.length,
    )
    // It reads only Zen colour tokens and its own glass tokens.
    const allowed = new Set([...ZEN_COLOR_TOKENS.map((t) => `--zen-${t}`), ...GLASS_TOKENS])
    for (const v of zenVarsRead(ZEN_GLASS_CSS)) expect(allowed.has(v), v).toBe(true)
  })

  it('reduced transparency: a solid Zen surface, no blur, for every glass selector', () => {
    const rule = zenGlassRule(['.a', '.b'])
    const reduced = rule.slice(rule.indexOf(REDUCED))
    expect(rule.indexOf(REDUCED)).toBeGreaterThan(0)
    expect(reduced).toMatch(/\.a,\s*\.b \{/)
    expect(reduced).toContain('background: var(--zen-surface);')
    expect(reduced).toContain('-webkit-backdrop-filter: none;')
    expect(reduced).toContain('backdrop-filter: none;')
    const shared = ZEN_GLASS_CSS.slice(ZEN_GLASS_CSS.indexOf(REDUCED))
    expect(shared).toContain('[data-zen-root] [data-zen-glass] {')
    expect(shared).toContain('background: var(--zen-surface);')
    // A hovered / open glass button is solid too.
    expect(shared).toContain('background: var(--zen-surface-raised);')
    expect(() => zenGlassRule([])).toThrow('no selectors')
  })
})

describe('the glass in every built-in theme (basic / midnight / paper, light and dark)', () => {
  function themeColors(name: string): Partial<Record<'light' | 'dark', Record<string, string>>> {
    const toml = readFileSync(resolve(process.cwd(), `crates/k2-core/src/zen/themes/${name}.toml`), 'utf8')
    const out: Partial<Record<'light' | 'dark', Record<string, string>>> = {}
    for (const scheme of ['light', 'dark'] as const) {
      const m = new RegExp(`^\\[colors\\.${scheme}\\]\\n([\\s\\S]*?)(?=^\\[|$(?![\\s\\S]))`, 'm').exec(toml)
      if (!m) continue
      const t: Record<string, string> = {}
      for (const line of m[1].split('\n')) {
        const kv = /^([a-z-]+)\s*=\s*"([^"]*)"/.exec(line.trim())
        if (kv) t[kv[1]] = kv[2]
      }
      out[scheme] = t
    }
    return out
  }

  const page = (colors: object, scheme: string | null): ZenResolvedPage => ({
    ...BUILTIN_TEXTING_PAGE,
    theme: scheme ? { scheme, colors } : { colors },
  })

  const read = zenVarsRead(ZEN_GLASS_CSS).filter((v) => !v.startsWith('--zen-glass'))

  for (const [name, scheme] of [
    ['basic', 'light'],
    ['basic', 'dark'],
    ['midnight', 'dark'],
    ['paper', 'light'],
  ] as const) {
    it(`${name} ${scheme}: every token the glass reads is an opaque colour, and a tile stands off the canvas`, () => {
      const colors = themeColors(name)
      // midnight is always dark and paper always light; basic follows the system.
      const forced = name === 'basic' ? null : scheme
      if (forced && Object.keys(colors).some((s) => s !== forced)) throw new Error(`${name} has another scheme's colours`)
      for (const system of ['light', 'dark'] as const) {
        if (!forced && system !== scheme) continue
        const r = buildZenTheme({ systemScheme: system, page: page(colors, forced) })
        expect(r.scheme).toBe(scheme)
        expect(r.rejected ?? []).toEqual([])
        for (const v of read) expect(r.vars[v], `${name} ${scheme} ${v}`).toMatch(/^rgb\(\d+, \d+, \d+\)$/)
        expect(r.vars['--zen-surface']).not.toBe(r.vars['--zen-canvas'])
        // Reduced transparency: the fallback's surface is solid as well.
        const solid = buildZenTheme({ systemScheme: system, page: page(colors, forced), reducedTransparency: true })
        expect(solid.vars['--zen-surface']).toMatch(/^rgb\(\d+, \d+, \d+\)$/)
      }
    })
  }
})
