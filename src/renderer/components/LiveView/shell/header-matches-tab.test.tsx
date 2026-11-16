// @vitest-environment jsdom
// The header row under a file tab is the selected tab's colour
// (color.tab-active), in every look: the tab runs into its header like a
// folder tab, and the header's hairline parts it from the content. Each look's
// palette is resolved the way the browser does (var() and color-mix in sRGB),
// for what the selected tab paints and for what the header paints, and the
// two must be the same colour. The token also keeps what each look painted
// before it existed, and the header's words keep 4.5:1 on it.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { EDITOR_CSS } from '@/components/Editor/editorCss'
import { LiveHeader } from './LiveHeader'
import { LIVE_SHELL_CSS } from './shellCss'

const repo = join(__dirname, '..', '..', '..', '..', '..')
const read = (...p: string[]): string => readFileSync(join(repo, ...p), 'utf8')

type Rgb = [number, number, number]

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(read('styles', style, 'palettes', `${id}.json`))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [`--color-${k}`, v.$value]))
}

/** A colour expression in a look: #hex, white, rgb(), rgba() over nothing, var(), color-mix(in srgb, A p%, B). */
function resolve(expr: string, vars: Record<string, string>, depth = 0): Rgb {
  if (depth > 20) throw new Error(`var() loop at ${expr}`)
  const e = expr.trim()
  const v = /^var\((--[\w-]+)(?:,\s*(.+))?\)$/.exec(e)
  if (v) {
    const value = vars[v[1]] ?? v[2]
    if (value === undefined) throw new Error(`unset ${v[1]}`)
    return resolve(value, vars, depth + 1)
  }
  const mix = /^color-mix\(in srgb,\s*(.+)\)$/.exec(e)
  if (mix) {
    const [a, b] = splitTop(mix[1])
    const pa = /^(.*)\s+([\d.]+)%$/.exec(a.trim())
    if (!pa) throw new Error(`color-mix needs A p%: ${e}`)
    const p = Number(pa[2]) / 100
    const ca = resolve(pa[1], vars, depth + 1), cb = resolve(b, vars, depth + 1)
    return ca.map((c, i) => c * p + cb[i] * (1 - p)) as Rgb
  }
  if (e === 'white') return [255, 255, 255]
  const hex = /^#([0-9a-f]{6})$/i.exec(e)
  if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)) as Rgb
  const rgb = /^rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*1)?\)$/.exec(e)
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])]
  throw new Error(`can't resolve ${e}`)
}
function splitTop(s: string): [string, string] {
  let d = 0
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '(') d++
    else if (s[i] === ')') d--
    else if (s[i] === ',' && d === 0) return [s.slice(0, i), s.slice(i + 1)]
  }
  throw new Error(`no top-level comma in ${s}`)
}
/** An rgba() wash laid over a solid ground. */
function over(wash: string, ground: Rgb): Rgb {
  const m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(wash)
  if (!m) throw new Error(`not an rgba() wash: ${wash}`)
  const a = Number(m[4])
  return [1, 2, 3].map((i, k) => Number(m[i]) * a + ground[k] * (1 - a)) as Rgb
}
const lum = ([r, g, b]: Rgb): number => {
  const f = (x: number): number => { const s = x / 255; return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4 }
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}
const contrast = (a: Rgb, b: Rgb): number => { const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05) }
const same = (a: Rgb, b: Rgb): boolean => a.every((c, i) => Math.abs(c - b[i]) < 0.51)

const GLASS_CSS = read('styles', 'glass', 'overrides.css')
const TERMINAL_CSS = read('styles', 'terminal', 'overrides.css')
const TAB_BAR = read('src', 'renderer', 'components', 'TabBar', 'TabBar.tsx')

/** A custom property's declaration in the rule whose selector starts with `sel`. */
function declIn(css: string, sel: string, prop: string): string | undefined {
  const i = css.indexOf(sel)
  if (i < 0) return undefined
  const body = css.slice(css.indexOf('{', i) + 1, css.indexOf('}', i))
  return new RegExp(`(?:^|[;\\s])${prop.replace(/[-]/g, '\\-')}:\\s*([^;]+);`).exec(body)?.[1].trim()
}

