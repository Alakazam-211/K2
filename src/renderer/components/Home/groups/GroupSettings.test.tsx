// @vitest-environment jsdom
//
// Home groups: the All groups modal's two panes, with plain props as K2
// Design will use them. The list (select, ⌥↑/⌥↓, drag, Add group) and the
// selected group's settings (name, "Most recent at the top", the whole
// icon set with search and Recent, Delete confirmed once and off on the
// last group). Also Pin / Unpin in a row's menu and the rail's name flash.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import GroupEditModal, { type GroupEditModalProps } from './GroupEditModal'
import { moveId } from './GroupList'
import type { GroupBarItem } from './HomeGroupBar'
import GroupNameFlash, { GROUP_NAME_FLASH_MS } from './GroupNameFlash'
import { pinMenuItem } from '../HomeSidebar'
import { GROUP_ICONS } from '@/lib/group-icons'
import { GROUP_ICON_RECENTS_KEY } from '@/lib/group-icon-recents'
import { MAX_PINS } from '@/lib/home-row-order'
import type { Home } from '@/stores/homes'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: true, pinned: [] },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [] },
  { id: 'g3', name: 'Later', icon: 'rocket', order: 2, recentFirst: false, pinned: [] },
]

function props(over: Partial<GroupEditModalProps> = {}): GroupEditModalProps {
  return {
    groups: G,
    currentId: 'g1',
    initialId: 'g2',
    onRename: vi.fn(),
    onSetIcon: vi.fn(),
    onCreate: vi.fn(() => null),
    onDelete: vi.fn(),
    onSetRecentFirst: vi.fn(),
    onReorderGroups: vi.fn(),
    onClose: vi.fn(),
    ...over,
  }
}

function row(id: string): HTMLElement {
  const el = document.querySelector(`[data-group-list-row="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no list row ${id}`)
  return el
}

function detail(): HTMLElement {
  const el = document.querySelector('[data-group-detail]')
  if (!(el instanceof HTMLElement)) throw new Error('no detail pane')
  return el
}

beforeEach(() => {
  localStorage.clear()
})
afterEach(() => cleanup())

