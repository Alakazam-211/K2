// Veil (light Glass) is the phone's light glass, value for value: one even
// thin edge (black 16%, where Obsidian's is white 22%), a soft lift
// (0 3px 10px at 8%, no contact line and no light catch), and a frost a shade
// darker than the page (55% white over the blur, the backdrop dimmed to 0.94).
// The merged outline in Thread view carries the same edge and lift. Obsidian
// keeps every one of its own values.
import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')
const read = (p: string): string => readFileSync(join(repo, p), 'utf8')
const veil = JSON.parse(read('styles/glass/palettes/veil.json'))
const obsidian = JSON.parse(read('styles/glass/palettes/obsidian.json'))
const overrides = read('styles/glass/overrides.css')
const globals = read('src/renderer/globals.css')

type Rule = { selector: string; body: string }
const rulesOf = (css: string): Rule[] =>
  Array.from(css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]*)\}/g), (m) => ({ selector: m[1].trim(), body: m[2] }))
const glass = rulesOf(overrides)
const page = rulesOf(globals)

/** The value the last block for exactly `selector` gives `prop`. */
function decl(rules: Rule[], selector: string, prop: string): string {
  let found: string | null = null
  for (const r of rules) {
    if (r.selector !== selector) continue
    for (const d of r.body.split(';')) {
      const at = d.indexOf(':')
      if (at >= 0 && d.slice(0, at).trim() === prop) found = d.slice(at + 1).trim()
    }
  }
  if (found === null) throw new Error(`no ${prop} in ${selector}`)
  return found
}
const indexOf = (rules: Rule[], selector: string): number => {
  const i = rules.findIndex((r) => r.selector === selector)
  if (i < 0) throw new Error(`no rule ${selector}`)
  return i
}

const VEIL = '[data-style="glass"][data-palette="veil"]'
const FROST = '[data-style="glass"] [data-material="frost"]:not([data-zen-root] *)'
const VEIL_FROST = `${VEIL} [data-material="frost"]:not([data-zen-root] *)`
const VEIL_MERGE = `${VEIL} .thread-float-merge:not([data-zen-root] *)`
const VEIL_MERGE_FILL = 'html[data-style="glass"][data-palette="veil"] [data-k2-merge="fill"]'

describe('Veil is the phone light glass', () => {
  it('the edge: black at 16%, the frost: 55% white', () => {
    expect(veil.color['frost-edge'].$value).toBe('rgba(0, 0, 0, 0.16)')
    expect(veil.color['frost-fill'].$value).toBe('rgba(255, 255, 255, 0.55)')
  })

  it('the lift: 0 3px 10px at 8%, from parts the merged outline reads too', () => {
    expect(decl(glass, VEIL, '--k2-veil-lift-y')).toBe('3px')
    expect(decl(glass, VEIL, '--k2-veil-lift-blur')).toBe('10px')
    expect(decl(glass, VEIL, '--k2-veil-lift-color')).toBe('rgba(0, 0, 0, 0.08)')
    expect(decl(glass, VEIL, '--k2-veil-lift')).toBe('0 var(--k2-veil-lift-y) var(--k2-veil-lift-blur) var(--k2-veil-lift-color)')
  })

  it('the frost dims what is behind it to 0.94, on the Style blur and saturation', () => {
    expect(decl(glass, VEIL, '--k2-veil-frost-brightness')).toBe('0.94')
    expect(decl(glass, VEIL, '--k2-veil-frost-filter')).toBe(
      'blur(var(--material-blur)) saturate(var(--material-saturate)) brightness(var(--k2-veil-frost-brightness))',
    )
  })

  it('a frost object: the even edge, the lift and the frost, and nothing else', () => {
    expect(decl(glass, VEIL_FROST, 'box-shadow')).toBe('var(--k2-veil-lift)')
    expect(decl(glass, VEIL_FROST, 'backdrop-filter')).toBe('var(--k2-veil-frost-filter)')
    expect(decl(glass, VEIL_FROST, '-webkit-backdrop-filter')).toBe('var(--k2-veil-frost-filter)')
    // The edge is the shared Frost border, coloured by the palette.
    expect(decl(glass, FROST, 'border')).toBe('0.5px solid var(--material-edge)')
    // No second ring over it (the old gradient, bright at top and bottom).
    expect(overrides).not.toContain('--k2-veil-ring')
    expect(glass.filter((r) => r.selector.includes('[data-palette="veil"]') && r.selector.includes('::before') && /content:\s*""/.test(r.body))).toEqual([])
  })

  it('on a white tile the glass is a grey wash, and its frost does not dim the tile behind it too', () => {
    const onTile = glass.find((r) => r.selector.startsWith(`${VEIL} :is([data-material="tile"]`) && r.selector.endsWith('[data-material="frost"]:not([data-zen-root] *)'))
    expect(onTile).toBeDefined()
    expect(decl([onTile!], onTile!.selector, '--material-tint')).toBe('rgba(0, 0, 0, 0.05)')
    expect(decl([onTile!], onTile!.selector, '--k2-veil-frost-filter')).toBe('blur(var(--material-blur)) saturate(var(--material-saturate))')
  })

  it('the frost rule sits before the rules that turn the blur off or up (nested frost, menus), so they still win', () => {
    const at = glass.findIndex((r) => r.selector === VEIL_FROST && /backdrop-filter/.test(r.body))
    expect(at).toBeGreaterThan(indexOf(glass, FROST))
    expect(at).toBeLessThan(indexOf(glass, '[data-style="glass"] [data-material="frost"] [data-material="frost"]:not([data-zen-root] *)'))
    expect(at).toBeLessThan(indexOf(glass, '[data-style="glass"] [data-material="frost"][data-float]:not([data-zen-root] *)'))
  })

  it('the merged outline: one even 1px line in the edge colour, the same lift, no light catch, the same frost', () => {
    expect(decl(glass, VEIL_MERGE, '--k2-merge-edge')).toBe('1px')
    expect(decl(glass, VEIL_MERGE, '--k2-merge-shadow-blur')).toBe('var(--k2-veil-lift-blur)')
    expect(decl(glass, VEIL_MERGE, '--k2-merge-shadow-y')).toBe('var(--k2-veil-lift-y)')
    expect(decl(glass, VEIL_MERGE, '--k2-merge-shadow-paint')).toBe('var(--k2-veil-lift-color)')
    expect(decl(glass, VEIL_MERGE, '--k2-merge-sheen-paint')).toBe('transparent')
    expect(glass.filter((r) => r.selector.includes('[data-palette="veil"]') && r.selector.includes('[data-k2-merge="edge"]'))).toEqual([])
    expect(decl(page, VEIL_MERGE_FILL, 'backdrop-filter')).toBe('var(--k2-veil-frost-filter)')
    expect(decl(page, VEIL_MERGE_FILL, '-webkit-backdrop-filter')).toBe('var(--k2-veil-frost-filter)')
  })
})

