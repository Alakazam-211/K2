// Glass Veil keeps every Frost object (and the Thread's round Send) a
// positioned box, so what's placed inside it doesn't move. That default must lose to the element's own Tailwind position class
// (absolute, fixed, sticky) and to any unlayered rule that places it:
// unlayered CSS beats every cascade layer whatever its specificity, so an
// unlayered `position: relative` pulled such boxes back into the flow (a
// sticky Frost bar, an editor popover placed by its own stylesheet). The
// default lives in a layer under Tailwind's.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import postcss, { type AtRule, type Container, type Rule } from 'postcss'
import { describe, expect, it } from 'vitest'

const renderer = join(__dirname, '..', '..')
const repo = join(renderer, '..', '..')
const globals = postcss.parse(readFileSync(join(renderer, 'globals.css'), 'utf8'))
const generated = postcss.parse(readFileSync(join(renderer, 'styles.generated.css'), 'utf8'))
const glass = postcss.parse(readFileSync(join(repo, 'styles', 'glass', 'overrides.css'), 'utf8'))

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

const decl = (r: Rule, prop: string): string | undefined => {
  const d = r.nodes.find((n) => n.type === 'decl' && n.prop === prop)
  return d && d.type === 'decl' ? d.value : undefined
}
/** A rule that positions a Frost box itself (not a ::before). */
const onFrostBox = (r: Rule): boolean =>
  /\[data-material="frost"\]/.test(r.selector) && !/::?(after|before)\b/.test(r.selector) && decl(r, 'position') !== undefined

function layerOrder(): string[] {
  let order: string[] = []
  globals.walkAtRules('layer', (a) => {
    if (!a.nodes && a.params.includes(',')) order = a.params.split(',').map((s) => s.trim())
  })
  return order
}

describe('Veil Frost box: Tailwind position classes win', () => {
  it('the material-base layer is declared under Tailwind\'s layer, beside the other base layers', () => {
    const order = layerOrder()
    expect(order).toContain('k2-material-base')
    expect(order.indexOf('k2-material-base')).toBeLessThan(order.indexOf('tw'))
    expect(order.indexOf('k2-material-base')).toBeGreaterThan(order.indexOf('k2-focus-base'))
  })

  it('the positioned box is set in the material-base layer, for Frost and the Thread\'s Send', () => {
    const box = rules(glass, (r) => /\[data-palette="veil"\]/.test(r.selector) && onFrostBox(r))
    expect(box.length).toBe(1)
    const [r] = box
    expect(layersOf(r)).toEqual(['k2-material-base'])
    expect(decl(r, 'position')).toBe('relative')
    expect(r.selector).toContain('[data-zen-send]')
  })

  it('no unlayered rule sets the position of a Frost box (in the source or the built CSS)', () => {
    for (const root of [glass, generated]) {
      for (const r of rules(root, onFrostBox)) expect(layersOf(r), r.selector).not.toEqual([])
    }
  })

  it('no :not(.absolute)-style exceptions: the layer does the work', () => {
    for (const r of rules(glass, onFrostBox)) expect(r.selector).not.toMatch(/absolute|fixed|sticky/)
  })

  it('the built CSS carries the same layered rule', () => {
    const built = rules(generated, (r) => /\[data-palette="veil"\]/.test(r.selector) && onFrostBox(r))
    expect(built.map((r) => layersOf(r))).toEqual([['k2-material-base']])
  })
})
