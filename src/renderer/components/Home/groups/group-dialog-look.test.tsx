// @vitest-environment jsdom
//
// Home groups: the Groups dialog's look rules (K2 Design's mock): K2's
// dialog shape, at most 720 × 540 kept 16 px from the window's sides and
// 24 px from its top and bottom, one pane below 640 px, Done and no ×.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { type GroupBarItem } from './HomeGroupBar'
import GroupEditModal, { GROUP_DIALOG_STYLE } from './GroupEditModal'
import { groupDialogSize } from './group-bar-look'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [], count: 4 },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [], count: 2 },
  { id: 'g3', name: 'Later', icon: 'music', order: 2, recentFirst: false, pinned: [], count: 0 },
]

afterEach(() => cleanup())

describe('the Groups dialog', () => {
  it('is at most 720 × 540, 16 px from the sides and 24 px from the top and bottom; one pane under 640 px', () => {
    expect(groupDialogSize(1440, 900)).toEqual({ width: 720, height: 540, onePane: false })
    expect(groupDialogSize(1100, 720)).toEqual({ width: 720, height: 540, onePane: false })
    expect(groupDialogSize(800, 600)).toEqual({ width: 720, height: 540, onePane: false })
    expect(groupDialogSize(1280, 560)).toEqual({ width: 720, height: 512, onePane: false })
    expect(groupDialogSize(640, 560)).toEqual({ width: 608, height: 512, onePane: true })
    expect(groupDialogSize(672, 600).onePane).toBe(false)
    expect(groupDialogSize(671, 600).onePane).toBe(true)
    expect(GROUP_DIALOG_STYLE.width).toBe('min(720px, calc(var(--k2-vw) - 32px))')
    expect(GROUP_DIALOG_STYLE.height).toBe('min(540px, calc(var(--k2-vh) - 48px))')
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
})