describe('the list pane', () => {
  it('lists every group with its icon or dot, its name and the "Recent" mark; a click selects', () => {
    render(<GroupEditModal {...props()} />)
    expect(Array.from(document.querySelectorAll('[data-group-list-row]')).map((r) => r.getAttribute('data-group-list-row'))).toEqual(['g1', 'g2', 'g3'])
    expect(row('g1').querySelector('svg[data-group-icon="briefcase"]')).not.toBeNull()
    expect(row('g2').querySelector('[data-group-dot="2"]')).not.toBeNull()
    expect(row('g1').querySelector('[data-group-list-recent]')).not.toBeNull()
    expect(row('g3').querySelector('[data-group-list-recent]')).toBeNull()
    expect(detail().getAttribute('data-group-detail')).toBe('g2')
    fireEvent.click(row('g3'))
    expect(detail().getAttribute('data-group-detail')).toBe('g3')
  })

  it('⌥↑/⌥↓ move the focused group; plain arrows move the selection', () => {
    const p = props()
    render(<GroupEditModal {...p} />)
    fireEvent.keyDown(row('g1'), { key: 'ArrowDown', altKey: true })
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g2', 'g1', 'g3'])
    fireEvent.keyDown(row('g3'), { key: 'ArrowUp', altKey: true })
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g1', 'g3', 'g2'])
    fireEvent.keyDown(row('g1'), { key: 'ArrowUp', altKey: true })
    expect(p.onReorderGroups).toHaveBeenCalledTimes(2)
    fireEvent.keyDown(row('g2'), { key: 'ArrowDown' })
    expect(detail().getAttribute('data-group-detail')).toBe('g3')
  })

  it('a drag drops a group onto another’s place', () => {
    const p = props()
    render(<GroupEditModal {...p} />)
    fireEvent.dragStart(row('g3'))
    fireEvent.dragOver(row('g1'))
    fireEvent.drop(row('g1'))
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g3', 'g1', 'g2'])
    expect(moveId(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a'])
  })
})

describe('the settings pane', () => {
  it('shows the whole icon set: search, Recent, "No icon" and every curated icon', () => {
    localStorage.setItem(GROUP_ICON_RECENTS_KEY, JSON.stringify(['rocket', 'house']))
    const p = props()
    render(<GroupEditModal {...p} />)
    const grid = detail().querySelector('[data-group-icon-grid]') as HTMLElement
    expect(grid.className).not.toMatch(/max-h-/)
    expect(grid.querySelectorAll('[data-group-icon-pick]').length).toBe(GROUP_ICONS.length + 1)
    expect(Array.from(detail().querySelectorAll('[data-group-icon-recent] [data-group-icon-pick]')).map((b) => b.getAttribute('data-group-icon-pick'))).toEqual(['rocket', 'house'])
    fireEvent.change(detail().querySelector('[data-group-icon-search]') as HTMLInputElement, { target: { value: 'rock' } })
    fireEvent.click(detail().querySelector('[data-group-icon-grid] [data-group-icon-pick="rocket"]') as HTMLElement)
    expect(p.onSetIcon).toHaveBeenLastCalledWith('g2', 'rocket')
    fireEvent.change(detail().querySelector('[data-group-icon-search]') as HTMLInputElement, { target: { value: '' } })
    fireEvent.click(within(detail()).getByRole('button', { name: 'No icon (numbered dot)' }))
    expect(p.onSetIcon).toHaveBeenLastCalledWith('g2', null)
  })

  it('the name saves as you type (an empty one too: the placeholder says what), the switch is the app’s Toggle', () => {
    const p = props({ emptyNameFallback: () => 'Group 4', initialId: 'g1' })
    render(<GroupEditModal {...p} />)
    const name = detail().querySelector('[data-group-name-input]') as HTMLInputElement
    expect(name.placeholder).toBe('Group 4')
    fireEvent.change(name, { target: { value: 'Work 2' } })
    expect(p.onRename).toHaveBeenLastCalledWith('g1', 'Work 2')
    fireEvent.change(name, { target: { value: '' } })
    expect(p.onRename).toHaveBeenLastCalledWith('g1', '')
    const sw = detail().querySelector('[data-group-recent-first="g1"]') as HTMLElement
    expect([sw.getAttribute('role'), sw.getAttribute('aria-checked'), sw.className.includes('k2-switch')]).toEqual(['switch', 'true', true])
    fireEvent.click(sw)
    expect(p.onSetRecentFirst).toHaveBeenLastCalledWith('g1', false)
  })

  it('Delete asks once, then deletes; it is off on the last group', () => {
    const p = props()
    const { unmount } = render(<GroupEditModal {...p} />)
    fireEvent.click(detail().querySelector('[data-group-delete]') as HTMLElement)
    expect(p.onDelete).not.toHaveBeenCalled()
    fireEvent.click(detail().querySelector('[data-group-delete-yes]') as HTMLElement)
    expect(p.onDelete).toHaveBeenCalledWith('g2')
    unmount()
    render(<GroupEditModal {...props({ groups: [G[1]] })} />)
    const del = detail().querySelector('[data-group-delete]') as HTMLButtonElement
    expect([del.disabled, del.title]).toEqual([true, 'The last group can’t be deleted'])
  })

  it('a deleted selection falls back to the first group; the × closes', () => {
    const p = props()
    const { rerender } = render(<GroupEditModal {...p} />)
    rerender(<GroupEditModal {...p} groups={[G[0], G[2]]} />)
    expect(detail().getAttribute('data-group-detail')).toBe('g1')
    fireEvent.click(screen.getAllByRole('button', { name: /close/i })[0])
    expect(p.onClose).toHaveBeenCalled()
  })
})

describe('Pin / Unpin in a row’s menu', () => {
  function home(pins: number, extra = 2): Home {
    const rows = Array.from({ length: pins + extra }, (_, i) => ({
      address: `a${i}::s`,
      workspaceId: null,
      label: `a${i}`,
      ...(i < pins ? { pinOrder: `${i + 1}` } : {}),
    }))
    return { id: 'h', name: 'Work', rows }
  }

  it('offers Pin with its ⌘ number, Unpin on a pinned row, and the tenth pin off with the reason', () => {
    expect(pinMenuItem(home(2), home(2).rows[3])).toEqual({ id: 'home-pin', label: 'Pin', shortcut: '⌘3' })
    expect(pinMenuItem(home(2), home(2).rows[0])).toEqual({ id: 'home-unpin', label: 'Unpin' })
    const full = home(MAX_PINS)
    expect(pinMenuItem(full, full.rows[MAX_PINS])).toEqual({ id: 'home-pin', label: 'Pin', enabled: false, hint: '9 pinned already' })
  })
})

describe('the rail’s name flash', () => {
  it('shows the group’s name for a moment after a switch, not on first paint', () => {
    vi.useFakeTimers()
    try {
      const { rerender } = render(<GroupNameFlash id="g1" name="Work" />)
      expect(document.querySelector('[data-group-name-flash]')).toBeNull()
      rerender(<GroupNameFlash id="g2" name="Play" />)
      const flag = document.querySelector('[data-group-name-flash="g2"]') as HTMLElement
      expect([flag.textContent, flag.getAttribute('role')]).toEqual(['Play', 'status'])
      act(() => {
        vi.advanceTimersByTime(GROUP_NAME_FLASH_MS + 1)
      })
      expect(document.querySelector('[data-group-name-flash]')).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })
})
