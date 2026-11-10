// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Tag } from '@/components/ui'

const globals = readFileSync(join(__dirname, '..', '..', 'globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')

/** The body of the first rule whose selector is exactly `sel`. */
function rule(sel: string): string {
  const at = globals.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return globals.slice(at, globals.indexOf('}', at))
}

describe('Tag', () => {
  it('is a chip cut in Terminal, with its tone and the badge type role', () => {
    render(<Tag tone="accent" className="ml-auto" title="t">beta</Tag>)
    const tag = screen.getByText('beta')
    expect(tag.tagName).toBe('SPAN')
    expect(tag.getAttribute('data-k2-chamfer')).toBe('chip')
    expect(tag.getAttribute('data-tone')).toBe('accent')
    expect(tag.className).toBe('k2-tag type-badge ml-auto')
    expect(tag.getAttribute('title')).toBe('t')
  })

  it('defaults to the neutral tone', () => {
    render(<Tag>label</Tag>)
    expect(screen.getByText('label').getAttribute('data-tone')).toBe('neutral')
  })

  it('reads the pill radius, so Glass draws a pill and Terminal (0) the cut', () => {
    expect(rule('.k2-tag')).toContain('border-radius: var(--radius-pill)')
    expect(rule('.k2-tag')).toContain('white-space: nowrap')
  })

  it('every tone sets an ink and an edge', () => {
    for (const tone of ['accent', 'muted', 'neutral', 'ok', 'warn', 'danger']) {
      const body = rule(`.k2-tag[data-tone="${tone}"]`)
      expect(body).toContain('--k2-tag-ink:')
      expect(body).toContain('--k2-tag-edge:')
    }
  })

  it('the quiet tone has no fill: a wash would take muted text under 4.5:1', () => {
    expect(rule('.k2-tag[data-tone="muted"]')).not.toContain('--k2-tag-fill')
  })
})
