// @vitest-environment jsdom
// Glass's two materials stay apart (prd-themes-terminal-glass-v1 §3.2): Tile
// comes from <Surface material>, Frost only from data-material="frost", and
// neither reaches a Garden or changes Terminal.

import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Surface } from './Surface'

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')
const overrides = readFileSync(join(repo, 'styles', 'glass-next', 'overrides.css'), 'utf8')
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

describe('glass-next overrides', () => {
  it('draws a backdrop blur only for data-material="frost"', () => {
    const frosted = rules.filter((r) => /backdrop-filter/.test(r.body))
    expect(frosted.length).toBeGreaterThan(0)
    for (const r of frosted) expect(r.selector).toContain('[data-material="frost"]')
  })

  it('keeps every rule out of Gardens', () => {
    for (const r of rules.filter((x) => x.selector !== '[data-style="glass-next"]')) {
      expect(r.selector, r.selector).toMatch(/:not\(\[data-zen-root\] \*(, \[data-k2-thread-zen\] \*)?\)/)
    }
  })

  it('sets the accent in exactly one place', () => {
    expect(overrides.match(/--glass-accent:/g)).toHaveLength(1)
  })
})
