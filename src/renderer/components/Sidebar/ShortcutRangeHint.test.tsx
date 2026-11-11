// @vitest-environment jsdom
//
// The nav's section shortcut hint (`⌥⌘ 1-9` beside "Pinned 3" and "Active"):
// the muted text colour at full strength, never faded, so it clears 4.5:1 on
// the nav's grounds in the four shipped looks (at half opacity it measured
// about 2.0–2.7:1). One component for both headers.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ShortcutRangeHint } from './ShortcutRangeHint'

const repo = join(__dirname, '..', '..', '..', '..')
const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

function hex(style: string, id: string, slot: string): [number, number, number] {
  const p = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8')).color
  const v = String(p[slot].$value)
  const m = /^#([0-9a-f]{6})$/i.exec(v)
  if (!m) throw new Error(`${id} ${slot} is not a hex colour: ${v}`)
  return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) as [number, number, number]
}
const lum = (c: [number, number, number]): number => {
  const [r, g, b] = c.map((v) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}
const contrast = (a: [number, number, number], b: [number, number, number]): number => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p)
  return (x + 0.05) / (y + 0.05)
}

afterEach(() => cleanup())

describe('ShortcutRangeHint', () => {
  it('is the muted text colour, never faded', () => {
    const { container } = render(<ShortcutRangeHint combo="⌥⌘ 1-9" />)
    const el = container.querySelector('[data-shortcut-hint]') as HTMLElement
    expect(el.getAttribute('data-shortcut-hint')).toBe('⌥⌘ 1-9')
    expect(el.className).toContain('text-[var(--color-text-muted)]')
    expect(el.className).not.toMatch(/\bopacity-/)
  })

  it('both headers draw it, and nothing else in the nav fades it', () => {
    const sidebar = readFileSync(join(__dirname, 'Sidebar.tsx'), 'utf8')
    const active = readFileSync(join(__dirname, 'ActiveBar.tsx'), 'utf8')
    expect(sidebar).toContain('<ShortcutRangeHint combo=')
    expect(active).toContain('<ShortcutRangeHint combo=')
    expect(active).not.toMatch(/text-\[var\(--color-text-muted\)\] opacity-50/)
  })

  it.each(LOOKS)('the muted text clears 4.5:1 on the nav’s grounds (%s %s)', (style, id) => {
    for (const ground of ['bg', 'bg-surface', 'bg-canvas']) {
      expect(contrast(hex(style, id, 'text-muted'), hex(style, id, ground)), `${id} on ${ground}`).toBeGreaterThanOrEqual(4.5)
    }
  })
})
