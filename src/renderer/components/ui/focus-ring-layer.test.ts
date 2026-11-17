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

// A plain k2-focus button (no cut of its own, no data-k2-focus ring) in
// Terminal, and any k2-focus control inside the desktop Thread box (Home
// chat's hover row, reaction chips), used to fall through to the browser's
// own ring, which is rounded. Each look draws its ring instead: a solid
// outline, square in Terminal (its radius tokens are 0) and round in Glass
// (the control's own pill radius).
describe('k2-focus rings never fall back to the browser ring', () => {
  const decl = (r: Rule, prop: string): string | undefined =>
    (r.nodes.find((d) => d.type === 'decl' && d.prop === prop) as postcss.Declaration | undefined)?.value

  it('Terminal rings a plain k2-focus button with a solid outline', () => {
    const r = rules(globals, (x) => x.selector.startsWith('html[data-style="terminal"] button.k2-focus') && /:focus-visible/.test(x.selector))
    expect(r.length).toBe(1)
    const sel = r[0].selector
    // Cut controls, data-k2-focus rows and glyph buttons draw their own ring.
    expect(sel).toContain(':not([data-k2-chamfer], [data-k2-focus], [data-k2-glyph-button])')
    expect(sel).toContain(':not([data-zen-root] *, [data-k2-thread-zen] *)')
    expect(decl(r[0], 'outline')).toBe('2px solid var(--color-accent-text)')
    expect(layersOf(r[0])).toEqual([])
  })

  it('the desktop Thread box rings its k2-focus controls in both looks, never in a Garden', () => {
    const r = rules(globals, (x) => x.selector.startsWith('[data-k2-thread-zen] .k2-focus') && /:focus-visible/.test(x.selector))
    expect(r.length).toBe(1)
    expect(r[0].selector).toContain(':not([data-zen-root] *)')
    expect(r[0].selector).not.toMatch(/data-style/)
    expect(decl(r[0], 'outline')).toBe('2px solid var(--zen-focus-ring)')
    expect(layersOf(r[0])).toEqual([])
  })

  it('Terminal radius tokens are square, so those outlines are square there', () => {
    const tokens = JSON.parse(readFileSync(join(renderer, '..', '..', 'styles', 'terminal', 'tokens.json'), 'utf8')) as {
      radius: Record<string, { $value: string }>
    }
    for (const k of ['field', 'selector', 'pill']) expect(tokens.radius[k].$value, k).toBe('0px')
  })

  it("Home chat's hover row, reaction chips and Call button opt into k2-focus", () => {
    const dir = join(renderer, 'components', 'HomeChat')
    const row = readFileSync(join(dir, 'HomeChatMessageRow.tsx'), 'utf8')
    expect(row).toMatch(/ACTION_BUTTON_CLASS =\s*'k2-focus /)
    const chips = readFileSync(join(dir, 'HomeChatReactions.tsx'), 'utf8')
    expect(chips).toMatch(/data-testid="home-chat-reaction"[\s\S]{0,120}className="k2-focus /)
    const call = readFileSync(join(dir, 'HomeChatCall.tsx'), 'utf8')
    expect(call).toMatch(/'k2-focus /)
  })
})
