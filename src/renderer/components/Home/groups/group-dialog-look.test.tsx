// @vitest-environment jsdom
//
// Home groups: the Groups dialog's look rules (K2 Design's mock): K2's
// dialog shape, at most 720 × 540 kept 16 px from the window's sides and
// 24 px from its top and bottom, one pane below 640 px, Done and no ×.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { DIALOG_EDGE_TINT } from '@/components/ui/ScrollEdgeFade'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { type GroupBarItem } from './HomeGroupBar'
import { NavPaletteField } from '../HomeNavFooter'
import GroupEditModal, { GROUP_DIALOG_STYLE } from './GroupEditModal'
import { GROUP_DIALOG_FOOT_CLASS, GROUP_DIALOG_FOOT_PX, GROUP_DIALOG_HEIGHT_SHARE, groupDialogSize } from './group-bar-look'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [], count: 4 },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [], count: 2 },
  { id: 'g3', name: 'Later', icon: 'music', order: 2, recentFirst: false, pinned: [], count: 0 },
]

afterEach(() => cleanup())

describe('the Groups dialog', () => {
  it('is up to 880 wide and 80% of the window high, 16 px from the sides; one pane under 640 px', () => {
    expect(groupDialogSize(1440, 900)).toEqual({ width: 880, height: 720, onePane: false })
    expect(groupDialogSize(1100, 720)).toEqual({ width: 880, height: 576, onePane: false })
    expect(groupDialogSize(800, 600)).toEqual({ width: 768, height: 480, onePane: false })
    expect(groupDialogSize(1280, 560)).toEqual({ width: 880, height: 448, onePane: false })
    expect(groupDialogSize(640, 560)).toEqual({ width: 608, height: 448, onePane: true })
    expect(groupDialogSize(672, 600).onePane).toBe(false)
    expect(groupDialogSize(671, 600).onePane).toBe(true)
    expect(GROUP_DIALOG_HEIGHT_SHARE).toBe(0.8)
    expect(GROUP_DIALOG_STYLE.width).toBe('min(880px, calc(var(--k2-vw) - 32px))')
    expect(GROUP_DIALOG_STYLE.height).toBe('calc(var(--k2-vh) * 0.8)')
  })

  it('is K2’s dialog shape with a Done button and no ×, its list 220 px', () => {
    render(<GroupEditModal groups={G} currentId="g1" initialId="g2" onRename={vi.fn()} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} onClose={vi.fn()} />)
    const m = document.querySelector('[data-group-edit-modal]') as HTMLElement
    expect(m.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(m.closest('[data-k2-chamfer-float]')).not.toBeNull()
    expect(screen.getByRole('button', { name: 'Done' })).toBeTruthy()
    expect(screen.queryByTestId('dialog-close')).toBeNull()
    const list = m.querySelector('[data-group-list]')?.parentElement as HTMLElement
    expect(list.style.width).toBe('220px')
    // Focus starts in the selected group's name.
    expect(document.activeElement).toBe(m.querySelector('[data-group-name-input]'))
    // Each group's agent count in the list.
    expect(Array.from(m.querySelectorAll('[data-group-list-count]')).map((c) => c.textContent)).toEqual(['4', '2', '0'])
  })

  it('both panes\' footer lines meet: one shared height, the line on top', () => {
    render(<GroupEditModal groups={G} currentId="g1" initialId="g2" onRename={vi.fn()} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} onClose={vi.fn()} />)
    const m = document.querySelector('[data-group-edit-modal]') as HTMLElement
    const list = m.querySelector('[data-group-dialog-foot="list"]') as HTMLElement
    const detail = m.querySelector('[data-group-dialog-foot="detail"]') as HTMLElement
    expect(GROUP_DIALOG_FOOT_PX).toBe(45)
    expect(GROUP_DIALOG_FOOT_CLASS).toContain(`h-[${GROUP_DIALOG_FOOT_PX}px]`)
    for (const foot of [list, detail]) {
      expect(foot.className.startsWith(GROUP_DIALOG_FOOT_CLASS)).toBe(true)
      // Nothing else sets a height or vertical padding on either.
      expect(foot.className.slice(GROUP_DIALOG_FOOT_CLASS.length)).not.toMatch(/(^|\s)(min-h-|h-|py-|pt-|pb-|border)/)
      // Each is the last thing in its pane, so both sit on the dialog's floor.
      expect(foot.nextElementSibling).toBeNull()
    }
  })

  it('the Agents | Icon segments hug their choices, left-aligned', () => {
    render(<GroupEditModal groups={G} currentId="g1" initialId="g2" onRename={vi.fn()} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} onClose={vi.fn()} />)
    const tabs = document.querySelector('[data-group-detail-tabs]') as HTMLElement
    expect(tabs.className).toContain('justify-start')
    const seg = tabs.querySelector('.k2-segmented') as HTMLElement
    expect(seg.className).not.toMatch(/w-full|flex-1/)
  })

  it('the icon grid fades right under its search: the band starts at the field, heavy, in the dialog\'s fill', () => {
    render(<GroupEditModal groups={G} currentId="g1" initialId="g2" startTab="icon" onRename={vi.fn()} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} onClose={vi.fn()} />)
    const check = (testId: string): void => {
      const frame = screen.getByTestId(testId)
      expect(frame.hasAttribute('data-edge-heavy')).toBe(true)
      expect(frame.style.getPropertyValue('--k2-edge-tint')).toBe(DIALOG_EDGE_TINT)
      const header = frame.previousElementSibling as HTMLElement
      expect(header.hasAttribute('data-edge-header')).toBe(true)
      // No room under the field: the frame (and its band) starts at its edge,
      // and the header paints over the band so the field's ring stays whole.
      expect(header.className).not.toMatch(/(^|\s)(pb|py)-/)
      expect(header.className).toContain('z-[2]')
    }
    check('group-icon-frame')
  })
})

