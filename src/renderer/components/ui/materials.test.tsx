// @vitest-environment jsdom
// Glass's two materials stay apart (prd-themes-terminal-glass-v1 §3.2): Tile
// comes from <Surface material>, Frost only from data-material="frost", and
// neither reaches a Garden or changes Terminal.

import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Surface } from './Surface'

vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => undefined }))

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')
const overrides = readFileSync(join(repo, 'styles', 'glass', 'overrides.css'), 'utf8')
const rules = Array.from(overrides.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]*)\}/g), (m) => ({
  selector: m[1].trim(),
  body: m[2],
}))

describe('Surface material', () => {
  it('marks a tile with data-material', () => {
    const { container } = render(<Surface material="tile" />)
    expect(container.firstElementChild?.getAttribute('data-material')).toBe('tile')
  })

  it('has no material by default, and never offers frost', () => {
    const { container } = render(<Surface />)
    expect(container.firstElementChild?.hasAttribute('data-material')).toBe(false)
    // @ts-expect-error Frost is not a Surface option.
    render(<Surface material="frost" />)
  })

  it('marks an elevated surface as floating so a tile keeps its dialog shadow', () => {
    const { container } = render(<Surface material="tile" elevation={5} />)
    expect(container.firstElementChild?.hasAttribute('data-floating')).toBe(true)
  })
})

describe('Glass overrides', () => {
  it('draws a backdrop blur only for data-material="frost"', () => {
    const frosted = rules.filter((r) => /backdrop-filter/.test(r.body))
    expect(frosted.length).toBeGreaterThan(0)
    for (const r of frosted) expect(r.selector).toContain('[data-material="frost"]')
  })

  it('keeps every rule out of Gardens', () => {
    // A block on the root element only sets variables; the Zen shield resets those.
    const rootBlock = /^\[data-style="glass"\](\[data-palette="\w+"\])?$/
    for (const r of rules.filter((x) => !rootBlock.test(x.selector))) {
      expect(r.selector, r.selector).toMatch(/:not\(\[data-zen-root\] \*(, \[data-k2-thread-zen\] \*)?\)/)
    }
  })

  it('sets the accent in exactly one place', () => {
    expect(overrides.match(/--glass-accent:/g)).toHaveLength(1)
  })
})

describe('Floating chrome is Frost', () => {
  it('the row "…" menu is a frosted floating menu', async () => {
    const { OverflowMenu } = await import('./OverflowMenu')
    const { getByRole } = render(<OverflowMenu label="More" items={[{ id: 'a', label: 'A', onSelect: () => {} }]} />)
    fireEvent.click(getByRole('button', { name: 'More' }))
    const menu = getByRole('menu')
    expect(menu.getAttribute('data-material')).toBe('frost')
    expect(menu.getAttribute('data-float')).toBe('menu')
    expect(menu.hasAttribute('data-floating')).toBe(false)
  })

  it('a frosted menu is solid enough that rows under it never read as words, in both modes, and keeps its rim', () => {
    const alpha = (body: string): number => Number(/background: rgba\(\d+, \d+, \d+, ([\d.]+)\)/.exec(body)![1])
    const dark = rules.find((r) => r.selector === '[data-style="glass"] [data-material="frost"][data-float]:not([data-zen-root] *)')!
    const light = rules.find((r) => r.selector === '[data-style="glass"][data-palette="veil"] [data-material="frost"][data-float]:not([data-zen-root] *)')!
    expect(alpha(dark.body)).toBeGreaterThanOrEqual(0.97)
    expect(alpha(light.body)).toBeGreaterThanOrEqual(0.97)
    // A heavier blur than the 28px on the rest of Frost.
    expect(dark.body).toContain('backdrop-filter: blur(48px) saturate(180%)')
    expect(dark.body).toContain('box-shadow: var(--material-sheen), var(--shadow-3)')
  })

  it('a search field is a frosted field; a plain field is not', async () => {
    const { Input } = await import('./Input')
    const { getByRole } = render(
      <>
        <Input variant="search" aria-label="Find" />
        <Input aria-label="Name" />
      </>,
    )
    const search = getByRole('searchbox', { name: 'Find' })
    expect(search.getAttribute('data-material')).toBe('frost')
    expect(search.getAttribute('data-frost-shape')).toBe('field')
    // Not the `bg-` class the "field in a tile" rule turns solid.
    expect(search.className).not.toContain('bg-[var(--color-bg)]')
    expect(getByRole('textbox', { name: 'Name' }).hasAttribute('data-material')).toBe(false)
    // A search heads a list: the search height (a token, density-scaled), not a row field's.
    expect(search.className).toContain('h-[calc(var(--control-search-height)*var(--density-scale,1))]')
    expect(getByRole('textbox', { name: 'Name' }).className).not.toContain('--control-search-height')
  })

  it('a frosted search field keeps the control edge in Glass', () => {
    const globals = readFileSync(join(repo, 'src', 'renderer', 'globals.css'), 'utf8')
    expect(globals).toMatch(
      /input\[data-k2-search\]\[data-material="frost"\]:not\(:focus\):not\(\[data-zen-root\] \*\) \{\s*border: 1px solid var\(--color-control-edge\);/,
    )
  })
})
