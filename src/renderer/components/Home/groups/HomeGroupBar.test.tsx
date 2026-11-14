// @vitest-environment jsdom
//
// Home groups: the group icon row. The bar alone with plain props (as K2
// Design will use it), then wired to the Homes store (`useHomeGroups`):
// the header's "…", rename and icon saved live, an emptied name.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { type GroupBarItem, type HomeGroupBarProps } from './HomeGroupBar'
import GroupNameHeader from './GroupNameHeader'
import { GROUP_BAR_ICON_PX } from './GroupGlyph'
import { useHomeGroups } from './useHomeGroups'
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
    swipe: null,
    ...over,
  }
}

function groupButton(name: string): HTMLElement {
  return within(screen.getByRole('toolbar', { name: 'Groups' })).getByRole('button', { name })
}

function modal(): HTMLElement {
  const el = document.querySelector('[data-group-edit-modal]')
  if (!(el instanceof HTMLElement)) throw new Error('no All groups modal')
  return el
}

beforeEach(() => {
  localStorage.clear()
  sessionStorage.clear()
})
afterEach(() => cleanup())

describe('the group icon row', () => {
  it('bare icons: the current group in the accent colour, the others muted; each a labelled, focusable button', () => {
    render(<HomeGroupBar {...props({ currentId: 'g2' })} />)
    const play = groupButton('Play')
    expect(play.getAttribute('aria-current')).toBe('true')
    expect(play.className).toContain('text-[var(--color-accent)]')
    for (const other of ['Work', 'Later']) {
      const b = groupButton(other)
      expect(b.getAttribute('aria-current')).toBeNull()
      expect(b.className).toContain('text-[var(--color-text-muted)]')
      expect(b.tagName).toBe('BUTTON')
      // The app's focus ring; no cell or bubble behind the glyph.
      expect(b.hasAttribute('data-k2-focus')).toBe(true)
      expect(b.className).not.toMatch(/\bbg-|rounded/)
    }
    // No "+" in the row.
    expect(screen.queryByRole('button', { name: 'New group' })).toBeNull()
  })

  it('draws a Lucide icon squared at half size, in a larger click target; a numbered dot when it has none or an unknown name', () => {
    render(<HomeGroupBar {...props()} />)
    const work = groupButton('Work')
    const svg = work.querySelector('svg[data-group-icon]')
    if (!svg) throw new Error('no icon on Work')
    expect(svg.getAttribute('class')).toContain('lucide-briefcase')
    expect([svg.getAttribute('width'), svg.getAttribute('stroke-width'), svg.getAttribute('stroke-linecap'), svg.getAttribute('stroke-linejoin'), svg.getAttribute('stroke')]).toEqual([
      String(GROUP_BAR_ICON_PX),
      '1.75',
      'square',
      'miter',
      'currentColor',
    ])
    expect(GROUP_BAR_ICON_PX).toBe(8)
    expect(work.className).toContain('h-6 w-6')
    expect(groupButton('Play').querySelector('[data-group-dot]')?.textContent).toBe('2')
    expect(groupButton('Play').querySelector('svg')).toBeNull()
    expect(groupButton('Later').querySelector('[data-group-dot]')?.textContent).toBe('3')
  })

  it('a click switches; the tooltip names the ⌥⌘ chord and "Most recent at the top"', () => {
    const p = props({ shortcutLabel: (i) => `⌥⌘${i + 1}` })
    render(<HomeGroupBar {...p} />)
    fireEvent.click(groupButton('Play'))
    expect(p.onSelect).toHaveBeenCalledWith('g2')
    expect(groupButton('Later').getAttribute('title')).toBe('Later (⌥⌘3)')
    const work = groupButton('Work')
    expect(work.getAttribute('title')).toBe('Work (⌥⌘1) · Most recent at the top')
    expect(work.getAttribute('aria-description')).toBe('Most recent at the top')
    expect(work.querySelector('[data-recent-first-mark]')).not.toBeNull()
    expect(groupButton('Play').querySelector('[data-recent-first-mark]')).toBeNull()
  })

  it('arrow keys move focus along the row', () => {
    render(<HomeGroupBar {...props()} />)
    groupButton('Work').focus()
    fireEvent.keyDown(groupButton('Work'), { key: 'ArrowRight' })
    expect(document.activeElement).toBe(groupButton('Play'))
    fireEvent.keyDown(groupButton('Play'), { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(groupButton('Work'))
  })

  it('carries swipe progress for the visual to follow, armed past 35%', () => {
    const { rerender } = render(<HomeGroupBar {...props({ swipe: { progress: 0.2, direction: 'prev' } })} />)
    const bar = screen.getByRole('toolbar', { name: 'Groups' })
    expect(bar.style.getPropertyValue('--group-swipe-progress')).toBe('0.2')
    expect([bar.getAttribute('data-swipe-direction'), bar.getAttribute('data-swipe-armed')]).toEqual(['prev', null])
    rerender(<HomeGroupBar {...props({ swipe: { progress: 0.4, direction: 'next' } })} />)
    expect([bar.getAttribute('data-swipe-direction'), bar.getAttribute('data-swipe-armed')]).toEqual(['next', ''])
    rerender(<HomeGroupBar {...props({ swipe: null })} />)
    expect([bar.getAttribute('data-swipe-direction'), bar.style.getPropertyValue('--group-swipe-progress')]).toEqual(['none', '0'])
  })

  it('with many groups the row scrolls and brings the current group into view', () => {
    const many = Array.from({ length: 14 }, (_, i) => ({ id: `m${i}`, name: `G${i}`, icon: null, order: i, recentFirst: false, pinned: [] }))
    const seen: string[] = []
    // jsdom has no scrollIntoView: record which element asked to be shown.
    const proto = HTMLElement.prototype as unknown as { scrollIntoView?: (o?: unknown) => void }
    expect(proto.scrollIntoView).toBeUndefined()
    proto.scrollIntoView = function (this: HTMLElement) {
      seen.push(this.getAttribute('data-group-id') ?? '')
    }
    try {
      const { rerender } = render(<HomeGroupBar {...props({ groups: many, currentId: 'm0' })} />)
      const scroller = document.querySelector('[data-group-bar-scroller]') as HTMLElement
      expect(scroller.className).toContain('overflow-x-auto')
      rerender(<HomeGroupBar {...props({ groups: many, currentId: 'm12' })} />)
      expect(seen.at(-1)).toBe('m12')
    } finally {
      delete proto.scrollIntoView
    }
  })
})

describe('opening the All groups modal', () => {
  it('a right-click opens it centred on the window, outside the row, with that group selected; Escape closes it', () => {
    render(<HomeGroupBar {...props()} />)
    fireEvent.contextMenu(groupButton('Play'))
    const m = modal()
    expect(screen.getByRole('toolbar', { name: 'Groups' }).contains(m)).toBe(false)
    expect(m.getAttribute('role')).toBe('dialog')
    expect(m.closest('[data-dialog-placement="center"]')).not.toBeNull()
    expect(m.querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe('g2')
    expect(m.querySelector('[data-group-list-row="g2"]')?.getAttribute('aria-selected')).toBe('true')
    act(() => {
      fireEvent.keyDown(document, { key: 'Escape' })
    })
    expect(document.querySelector('[data-group-edit-modal]')).toBeNull()
  })

  it('a long press on touch opens it; a short tap does not', () => {
    vi.useFakeTimers()
    try {
      render(<HomeGroupBar {...props()} />)
      fireEvent.pointerDown(groupButton('Later'), { pointerType: 'touch' })
      fireEvent.pointerUp(groupButton('Later'), { pointerType: 'touch' })
      act(() => {
        vi.advanceTimersByTime(800)
      })
      expect(document.querySelector('[data-group-edit-modal]')).toBeNull()
      fireEvent.pointerDown(groupButton('Later'), { pointerType: 'touch' })
      act(() => {
        vi.advanceTimersByTime(600)
      })
      expect(modal().querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe('g3')
    } finally {
      vi.useRealTimers()
    }
  })

  it('the context-menu key on a focused icon opens it', () => {
    render(<HomeGroupBar {...props()} />)
    fireEvent.keyDown(groupButton('Work'), { key: 'ContextMenu' })
    expect(modal().querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe('g1')
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

describe('the group row on the Homes store', () => {
  it('the header’s "…" opens the modal on the current group; Add group makes the next "Group N", selected, its name selected', () => {
    setHomeNames(['Home', 'Group 1', 'group 3'])
    act(() => useHomesStore.getState().selectHome(useHomesStore.getState().homes[1].id))
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Group 1)' }))
    expect(modal().querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe(useHomesStore.getState().homes[1].id)
    fireEvent.click(modal().querySelector('[data-group-create]') as HTMLElement)
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Home', 'Group 1', 'group 3', 'Group 2'])
    const added = useHomesStore.getState().homes[3]
    expect(modal().querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe(added.id)
    const input = modal().querySelector('[data-group-name-input]') as HTMLInputElement
    expect(input.value).toBe('Group 2')
    expect(document.activeElement).toBe(input)
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 'Group 2'.length])
    fireEvent.change(input, { target: { value: 'Sales' } })
    expect(useHomesStore.getState().homes[3].name).toBe('Sales')
  })

  it('an emptied name saves as the next unused "Group N"; an icon pick lands in the store and the row', () => {
    setHomeNames(['Group 1', 'Work', 'Group 3'])
    render(<Wired />)
    fireEvent.contextMenu(groupButton('Work'))
    const input = modal().querySelector('[data-group-name-input]') as HTMLInputElement
    expect(input.placeholder).toBe('Group 2')
    fireEvent.change(input, { target: { value: '' } })
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Group 1', 'Group 2', 'Group 3'])
    fireEvent.change(modal().querySelector('[data-group-icon-search]') as HTMLInputElement, { target: { value: 'gamepad' } })
    fireEvent.click(modal().querySelector('[data-group-icon-pick="gamepad-2"]') as HTMLElement)
    expect(useHomesStore.getState().homes[1].icon).toBe('gamepad-2')
    expect(groupButton('Group 2').querySelector('svg')?.getAttribute('class')).toContain('lucide-gamepad-2')
    // A click on a group in the row still switches.
    fireEvent.click(groupButton('Group 3'))
    expect(selectedHome(useHomesStore.getState()).name).toBe('Group 3')
  })
})