/** Rules that paint a selected tab itself (not its ::before/::after, not a :not([data-selected]) tab). */
function repaintsSelectedTab(css: string): string[] {
  const out: string[] = []
  for (const m of css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const sel = m[1].replace(/:not\((?:[^()]|\([^()]*\))*\)/g, '')
    if (sel.includes('[data-tab-id]') && sel.includes('[data-selected]') && !sel.includes('::') && /(^|;)\s*background(-color)?:/.test(m[2])) out.push(m[1].trim())
  }
  return out
}

/** Terminal's header-row rule ([data-tab-header]): its custom properties. */
const HEADER_ROW_VARS: Record<string, string> = (() => {
  const i = TERMINAL_CSS.indexOf('[data-style="terminal"] [data-tab-header]:not([data-zen-root] *) {')
  if (i < 0) return {}
  const body = TERMINAL_CSS.slice(TERMINAL_CSS.indexOf('{', i) + 1, TERMINAL_CSS.indexOf('}', i))
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]))
})()

/** What the selected room tab paints, from the style's own rules. */
function tabPaint(style: 'terminal' | 'glass', vars: Record<string, string>): Rgb {
  if (style === 'glass') {
    // The selected tab is the wave; the wave reads the token on the tab group.
    expect(GLASS_CSS).toMatch(/\[data-room-tab-bar\] \[data-tab-id\]:is\(\[data-tab-rail\], :not\(\[data-tab-rail\]\)\)\[data-selected\]:not\(\[data-zen-root\] \*\) \{[^}]*background: var\(--k2-wave-fill\);/)
    const wave = declIn(GLASS_CSS, '[data-style="glass"] [data-tab-group-index]:not([data-zen-root] *) {', '--k2-wave-fill')
    expect(wave).toBe('var(--color-tab-active)')
    return resolve(wave!, vars)
  }
  // Terminal: the TabBar's own fill, which no Terminal rule repaints.
  expect(TAB_BAR).toContain("'bg-[var(--color-tab-active)] text-[var(--color-text-primary)]'")
  expect(repaintsSelectedTab(TERMINAL_CSS)).toEqual([])
  return resolve('var(--color-tab-active)', vars)
}

/** What the header row paints: the live header's own fill, which the editor keeps. */
function headerPaint(vars: Record<string, string>): Rgb {
  const head = screen.getByTestId('lv-shell-header')
  const fill = /bg-\[(var\(--[\w-]+\))\]/.exec(head.className)?.[1]
  expect(fill).toBe('var(--color-tab-active)')
  expect(EDITOR_CSS).toContain('.ed-tab .lv-head { background: var(--color-tab-active); }')
  return resolve(fill!, vars)
}

// [style, palette, what the selected tab painted before the token]
const LOOKS: Array<[string, 'terminal' | 'glass', string, (p: Record<string, string>) => Rgb]> = [
  ['Terminal dark (Charcoal)', 'terminal', 'charcoal', (p) => over('rgba(255, 255, 255, 0.08)', resolve(p['--color-bg-surface'], p))],
  ['Terminal light (Paper)', 'terminal', 'paper', (p) => over(p['--color-wash-3'], resolve(p['--color-bg-surface'], p))],
  ['Terminal Yellow', 'terminal', 'yellow', (p) => over('rgba(255, 255, 255, 0.08)', resolve(p['--color-bg-surface'], p))],
  ['Glass dark (Obsidian)', 'glass', 'obsidian', (p) => resolve(p['--color-bg'], p)],
  // Veil's tab group repaints bg with the tile (bg-surface), and the wave was that bg.
  ['Glass light (Veil)', 'glass', 'veil', (p) => resolve(p['--color-bg-surface'], p)],
  ['Glass Dusk (yellow)', 'glass', 'yellow', (p) => resolve(p['--color-bg'], p)],
]

