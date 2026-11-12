// Selected text in a Terminal field reads at 4.5:1: the field's ::selection
// is the accent button's pair (accent fill under on-accent text), whose
// contrast control-contrast.test pins in every palette. The browser's own
// highlight read 3.78:1 in Charcoal.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repo = join(__dirname, '..', '..', '..', '..')
const globals = readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')

describe('selected text in a Terminal field', () => {
  it('uses the accent fill under on-accent text, and leaves Gardens alone', () => {
    const rule = globals.match(/html\[data-style="terminal"\] :is\(input, textarea[^{]*::selection[^{]*\{([^}]*)\}/)
    expect(rule, 'the Terminal field ::selection rule').not.toBeNull()
    const body = rule?.[1] ?? ''
    expect(body).toMatch(/background:\s*var\(--color-accent-fill\)/)
    expect(body).toMatch(/color:\s*var\(--color-on-accent\)/)
    expect(rule?.[0]).toContain(':not([data-zen-root] *)')
  })
})
