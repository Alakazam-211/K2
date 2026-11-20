// @vitest-environment jsdom
// The Home nav's footer: the ⌘K field and + (Add to Home). In Glass they are
// Frost, the same glass as the top bar's capsules and the nav's round
// buttons: the frost fill, blur and sheen and the frost edge, in every Glass
// palette (Veil's light-glass recipe comes with the material), and no
// control outline. Terminal is not touched, and only the footer row changes:
// every other search field and quiet button keeps its own look.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { HomeAddButton, HomeNavFooterRow } from './HomeNavFooter'

const repo = join(__dirname, '..', '..', '..', '..')
const css = (): string =>
  readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')

/** Every rule whose selector names the footer row, as [selector, body]. */
function footerRules(text: string): Array<[string, string]> {
  const out: Array<[string, string]> = []
  const re = /([^{}]*\[data-home-footer-row\][^{}]*)\{([^}]*)\}/g
  for (let m = re.exec(text); m; m = re.exec(text)) out.push([m[1].trim(), m[2]])
  return out
}

const PLUS = 'button[data-home-add-open][data-home-add-open][data-material="frost"]'

afterEach(cleanup)

describe('the Home nav footer in Glass', () => {
  it('the field and the footer + are Frost; the empty Home + is not', () => {
    const { container } = render(<HomeNavFooterRow />)
    const row = container.querySelector('[data-home-footer-row]')!
    expect(row.querySelector('input[data-k2-search]')!.getAttribute('data-material')).toBe('frost')
    const plus = row.querySelector('button[data-home-add-open="footer"]')!
    expect(plus.getAttribute('data-material')).toBe('frost')
    expect(plus.getAttribute('data-frost-shape')).toBe('round')
    cleanup()
    const empty = render(<HomeAddButton place="empty" />).container.querySelector('button')!
    expect(empty.hasAttribute('data-material')).toBe(false)
  })

  it('both take the frost edge at rest, not the control edge', () => {
    const rules = footerRules(css())
    const edge = rules.find(([sel]) =>
      sel.startsWith('html[data-style="glass"] [data-home-footer-row] :is(input[data-k2-search]:not(:focus),'),
    )
    expect(edge).toBeDefined()
    expect(edge![0]).toContain('button[data-home-add-open][data-home-add-open])[data-material="frost"]')
    expect(edge![1]).toContain('border: 0.5px solid var(--material-edge)')
    for (const [, body] of rules) expect(body).not.toContain('--color-control-edge')
  })

  it("+ takes the frost fill (the raised tile's fill is a bordered button's)", () => {
    const rules = footerRules(css())
    const rest = rules.find(([sel]) => sel === `html[data-style="glass"] [data-home-footer-row] ${PLUS}:not([data-zen-root] *)`)
    expect(rest?.[1]).toContain('background: var(--material-tint)')
    const hover = rules.find(([sel]) => sel.includes(`${PLUS}:hover`))
    expect(hover?.[1]).toContain('background: var(--k2-float-fill-hover)')
    expect(hover?.[1]).toContain('border-color: var(--material-edge)')
  })

  it('every footer rule is Glass only, the same in every Glass palette', () => {
    const rules = footerRules(css())
    expect(rules.length).toBeGreaterThan(0)
    for (const [sel] of rules) {
      expect(sel.startsWith('html[data-style="glass"]')).toBe(true)
      expect(sel).not.toContain('data-scheme')
      expect(sel).not.toContain('data-palette')
    }
  })
})