describe('The header row matches the selected tab', () => {
  afterEach(cleanup)

  it.each(LOOKS)('%s: the tab and the header resolve to one colour, the one the tab painted before', (_name, style, id, before) => {
    const vars = palette(style, id)
    render(<LiveHeader fileName="README.md" sentence={null} />)
    const tab = tabPaint(style, vars)
    const head = headerPaint(vars)
    expect(same(tab, head), `tab ${tab} vs header ${head}`).toBe(true)
    expect(same(tab, before(vars)), `tab ${tab} vs before ${before(vars)}`).toBe(true)
    for (const slot of ['--color-text-primary', '--color-text-secondary']) {
      expect(contrast(resolve(vars[slot], vars), head), `${id} ${slot} on the header`).toBeGreaterThanOrEqual(4.5)
    }
    // On a Terminal header row muted takes secondary and a quiet control's press steps off the row.
    const row = style === 'terminal' ? { ...vars, ...HEADER_ROW_VARS } : vars
    expect(contrast(resolve(row['--color-text-muted'], row), head), `${id} muted on the header`).toBeGreaterThanOrEqual(4.5)
    expect(contrast(resolve(row['--color-control-hover-quiet'], row), head), `${id} a pressed quiet control on the header`).toBeGreaterThanOrEqual(style === 'terminal' ? 1.25 : 1)
    expect(screen.getByTestId('lv-shell-header').hasAttribute('data-tab-header')).toBe(true)
  })

  it('every palette of every style carries the slot (the build requires it)', () => {
    expect(read('scripts', 'build-styles.mjs')).toMatch(/'tab-active',\n\]/)
    const css = read('src', 'renderer', 'styles.generated.css')
    const blocks = (css.match(/\[data-style="[\w-]+"\]\[data-palette="[\w-]+"\] \{[^}]*\}/g) ?? []).filter((b) => b.includes('--color-bg:'))
    expect(blocks.length).toBeGreaterThanOrEqual(6)
    for (const b of blocks) expect(b, b.slice(0, 60)).toContain('--color-tab-active:')
  })

  it('the pinned tabs, the badge mask and the avatar ring in the header read the token', () => {
    expect(TAB_BAR.match(/'var\(--k2-rail-lit, var\(--color-tab-active\)\)'/g)).toHaveLength(2)
    expect(TAB_BAR).toContain("'--tab-bg': isActive ? 'var(--color-tab-active)'")
    expect(LIVE_SHELL_CSS).toContain('.lv-shell .lv-head .lv-ring { background: var(--color-tab-active); }')
  })

  it('the other header rows under a tab paint it too', () => {
    const src = (...p: string[]): string => read('src', 'renderer', 'components', ...p)
    expect(src('FileViewerPane', 'FileViewerPane.tsx').match(/border-b border-\[var\(--color-border\)\] bg-\[var\(--color-tab-active\)\] px-3 py-1\.5/g)).toHaveLength(2)
    expect(src('DiffViewer', 'DiffViewer.tsx')).toContain('border-b border-[var(--color-border)] bg-[var(--color-tab-active)]')
    expect(src('BrowserPane', 'BrowserPane.tsx')).toContain('flex h-9 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-tab-active)]')
    expect(src('LiveView', 'WorkbookChrome.tsx')).toContain('bg-[var(--color-tab-active)] px-3 py-1.5" data-testid="lv-header"')
    // Each row is marked as the header under a tab (Terminal steps muted and the quiet press up on it).
    expect(src('FileViewerPane', 'FileViewerPane.tsx').match(/data-tab-header="" className="flex items-center gap-2 border-b border-\[var\(--color-border\)\] bg-\[var\(--color-tab-active\)\]/g)).toHaveLength(2)
    for (const f of [['DiffViewer', 'DiffViewer.tsx'], ['BrowserPane', 'BrowserPane.tsx'], ['LiveView', 'WorkbookChrome.tsx']]) {
      expect(src(...f), f[1]).toMatch(/data-tab-header="" className="[^"]*bg-\[var\(--color-tab-active\)\]/)
    }
    expect(HEADER_ROW_VARS['--color-text-muted']).toBe('var(--color-text-secondary)')
  })
})