describe('Obsidian keeps its own glass', () => {
  it('its palette: 10% white frost, a 22% white edge, a 28% light catch', () => {
    expect(obsidian.color['frost-fill'].$value).toBe('rgba(255, 255, 255, 0.10)')
    expect(obsidian.color['frost-edge'].$value).toBe('rgba(255, 255, 255, 0.22)')
    expect(obsidian.color['frost-sheen'].$value).toBe('rgba(255, 255, 255, 0.28)')
  })

  it('the shared Frost rule it draws with is as it was', () => {
    expect(decl(glass, FROST, 'background')).toBe('var(--material-tint)')
    expect(decl(glass, FROST, 'backdrop-filter')).toBe('blur(var(--material-blur)) saturate(var(--material-saturate))')
    expect(decl(glass, FROST, 'box-shadow')).toBe('var(--material-sheen)')
  })

  it('its merged outline: a 0.5px edge, no drop, the light catch, in its own tokens', () => {
    expect(decl(page, '.thread-float-merge', '--k2-merge-edge')).toBe('0.5px')
    expect(decl(page, '.thread-float-merge', '--k2-merge-shadow-blur')).toBe('0px')
    expect(decl(page, '.thread-float-merge', '--k2-merge-shadow-y')).toBe('0px')
    expect(decl(page, '[data-k2-merge="fill"]', 'backdrop-filter')).toBe('blur(var(--material-blur)) saturate(var(--material-saturate))')
    expect(decl(page, '[data-k2-merge="edge"]', 'background')).toBe('var(--material-edge)')
    expect(decl(page, '[data-k2-merge="sheen"]', 'background')).toBe('var(--k2-merge-sheen-paint, var(--color-frost-sheen))')
    expect(decl(page, '[data-k2-merge="shadow"]', 'background')).toBe('var(--k2-merge-shadow-paint, var(--color-shadow-soft))')
  })

  it("Veil's values are set only under Veil", () => {
    for (const r of [...glass, ...page]) {
      if (!/--k2-veil-|--k2-merge-(shadow|sheen)-paint/.test(r.body)) continue
      for (const d of r.body.split(';')) {
        const at = d.indexOf(':')
        if (at < 0 || !/^--k2-(veil-|merge-(shadow|sheen)-paint)/.test(d.slice(0, at).trim())) continue
        expect(r.selector, d.trim()).toContain('[data-palette="veil"]')
      }
    }
  })
})
