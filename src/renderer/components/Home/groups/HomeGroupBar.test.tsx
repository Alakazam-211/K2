// @vitest-environment jsdom
//
// Home groups: the bottom group bar and its edit card. The bar alone with
// plain props (as K2 Design will use it), then wired to the Homes store
// (`useHomeGroups`): "+", the header's "…", rename and icon saved live.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { type GroupBarItem, type HomeGroupBarProps } from './HomeGroupBar'
import GroupNameHeader from './GroupNameHeader'
import { useHomeGroups } from './useHomeGroups'
import { SWIPE_REST } from '@/lib/group-swipe'
import { GROUP_ICON_RECENTS_KEY } from '@/lib/group-icon-recents'
import { selectedHome, useHomesStore } from '@/stores/homes'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: true, pinned: ['a::s'] },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [] },
  { id: 'g3', name: 'Later', icon: 'not-a-lucide-icon', order: 2, recentFirst: false, pinned: [] },
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
    swipe: SWIPE_REST,
    ...over,
  }
}

function groupButton(name: string): HTMLElement {
  return within(screen.getByRole('toolbar', { name: 'Groups' })).getByRole('button', { name })
}

function card(): HTMLElement {
  const el = document.querySelector('[data-group-edit-card]')
  if (!(el instanceof HTMLElement)) throw new Error('no group edit card')
  return el
}

beforeEach(() => {
  localStorage.clear()
  sessionStorage.clear()
})
afterEach(() => cleanup())

