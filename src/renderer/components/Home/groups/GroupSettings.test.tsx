// @vitest-environment jsdom
//
// Home groups, the second step: the bar's "Recent first" mark, the edit
// card's Group settings view (every group: icon, name, "Most recent at the
// top", order by drag or ⌥↑/⌥↓), and Pin / Unpin in a row's menu with the
// tenth pin offered but off.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { type GroupBarItem, type HomeGroupBarProps } from './HomeGroupBar'
import { moveId } from './GroupSettingsList'
import { pinMenuItem } from '../HomeSidebar'
import { SWIPE_REST } from '@/lib/group-swipe'
import { MAX_PINS } from '@/lib/home-row-order'
import type { Home } from '@/stores/homes'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: true, pinned: [] },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [] },
  { id: 'g3', name: 'Later', icon: 'rocket', order: 2, recentFirst: false, pinned: [] },
]

function props(over: Partial<HomeGroupBarProps> = {}): HomeGroupBarProps {
  return {
    groups: G,
    currentId: 'g2',
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onSetIcon: vi.fn(),
    onCreate: vi.fn(() => null),
    onDelete: vi.fn(),
    onSetRecentFirst: vi.fn(),
    onReorderGroups: vi.fn(),
    swipe: SWIPE_REST,
    ...over,
  }
}

function bar(): HTMLElement {
  return screen.getByRole('toolbar', { name: 'Groups' })
}

function openSettings(): HTMLElement {
  fireEvent.contextMenu(within(bar()).getByRole('button', { name: 'Play' }))
  const card = document.querySelector('[data-group-edit-card]') as HTMLElement
  fireEvent.click(card.querySelector('[data-group-settings-open]') as HTMLElement)
  const list = card.querySelector('[data-group-settings]')
  if (!(list instanceof HTMLElement)) throw new Error('no Group settings view')
  return list
}

function settingsRow(list: HTMLElement, id: string): HTMLElement {
  const el = list.querySelector(`[data-group-settings-row="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no settings row ${id}`)
  return el
}

beforeEach(() => {
  localStorage.clear()
})
afterEach(() => cleanup())

describe('the "Recent first" mark', () => {
  it('marks only a group whose switch is on, in its tooltip and for a screen reader', () => {
    render(<HomeGroupBar {...props()} />)
    const work = within(bar()).getByRole('button', { name: 'Work' })
    expect(work.querySelector('[data-recent-first-mark]')).not.toBeNull()
    expect(work.getAttribute('title')).toContain('Most recent at the top')
    expect(work.getAttribute('aria-description')).toBe('Most recent at the top')
    const play = within(bar()).getByRole('button', { name: 'Play' })
    expect(play.querySelector('[data-recent-first-mark]')).toBeNull()
    expect(play.getAttribute('aria-description')).toBeNull()
  })
})

describe('Group settings in the edit card', () => {
  it('lists every group with its icon or dot, its name and its switch', () => {
    render(<HomeGroupBar {...props()} />)
    const list = openSettings()
    expect(Array.from(list.querySelectorAll('[data-group-settings-row]')).map((r) => r.getAttribute('data-group-settings-row'))).toEqual(['g1', 'g2', 'g3'])
    expect(settingsRow(list, 'g1').querySelector('svg[data-group-icon="briefcase"]')).not.toBeNull()
    expect(settingsRow(list, 'g2').querySelector('[data-group-dot="2"]')).not.toBeNull()
    expect((settingsRow(list, 'g1').querySelector('[data-group-recent-first]') as HTMLInputElement).checked).toBe(true)
    expect((settingsRow(list, 'g3').querySelector('[data-group-recent-first]') as HTMLInputElement).checked).toBe(false)
  })

  it('the switch, a rename and ⌥↑/⌥↓ go out through the callbacks', () => {
    const p = props()
    render(<HomeGroupBar {...p} />)
    const list = openSettings()
    fireEvent.click(settingsRow(list, 'g3').querySelector('[data-group-recent-first]') as HTMLElement)
    expect(p.onSetRecentFirst).toHaveBeenLastCalledWith('g3', true)
    fireEvent.change(settingsRow(list, 'g2').querySelector('[data-group-settings-name]') as HTMLElement, { target: { value: 'Fun' } })
    expect(p.onRename).toHaveBeenLastCalledWith('g2', 'Fun')
    fireEvent.keyDown(settingsRow(list, 'g1'), { key: 'ArrowDown', altKey: true })
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g2', 'g1', 'g3'])
    fireEvent.keyDown(settingsRow(list, 'g3'), { key: 'ArrowUp', altKey: true })
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g1', 'g3', 'g2'])
    // Without ⌥, or past either end, nothing moves.
    fireEvent.keyDown(settingsRow(list, 'g1'), { key: 'ArrowDown' })
    fireEvent.keyDown(settingsRow(list, 'g1'), { key: 'ArrowUp', altKey: true })
    expect(p.onReorderGroups).toHaveBeenCalledTimes(2)
  })

  it('a drag drops a group onto another’s place', () => {
    const p = props()
    render(<HomeGroupBar {...p} />)
    const list = openSettings()
    fireEvent.dragStart(settingsRow(list, 'g3'))
    fireEvent.dragOver(settingsRow(list, 'g1'))
    fireEvent.drop(settingsRow(list, 'g1'))
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g3', 'g1', 'g2'])
    expect(moveId(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a'])
  })

  it('a group’s icon opens that group’s own card; Back returns', () => {
    const p = props()
    const { rerender } = render(<HomeGroupBar {...p} />)
    const list = openSettings()
    fireEvent.click(within(settingsRow(list, 'g3')).getByRole('button', { name: 'Icon and more for Later' }))
    rerender(<HomeGroupBar {...p} />)
    expect(document.querySelector('[data-group-edit-card]')?.getAttribute('data-group-edit-card')).toBe('g3')
    expect(document.querySelector('[data-group-settings]')).toBeNull()
    const card = document.querySelector('[data-group-edit-card]') as HTMLElement
    fireEvent.click(card.querySelector('[data-group-settings-open]') as HTMLElement)
    fireEvent.click(card.querySelector('[data-group-settings-back]') as HTMLElement)
    expect(card.querySelector('[data-group-name-input]')).not.toBeNull()
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
