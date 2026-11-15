// @vitest-environment jsdom
//
// Home groups: the bar's look rules (K2 Design's mock and Rosson's calls),
// pinned so a later change has to mean it: the glyph-size token (14 px,
// Rosson), the space between glyphs (the footer's own Add Agent | « gap),
// the hit targets (touching, never overlapping, never under 20 px), the
// focus ring and the edge fades.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { cleanup, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { type GroupBarItem, type HomeGroupBarProps } from './HomeGroupBar'
import { barFades } from './group-bar-scroll'
import {
  GROUP_BAR_GAP_PX,
  GROUP_BAR_ICON_PX,
  GROUP_BAR_TARGET_GAP_PX,
  GROUP_BAR_TARGET_MIN_PX,
  GROUP_BAR_TARGET_PX,
  GROUP_RAIL_MAX_PX,
  GROUP_RAIL_VISIBLE_MAX,
  groupGlyphStroke,
  groupNumeralPx,
} from './group-bar-look'
import { NAV_COLLAPSE_GAP_PX } from '@/components/Sidebar/SidebarCollapseButton'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [], count: 4 },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [], count: 2 },
  { id: 'g3', name: 'Later', icon: 'music', order: 2, recentFirst: false, pinned: [], count: 0 },
]

function props(over: Partial<HomeGroupBarProps> = {}): HomeGroupBarProps {
  return {
    groups: G,
    currentId: 'g1',
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onSetIcon: vi.fn(),
    onCreate: vi.fn(() => null),
    onDelete: vi.fn(),
    onSetRecentFirst: vi.fn(),
    onReorderGroups: vi.fn(),
    swipe: null,
    ...over,
  }
}

afterEach(() => cleanup())

describe('the glyph-size token', () => {
  it('is one constant, 14 px (Rosson: 12 px, then 15% bigger), drawn at the app’s stroke; 8 px keeps a 1 px line', () => {
    expect(GROUP_BAR_ICON_PX).toBe(14)
    expect(groupGlyphStroke(14)).toBe(K2_ICON_STROKE)
    expect(groupGlyphStroke(12)).toBe(K2_ICON_STROKE)
    expect(groupGlyphStroke(8)).toBe(3) // 3 / 24 × 8 = a 1 px line
    expect(groupNumeralPx(14)).toBe(12)
    expect(groupNumeralPx(12)).toBe(11)
    expect(groupNumeralPx(8)).toBe(9)
  })

  it('reaches every glyph and the root’s CSS tokens', () => {
    render(<HomeGroupBar {...props()} />)
    const bar = screen.getByRole('toolbar', { name: 'Groups' })
    expect(bar.style.getPropertyValue('--k2-group-icon')).toBe(`${GROUP_BAR_ICON_PX}px`)
    expect(bar.style.getPropertyValue('--k2-group-gap')).toBe(`${GROUP_BAR_GAP_PX}px`)
    expect(bar.style.getPropertyValue('--k2-group-target')).toBe(`${GROUP_BAR_TARGET_PX}px`)
    const svg = bar.querySelector('svg[data-group-icon="briefcase"]') as SVGElement
    expect([svg.getAttribute('width'), svg.getAttribute('stroke-width')]).toEqual([String(GROUP_BAR_ICON_PX), String(K2_ICON_STROKE)])
    const n = bar.querySelector('[data-group-numeral]') as HTMLElement
    expect([n.textContent, n.style.fontSize, n.className.includes('font-bold')]).toEqual(['2', '12px', true])
  })
})

describe('spacing and hit targets', () => {
  it('glyphs sit the footer’s Add Agent | « gap apart, in targets that touch and never overlap', () => {
    expect(GROUP_BAR_GAP_PX).toBe(NAV_COLLAPSE_GAP_PX)
    expect(GROUP_BAR_TARGET_PX).toBeGreaterThanOrEqual(GROUP_BAR_TARGET_MIN_PX)
    expect(GROUP_BAR_TARGET_MIN_PX).toBe(20)
    expect(GROUP_BAR_TARGET_GAP_PX).toBeGreaterThanOrEqual(0)
    // Glyph to glyph: target − glyph + the space between targets.
    expect(GROUP_BAR_TARGET_PX - GROUP_BAR_ICON_PX + GROUP_BAR_TARGET_GAP_PX).toBe(NAV_COLLAPSE_GAP_PX)
    expect([GROUP_BAR_TARGET_PX, GROUP_BAR_TARGET_GAP_PX]).toEqual([22, 0])
  })

  it('every target is a square button of the target token with the app’s focus ring; no fill', () => {
    render(<HomeGroupBar {...props()} />)
    for (const b of within(screen.getByRole('toolbar', { name: 'Groups' })).getAllByRole('button')) {
      expect(b.className).toContain('h-[var(--k2-group-target)] w-[var(--k2-group-target)]')
      expect(b.hasAttribute('data-k2-focus')).toBe(true)
      expect(b.className).not.toMatch(/\bbg-|rounded|\bborder\b/)
    }
    const scroller = document.querySelector('[data-group-bar-scroller]') as HTMLElement
    expect(scroller.style.gap).toBe(`${GROUP_BAR_TARGET_GAP_PX}px`)
  })

  it('the rail stack shows five, then scrolls', () => {
    expect(GROUP_RAIL_VISIBLE_MAX).toBe(5)
    expect(GROUP_RAIL_MAX_PX).toBe(5 * GROUP_BAR_TARGET_PX + 4 * GROUP_BAR_TARGET_GAP_PX)
    render(<HomeGroupBar {...props({ orientation: 'vertical' })} />)
    const scroller = document.querySelector('[data-group-bar-scroller]') as HTMLElement
    expect(scroller.style.maxHeight).toBe(`${GROUP_RAIL_MAX_PX}px`)
    expect(scroller.className).toContain('overflow-y-auto')
  })

  it('edge fades show only where more groups sit past the edge', () => {
    expect(barFades(0, 160, 160)).toEqual({ start: false, end: false })
    expect(barFades(0, 160, 280)).toEqual({ start: false, end: true })
    expect(barFades(60, 160, 280)).toEqual({ start: true, end: true })
    expect(barFades(120, 160, 280)).toEqual({ start: true, end: false })
  })
})

describe('the focus ring and the look’s CSS', () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'home-groups.css'), 'utf8')

  it('is a circle outside Terminal (Terminal keeps the app’s cut ring)', () => {
    expect(css).toMatch(/html:not\(\[data-style="terminal"\]\) \[data-home-group-bar\] \[data-group-id\]\[data-k2-focus\]:focus-visible::after \{\s*border-radius: 50%;/)
    expect(css).not.toMatch(/html\[data-style="terminal"\][^{]*data-group-id[^{]*focus/)
  })

  it('uses tokens only: no raw colours in the look’s CSS', () => {
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i)
  })
})