describe('the group bar', () => {
  it('paints the current group in the accent colour and the others muted; each is a labelled button', () => {
    render(<HomeGroupBar {...props({ currentId: 'g2' })} />)
    const play = groupButton('Play')
    expect(play.getAttribute('aria-current')).toBe('true')
    expect(play.className).toContain('text-[var(--color-accent)]')
    for (const other of ['Work', 'Later']) {
      const b = groupButton(other)
      expect(b.getAttribute('aria-current')).toBeNull()
      expect(b.className).toContain('text-[var(--color-text-muted)]')
      expect(b.className).not.toContain('text-[var(--color-accent)]')
      expect(b.tagName).toBe('BUTTON')
    }
  })

  it('draws a group’s Lucide icon squared at 16 px, and a numbered dot when it has none or an unknown name', () => {
    render(<HomeGroupBar {...props()} />)
    const svg = groupButton('Work').querySelector('svg')
    if (!svg) throw new Error('no icon on Work')
    expect(svg.getAttribute('class')).toContain('lucide-briefcase')
    expect([svg.getAttribute('width'), svg.getAttribute('stroke-width'), svg.getAttribute('stroke-linecap'), svg.getAttribute('stroke-linejoin'), svg.getAttribute('stroke')]).toEqual([
      '16',
      '1.75',
      'square',
      'miter',
      'currentColor',
    ])
    expect(groupButton('Play').querySelector('[data-group-dot]')?.textContent).toBe('2')
    expect(groupButton('Play').querySelector('svg')).toBeNull()
    expect(groupButton('Later').querySelector('[data-group-dot]')?.textContent).toBe('3')
  })

  it('a click switches; the tooltip names the ⌥⌘ chord when given', () => {
    const p = props({ shortcutLabel: (i) => `⌥⌘${i + 1}` })
    render(<HomeGroupBar {...p} />)
    fireEvent.click(groupButton('Play'))
    expect(p.onSelect).toHaveBeenCalledWith('g2')
    expect(groupButton('Later').getAttribute('title')).toBe('Later (⌥⌘3)')
  })

  it('arrow keys move focus along the bar', () => {
    render(<HomeGroupBar {...props()} />)
    groupButton('Work').focus()
    fireEvent.keyDown(groupButton('Work'), { key: 'ArrowRight' })
    expect(document.activeElement).toBe(groupButton('Play'))
    fireEvent.keyDown(groupButton('Play'), { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(groupButton('Work'))
  })

  it('carries swipe progress for the visual to follow', () => {
    render(<HomeGroupBar {...props({ swipe: { progress: 0.4, direction: -1 } })} />)
    const bar = screen.getByRole('toolbar', { name: 'Groups' })
    expect(bar.style.getPropertyValue('--group-swipe-progress')).toBe('0.4')
    expect(bar.getAttribute('data-swipe-direction')).toBe('-1')
  })

  it('with many groups the bar scrolls and brings the current group into view', () => {
    const many = Array.from({ length: 14 }, (_, i) => ({ id: `m${i}`, name: `G${i}`, icon: null, order: i, recentFirst: false, pinned: [] }))
    const seen: string[] = []
    // jsdom has no scrollIntoView: record which element asked to be shown.
    const proto = HTMLElement.prototype as unknown as { scrollIntoView?: (o?: unknown) => void }
    expect(proto.scrollIntoView).toBeUndefined()
    proto.scrollIntoView = function (this: HTMLElement) {
      seen.push(this.getAttribute('data-group-id') ?? '')
    }
    const spy = { mockRestore: () => delete proto.scrollIntoView }
    try {
      const { rerender } = render(<HomeGroupBar {...props({ groups: many, currentId: 'm0' })} />)
      const scroller = document.querySelector('[data-group-bar-scroller]') as HTMLElement
      expect(scroller.className).toContain('overflow-x-auto')
      rerender(<HomeGroupBar {...props({ groups: many, currentId: 'm12' })} />)
      expect(seen.at(-1)).toBe('m12')
    } finally {
      spy.mockRestore()
    }
  })
})

describe('the group edit card', () => {
  it('right-click opens it, outside the bar; Escape and an outside click close it', () => {
    render(<HomeGroupBar {...props()} />)
    fireEvent.contextMenu(groupButton('Play'))
    const c = card()
    expect(c.getAttribute('data-group-edit-card')).toBe('g2')
    expect(screen.getByRole('toolbar', { name: 'Groups' }).contains(c)).toBe(false)
    expect((within(c).getByDisplayValue('Play') as HTMLInputElement).value).toBe('Play')
    fireEvent.keyDown(c, { key: 'Escape' })
    expect(document.querySelector('[data-group-edit-card]')).toBeNull()

    fireEvent.contextMenu(groupButton('Work'))
    expect(card().getAttribute('data-group-edit-card')).toBe('g1')
    act(() => {
      fireEvent.mouseDown(document.body)
    })
    expect(document.querySelector('[data-group-edit-card]')).toBeNull()
  })

  it('a long press on touch opens it; a short tap does not', () => {
    vi.useFakeTimers()
    try {
      render(<HomeGroupBar {...props()} />)
      fireEvent.pointerDown(groupButton('Play'), { pointerType: 'touch' })
      fireEvent.pointerUp(groupButton('Play'), { pointerType: 'touch' })
      act(() => {
        vi.advanceTimersByTime(800)
      })
      expect(document.querySelector('[data-group-edit-card]')).toBeNull()
      fireEvent.pointerDown(groupButton('Play'), { pointerType: 'touch' })
      act(() => {
        vi.advanceTimersByTime(600)
      })
      expect(card().getAttribute('data-group-edit-card')).toBe('g2')
    } finally {
      vi.useRealTimers()
    }
  })

  it('saves the name as you type (never an empty one) and the icon on a pick', () => {
    const p = props()
    render(<HomeGroupBar {...p} />)
    fireEvent.contextMenu(groupButton('Work'))
    const name = card().querySelector('[data-group-name-input]') as HTMLInputElement
    fireEvent.change(name, { target: { value: 'Work 2' } })
    expect(p.onRename).toHaveBeenLastCalledWith('g1', 'Work 2')
    fireEvent.change(name, { target: { value: '   ' } })
    expect(p.onRename).toHaveBeenCalledTimes(1)

    const search = card().querySelector('[data-group-icon-search]') as HTMLInputElement
    fireEvent.change(search, { target: { value: 'rock' } })
    const grid = card().querySelector('[data-group-icon-grid]') as HTMLElement
    const rocket = grid.querySelector('[data-group-icon-pick="rocket"]') as HTMLElement
    expect(rocket).not.toBeNull()
    fireEvent.click(rocket)
    expect(p.onSetIcon).toHaveBeenLastCalledWith('g1', 'rocket')
    // The pick joins this computer's Recent row.
    expect(JSON.parse(localStorage.getItem(GROUP_ICON_RECENTS_KEY) ?? 'null')).toEqual(['rocket'])
    fireEvent.change(search, { target: { value: '' } })
    expect(card().querySelector('[data-group-icon-recent] [data-group-icon-pick="rocket"]')).not.toBeNull()
    // "No icon" clears it.
    fireEvent.click(within(card()).getByRole('button', { name: 'No icon (numbered dot)' }))
    expect(p.onSetIcon).toHaveBeenLastCalledWith('g1', null)
  })

  it('Delete asks once, then deletes; it is off on the last group', () => {
    const p = props()
    const { unmount } = render(<HomeGroupBar {...p} />)
    fireEvent.contextMenu(groupButton('Later'))
    fireEvent.click(card().querySelector('[data-group-delete]') as HTMLElement)
    expect(p.onDelete).not.toHaveBeenCalled()
    fireEvent.click(card().querySelector('[data-group-delete-yes]') as HTMLElement)
    expect(p.onDelete).toHaveBeenCalledWith('g3')
    expect(document.querySelector('[data-group-edit-card]')).toBeNull()
    unmount()

    const one = props({ groups: [G[0]] })
    render(<HomeGroupBar {...one} />)
    fireEvent.contextMenu(groupButton('Work'))
    const del = card().querySelector('[data-group-delete]') as HTMLButtonElement
    expect(del.disabled).toBe(true)
    expect(del.title).toBe('The last group can’t be deleted')
  })
})

function Wired(): React.JSX.Element {
  const g = useHomeGroups()
  return (
    <div>
      <GroupNameHeader name={g.current.name} icon={g.current.icon} index={g.current.index} count={g.current.count} onEdit={g.editCurrent} />
      <HomeGroupBar {...g.bar} />
    </div>
  )
}

/** Leave exactly these Homes, in order (the store refuses to drop the last). */
function setHomeNames(names: string[]): void {
  const st = useHomesStore.getState()
  const before = st.homes.map((h) => h.id)
  for (const n of names) if (st.createHome(n) === null) throw new Error(`createHome refused ${n}`)
  for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error(`deleteHome refused ${id}`)
  expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(names)
}

describe('the group bar on the Homes store', () => {
  it('"+" makes the next free "Group N", selects it and opens its card with the name selected', () => {
    setHomeNames(['Home', 'Group 1', 'group 3'])
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'New group' }))
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Home', 'Group 1', 'group 3', 'Group 2'])
    expect(selectedHome(useHomesStore.getState()).name).toBe('Group 2')
    const input = card().querySelector('[data-group-name-input]') as HTMLInputElement
    expect(input.value).toBe('Group 2')
    expect(document.activeElement).toBe(input)
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 'Group 2'.length])
    // Typing renames it live.
    fireEvent.change(input, { target: { value: 'Sales' } })
    expect(selectedHome(useHomesStore.getState()).name).toBe('Sales')
  })

  it('the header shows the current group and its "…" opens the same card; an icon pick lands in the store', () => {
    setHomeNames(['Work', 'Play'])
    act(() => useHomesStore.getState().selectHome(useHomesStore.getState().homes[1].id))
    render(<Wired />)
    const header = document.querySelector('[data-group-name-header]') as HTMLElement
    expect(header.textContent).toContain('Play')
    fireEvent.click(screen.getByRole('button', { name: 'Edit group Play' }))
    const play = useHomesStore.getState().homes[1]
    expect(card().getAttribute('data-group-edit-card')).toBe(play.id)
    fireEvent.change(card().querySelector('[data-group-icon-search]') as HTMLInputElement, { target: { value: 'gamepad' } })
    fireEvent.click(card().querySelector('[data-group-icon-pick="gamepad-2"]') as HTMLElement)
    expect(useHomesStore.getState().homes[1].icon).toBe('gamepad-2')
    // The bar and the header both draw it now.
    expect(groupButton('Play').querySelector('svg')?.getAttribute('class')).toContain('lucide-gamepad-2')
    expect(header.querySelector('svg')?.getAttribute('class')).toContain('lucide-gamepad-2')
    // A click on the other group switches.
    fireEvent.click(groupButton('Work'))
    expect(selectedHome(useHomesStore.getState()).name).toBe('Work')
  })
})