// Round mp-4 looks: Terminal's real text field, the cut floats, the dot.
const here = dirname(fileURLToPath(import.meta.url))
const homeCss = readFileSync(join(here, 'home-groups.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

describe('Terminal looks (round mp-4)', () => {
  it('the name and both of the dialog’s searches are K2’s cut Terminal field (the control chamfer)', () => {
    render(<GroupEditModal groups={G} currentId="g1" initialId="g2" onRename={vi.fn(() => 'saved' as const)} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} onClose={vi.fn()} />)
    const m = document.querySelector('[data-group-edit-modal]') as HTMLElement
    const name = m.querySelector('[data-group-name-input]') as HTMLElement
    expect(name.getAttribute('data-k2-chamfer')).toBe('control')
    expect(name.className).toContain('k2-focus')
    const iconSearch = m.querySelector('[data-group-icon-search]') as HTMLElement
    expect([iconSearch.getAttribute('data-k2-chamfer'), iconSearch.hasAttribute('data-k2-search')]).toEqual(['control', true])
    // The Agents search is the same shared SearchField (it needs the Homes
    // store, so it is checked where that is wired).
    expect(readFileSync(join(here, 'GroupAgentsPane.tsx'), 'utf8')).toMatch(/<SearchField\b/)
  })

  it('the nav footer’s ⌘K field is the same cut field', () => {
    render(<NavPaletteField />)
    const field = document.querySelector('[data-home-palette-field] input') as HTMLElement
    expect(field.getAttribute('data-k2-chamfer')).toBe('control')
  })

  it('the bar’s tip is cut (the control chamfer) inside a drop-shadow wrapper', () => {
    render(<HomeGroupBar groups={G} currentId="g1" onSelect={vi.fn()} onRename={vi.fn(() => 'saved' as const)} onSetIcon={vi.fn()} onCreate={vi.fn(() => null)} onDelete={vi.fn()} onSetRecentFirst={vi.fn()} onReorderGroups={vi.fn()} swipe={null} />)
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Play' }))
    const tip = document.querySelector('[data-group-bar-tip]') as HTMLElement
    expect(tip.getAttribute('data-k2-chamfer-float')).toBe('menu')
    const box = tip.querySelector('[data-group-float="tip"]') as HTMLElement
    expect(box.getAttribute('data-k2-chamfer')).toBe('control')
  })

  it('the CSS: a cut float drops its box-shadow; the Home rows’ dot is round in Terminal only, never in a Garden', () => {
    expect(homeCss).toContain('html[data-style="terminal"] [data-group-float][data-k2-chamfer] { box-shadow: none; }')
    expect(homeCss).toContain('html[data-style="terminal"] :is([data-home-sidebar], [data-home-rail]):not([data-zen-root] *) { --k2-live-square-radius: 50%; }')
  })
})
