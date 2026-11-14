// The shared keyboard-focus ring (data-k2-focus) gives its element a
// positioned box, so the ring's ::after rings that element and not the
// nearest positioned ancestor. That default must lose to an element's own
// Tailwind position class (absolute, fixed, sticky): unlayered CSS beats
// every cascade layer whatever its specificity, so when the default was
// unlayered it pulled absolute boxes back into the flow (the search field's
// clear button, Get started's Skip). The default lives in a layer under
// Tailwind's; the ring itself stays unlayered, so no Tailwind class can turn
// it off. The real engine check is focus-ring-layer.wktest.ts.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import postcss, { type AtRule, type Container, type Rule } from 'postcss'
import { describe, expect, it } from 'vitest'

const renderer = join(__dirname, '..', '..')
const globals = postcss.parse(readFileSync(join(renderer, 'globals.css'), 'utf8'))
const generated = postcss.parse(readFileSync(join(renderer, 'styles.generated.css'), 'utf8'))

/** The cascade layers a rule sits in, outermost first ([] = unlayered). */
function layersOf(node: Rule): string[] {
  const out: string[] = []
  for (let p: Container | undefined = node.parent as Container | undefined; p && p.type !== 'root'; p = p.parent as Container | undefined) {
    if (p.type === 'atrule' && (p as AtRule).name === 'layer') out.unshift((p as AtRule).params.trim())
  }
  return out
}

function rules(root: postcss.Root, test: (r: Rule) => boolean): Rule[] {
  const out: Rule[] = []
  root.walkRules((r) => {
    if (test(r)) out.push(r)
  })
  return out
}

const onFocusBox = (r: Rule): boolean => /\[data-k2-focus\]/.test(r.selector) && !/::?(after|before)\b/.test(r.selector)
const sets = (r: Rule, prop: string): boolean => r.nodes.some((d) => d.type === 'decl' && d.prop === prop)

/** The layer order statement (`@layer a, b, c;`). */
function layerOrder(): string[] {
  let order: string[] = []
  globals.walkAtRules('layer', (a) => {
    if (!a.nodes && a.params.includes(',')) order = a.params.split(',').map((s) => s.trim())
  })
  return order
}

describe('focus ring box: Tailwind position classes win', () => {
  it('the focus-base layer is declared under Tailwind\'s layer', () => {
    const order = layerOrder()
    expect(order).toContain('k2-focus-base')
    expect(order).toContain('tw')
    expect(order.indexOf('k2-focus-base')).toBeLessThan(order.indexOf('tw'))
  })

  it('Tailwind is imported into that tw layer', () => {
    const imp = globals.nodes.find((n) => n.type === 'atrule' && n.name === 'import' && /"tailwindcss"/.test(n.params)) as AtRule | undefined
    expect(imp?.params).toMatch(/layer\(tw\)/)
  })

  it('the positioned box is set in the focus-base layer, with zero specificity', () => {
    const box = rules(globals, (r) => onFocusBox(r) && sets(r, 'position'))
    expect(box.length).toBeGreaterThan(0)
    for (const r of box) {
      expect(layersOf(r), r.selector).toEqual(['k2-focus-base'])
      expect(r.selector.startsWith(':where('), r.selector).toBe(true)
      expect(r.nodes.find((d) => d.type === 'decl' && d.prop === 'position')?.toString()).toBe('position: relative')
    }
  })

  it('no unlayered rule sets the position of a data-k2-focus box (it would beat absolute, fixed and sticky)', () => {
    for (const root of [globals, generated]) {
      for (const r of rules(root, (x) => onFocusBox(x) && sets(x, 'position'))) {
        expect(layersOf(r), r.selector).not.toEqual([])
      }
    }
  })

  it('no :not(.absolute)-style exceptions: the layer does the work', () => {
    for (const r of rules(globals, onFocusBox)) {
      expect(r.selector).not.toMatch(/absolute|fixed|sticky/)
    }
  })
})

describe('the ring itself stays unlayered', () => {
  it('the ring (::after) and the outline it replaces are top level, so no Tailwind class can override them', () => {
    const ring = rules(globals, (r) => /\[data-k2-focus\]:focus-visible/.test(r.selector))
    // The base ring, the outline reset, Terminal's cut ring and the menu row shapes.
    expect(ring.length).toBeGreaterThanOrEqual(4)
    for (const r of ring) expect(layersOf(r), r.selector).toEqual([])
    const after = ring.find((r) => r.selector.startsWith('[data-k2-focus]:focus-visible') && r.selector.endsWith('::after'))
    expect(after?.toString()).toContain('position: absolute')
    expect(after?.toString()).toContain('box-shadow: inset 0 0 0 2px var(--color-accent-text)')
    const reset = ring.find((r) => sets(r, 'outline'))
    expect(reset?.toString()).toContain('outline: none')
  })
})

describe('the elements this frees', () => {
  it('the search field\'s clear button lays itself over the field (absolute + data-k2-focus)', () => {
    const src = readFileSync(join(renderer, 'components', 'ui', 'SearchField.tsx'), 'utf8')
    const at = src.indexOf('data-k2-search-clear')
    const tag = src.slice(src.lastIndexOf('<button', at), src.indexOf('>', at))
    expect(tag).toContain('data-k2-focus=""')
    expect(tag).toMatch(/className="absolute right-1\.5 /)
  })
})
