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
import { barOverflows, centredScroll, pagedScroll, revealScroll } from './group-bar-scroll'
import { useHomeGroups } from './useHomeGroups'
import { HOMES_STORAGE_KEY, selectedHome, useHomesStore } from '@/stores/homes'
import { useProjectsStore } from '@/stores/projects'
import { pinnedRows } from '@/lib/home-row-order'

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
  it('bare icons: the current group in the accent text colour, the others muted; each a labelled, focusable button', () => {
    render(<HomeGroupBar {...props({ currentId: 'g2' })} />)
    const play = groupButton('Play')
    expect(play.getAttribute('aria-current')).toBe('true')
    expect(play.className).toContain('text-[var(--color-accent-text)]')
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

  it('draws a Lucide icon squared at the bar size, in a larger click target; its number when it has none or an unknown name', () => {
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
    expect(GROUP_BAR_ICON_PX).toBe(12)
    expect(work.className).toContain('h-[var(--k2-group-target)] w-[var(--k2-group-target)]')
    expect(groupButton('Play').querySelector('[data-group-dot]')?.textContent).toBe('2')
    expect(groupButton('Play').querySelector('svg')).toBeNull()
    expect(groupButton('Later').querySelector('[data-group-dot]')?.textContent).toBe('3')
  })

  it('a click switches; the name tip shows the ⌥⌘ chord; the label carries the chord and "Most recent at the top"', () => {
    const p = props({ shortcutLabel: (i) => `⌥⌘${i + 1}` })
    render(<HomeGroupBar {...p} />)
    fireEvent.click(groupButton('Play'))
    expect(p.onSelect).toHaveBeenCalledWith('g2')
    // The tip (K2 Design's look) replaces the native title.
    expect(groupButton('Later').getAttribute('title')).toBeNull()
    fireEvent.mouseEnter(groupButton('Later'))
    const tip = document.querySelector('[data-group-bar-tip]') as HTMLElement
    expect(tip.textContent).toBe('Later⌥⌘3')
    expect(tip.getAttribute('aria-hidden')).toBe('true')
    fireEvent.mouseLeave(groupButton('Later'))
    expect(document.querySelector('[data-group-bar-tip]')).toBeNull()
    expect(groupButton('Later').getAttribute('aria-keyshortcuts')).toBe('Alt+Meta+3')
    const work = groupButton('Work')
    expect(work.getAttribute('aria-description')).toBe('Most recent at the top')
  })

  it('is one tab stop, the current group; Home and End reach the ends', () => {
    render(<HomeGroupBar {...props({ currentId: 'g2' })} />)
    expect(['Work', 'Play', 'Later'].map((n) => groupButton(n).tabIndex)).toEqual([-1, 0, -1])
    groupButton('Play').focus()
    fireEvent.keyDown(groupButton('Play'), { key: 'End' })
    expect(document.activeElement).toBe(groupButton('Later'))
    fireEvent.keyDown(groupButton('Later'), { key: 'Home' })
    expect(document.activeElement).toBe(groupButton('Work'))
    // Moving focus doesn't move the tab stop: it stays on the current group.
    expect(groupButton('Work').tabIndex).toBe(-1)
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

  it('with many groups the row scrolls and centres the current group (its own scroller only)', () => {
    const many = Array.from({ length: 14 }, (_, i) => ({ id: `m${i}`, name: `G${i}`, icon: null, order: i, recentFirst: false, pinned: [] }))
    render(<HomeGroupBar {...props({ groups: many, currentId: 'm12' })} />)
    const scroller = document.querySelector('[data-group-bar-scroller]') as HTMLElement
    expect(scroller.className).toContain('overflow-x-auto')
    // 14 targets of 20 px in a 160 px view: the 13th centred, then clamped at the ends.
    expect(centredScroll(12 * 20, 20, 160, 14 * 20)).toBe(120)
    expect(centredScroll(0, 20, 160, 14 * 20)).toBe(0)
    expect(centredScroll(13 * 20, 20, 160, 14 * 20)).toBe(120)
    expect(centredScroll(40, 20, 160, 100)).toBe(0)
  })
})

// Overflow (round mp-4): chevrons at the bar's ends while the groups don't
// fit, each while more groups sit that way; they page the bar and are out of
// the tab order. jsdom has no layout, so the bar's sizes are stubbed: 12
// groups of 20 px (240) in a 200 px bar whose scroller shows 160.
describe('the bar with more groups than fit', () => {
  const twelve = Array.from({ length: 12 }, (_, i) => ({ id: `t${i}`, name: `T${i}`, icon: null, order: i, recentFirst: false, pinned: [] }))
  const stubLayout = (barPx: number): void => {
    const size = (el: HTMLElement, axis: 'w' | 'scroll'): number | undefined => {
      if (el.hasAttribute('data-group-bar-scroller')) return axis === 'scroll' ? 240 : Math.min(240, barPx - 40)
      if (el.hasAttribute('data-home-group-bar')) return barPx
      return undefined
    }
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return size(this, 'w') ?? 0
    })
    vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return size(this, 'scroll') ?? 0
    })
    vi.spyOn(HTMLElement.prototype, 'offsetLeft', 'get').mockImplementation(function (this: HTMLElement) {
      const id = this.dataset.groupId
      return id ? Number(id.slice(1)) * 20 : 0
    })
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.groupId ? 20 : 0
    })
  }
  afterEach(() => vi.restoreAllMocks())

  const bar = (): HTMLElement => screen.getByRole('toolbar', { name: 'Groups' })
  const chevron = (end: 'start' | 'end'): HTMLElement => bar().querySelector(`[data-group-bar-chevron="${end}"]`) as HTMLElement
  const scroller = (): HTMLElement => bar().querySelector('[data-group-bar-scroller]') as HTMLElement

  it('the sums: overflow against the whole bar, a page is the visible extent, focus reveals', () => {
    expect(barOverflows(240, 200)).toBe(true)
    expect(barOverflows(240, 240)).toBe(false)
    expect(pagedScroll(0, 160, 240, 1)).toBe(80)
    expect(pagedScroll(80, 160, 240, -1)).toBe(0)
    expect(revealScroll(200, 20, 0, 160)).toBe(60)
    expect(revealScroll(20, 20, 60, 160)).toBe(20)
    expect(revealScroll(100, 20, 60, 160)).toBe(60)
  })

  it('shows a chevron only towards more groups; a click pages; neither is a tab stop', () => {
    stubLayout(200)
    render(<HomeGroupBar {...props({ groups: twelve, currentId: 't0' })} />)
    expect(bar().hasAttribute('data-overflow')).toBe(true)
    expect([bar().hasAttribute('data-more-start'), bar().hasAttribute('data-more-end')]).toEqual([false, true])
    for (const end of ['start', 'end'] as const) {
      expect(chevron(end).tabIndex).toBe(-1)
      expect(chevron(end).getAttribute('aria-hidden')).toBe('true')
    }
    expect(chevron('end').getAttribute('title')).toBe('More groups')
    fireEvent.click(chevron('end'))
    expect(scroller().scrollLeft).toBe(80)
    expect([bar().hasAttribute('data-more-start'), bar().hasAttribute('data-more-end')]).toEqual([true, false])
    fireEvent.click(chevron('start'))
    expect(scroller().scrollLeft).toBe(0)
  })

  it('a wide bar fits every group: no overflow, no chevrons', () => {
    stubLayout(400)
    render(<HomeGroupBar {...props({ groups: twelve, currentId: 't0' })} />)
    expect(bar().hasAttribute('data-overflow')).toBe(false)
  })

  it('End on the bar focuses the last group and scrolls it into view', () => {
    stubLayout(200)
    render(<HomeGroupBar {...props({ groups: twelve, currentId: 't0' })} />)
    groupButton('T0').focus()
    fireEvent.keyDown(groupButton('T0'), { key: 'End' })
    expect(document.activeElement).toBe(groupButton('T11'))
    expect(scroller().scrollLeft).toBe(80)
    fireEvent.keyDown(groupButton('T11'), { key: 'Home' })
    expect(scroller().scrollLeft).toBe(0)
  })

  it('the rail: up and down chevrons', () => {
    render(<HomeGroupBar {...props({ groups: twelve, currentId: 't0', orientation: 'vertical' })} />)
    expect(chevron('start').querySelector('svg')?.getAttribute('class')).toContain('lucide-chevron-up')
    expect(chevron('end').querySelector('svg')?.getAttribute('class')).toContain('lucide-chevron-down')
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

// K2 Design's Home nav: the right pane's Agents | Icon segments. "…" opens
// on Agents; a right-click on an icon opens on Icon.
describe('the Groups dialog: Agents | Icon', () => {
  const proj = (id: string, name: string) =>
    ({ id, name, handle: name.toLowerCase(), path: `/w/${name.toLowerCase()}`, color: '#61afef', iconUrl: null, workspaces: [] }) as never

  beforeEach(() => {
    useProjectsStore.setState({ projects: [proj('pc', 'Cortana'), proj('pn', 'Nova'), proj('pq', 'Quill')] } as never)
    setHomeNames(['Home', 'Studio'])
    const studio = useHomesStore.getState().homes[1]
    act(() => {
      useHomesStore.getState().selectHome(studio.id)
      useHomesStore.getState().addRow(studio.id, { address: 'cortana::local', workspaceId: 'pc', label: 'Cortana' })
      useHomesStore.getState().pinRow(studio.id, 'cortana::local')
    })
  })
  afterEach(() => useProjectsStore.setState({ projects: [] } as never))

  const segment = (name: string): HTMLElement => within(modal()).getByRole('radio', { name })
  const check = (name: string): HTMLInputElement => within(modal()).getByRole('checkbox', { name: `${name} in Studio` }) as HTMLInputElement
  const studio = () => useHomesStore.getState().homes[1]

  it('"…" opens on Agents: every addable agent, ticked when in the group; ticking adds, unticking removes it and its pin', () => {
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Studio)' }))
    // Name and "Most recent at the top" stay at the top, then the segments.
    const detail = modal().querySelector('[data-group-detail]') as HTMLElement
    const name = detail.querySelector('[data-group-name-input]') as HTMLElement
    const tabs = detail.querySelector('[data-group-detail-tabs]') as HTMLElement
    const recent = detail.querySelector('[data-group-recent-first]') as HTMLElement
    expect(name.compareDocumentPosition(recent) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(recent.compareDocumentPosition(tabs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(segment('Agents').getAttribute('aria-checked')).toBe('true')
    expect(segment('Icon').getAttribute('aria-checked')).toBe('false')
    expect(modal().querySelector('[data-group-icon-picker]')).toBeNull()
    // Delete stays pinned at the bottom.
    expect(detail.lastElementChild?.hasAttribute('data-group-delete-row')).toBe(true)

    expect(modal().querySelector('[data-group-agents-count]')?.textContent).toBe('1 in Studio')
    expect([check('Cortana').checked, check('Nova').checked, check('Quill').checked]).toEqual([true, false, false])

    fireEvent.click(check('Nova'))
    expect(studio().rows.map((r) => r.address)).toEqual(['cortana::local', 'nova::local'])
    expect(check('Nova').checked).toBe(true)
    expect(modal().querySelector('[data-group-agents-count]')?.textContent).toBe('2 in Studio')

    // Unticking the pinned Cortana takes it out, pin and all.
    expect(pinnedRows(studio()).map((r) => r.address)).toEqual(['cortana::local'])
    fireEvent.click(check('Cortana'))
    expect(studio().rows.map((r) => r.address)).toEqual(['nova::local'])
    expect(pinnedRows(studio())).toEqual([])
    expect(check('Cortana').checked).toBe(false)
    // The other group is untouched.
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).not.toContain('nova::local')
  })

  it('a row the nav finds by its workspace id (an old handle) counts as in: ticked, and unticking removes that row', () => {
    act(() => useHomesStore.getState().addRow(studio().id, { address: 'oldnova::local', workspaceId: 'pn', label: 'Nova' }))
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Studio)' }))
    expect([check('Cortana').checked, check('Nova').checked, check('Quill').checked]).toEqual([true, true, false])
    expect(modal().querySelector('[data-group-agents-count]')?.textContent).toBe('2 in Studio')
    fireEvent.click(check('Nova'))
    expect(studio().rows.map((r) => r.address)).toEqual(['cortana::local'])
  })

  it('the agents list fades right under its search: heavy, in the dialog\'s fill, no room under the field', () => {
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Studio)' }))
    const frame = within(modal()).getByTestId('group-agents-frame')
    expect(frame.hasAttribute('data-edge-heavy')).toBe(true)
    expect(frame.style.getPropertyValue('--k2-edge-tint')).toBe('var(--tile-fill, var(--color-bg-surface))')
    const header = frame.previousElementSibling as HTMLElement
    expect(header.hasAttribute('data-edge-header')).toBe(true)
    expect(header.className).not.toMatch(/(^|\s)(pb|py)-/)
    expect(header.className).toContain('z-[2]')
  })

  it('the search narrows the list; the list is one tab stop and ↑/↓ move through it', () => {
    render(<Wired />)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Studio)' }))
    const search = within(modal()).getByRole('searchbox', { name: 'Search agents for Studio' })
    fireEvent.change(search, { target: { value: 'qui' } })
    expect(within(modal()).getAllByRole('checkbox').map((c) => c.getAttribute('aria-label'))).toEqual(['Quill in Studio'])
    fireEvent.change(search, { target: { value: '' } })
    const boxes = within(modal()).getAllByRole('checkbox')
    expect(boxes.map((b) => b.tabIndex)).toEqual([0, -1, -1])
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(boxes[0])
    fireEvent.keyDown(boxes[0], { key: 'ArrowDown' })
    expect(document.activeElement).toBe(check('Nova'))
    expect(within(modal()).getAllByRole('checkbox').map((b) => b.tabIndex)).toEqual([-1, 0, -1])
    fireEvent.keyDown(check('Nova'), { key: 'ArrowUp' })
    expect(document.activeElement).toBe(check('Cortana'))
  })

  it('a right-click opens on Icon; ←/→ switch the segment, which stays as the selection changes', () => {
    render(<Wired />)
    fireEvent.contextMenu(groupButton('Studio'))
    expect(segment('Icon').getAttribute('aria-checked')).toBe('true')
    expect(modal().querySelector('[data-group-icon-search]')).not.toBeNull()
    expect(modal().querySelector('[data-group-agents]')).toBeNull()
    fireEvent.keyDown(segment('Icon'), { key: 'ArrowLeft' })
    expect(segment('Agents').getAttribute('aria-checked')).toBe('true')
    expect(modal().querySelector('[data-group-agents]')?.getAttribute('data-group-agents')).toBe(studio().id)
    // Another group in the list keeps Agents.
    const home = useHomesStore.getState().homes[0]
    fireEvent.click(modal().querySelector(`[data-group-list-row="${home.id}"]`) as HTMLElement)
    expect(modal().querySelector('[data-group-agents]')?.getAttribute('data-group-agents')).toBe(home.id)
    fireEvent.keyDown(segment('Agents'), { key: 'ArrowRight' })
    expect(segment('Icon').getAttribute('aria-checked')).toBe('true')
  })
})

// Round mp-4: Undo after a delete, and a rename that fails, on the store.
describe('the Groups dialog on the Homes store: Undo and a failed rename', () => {
  afterEach(() => vi.restoreAllMocks())

  it('a delete shows "<Name> deleted." with Undo; Undo puts it back as it was, in its place, current again', () => {
    setHomeNames(['Home', 'Errands', 'Later'])
    const st = useHomesStore.getState()
    const errands = st.homes[1]
    act(() => {
      st.selectHome(errands.id)
      st.addRow(errands.id, { address: 'atlas::local', workspaceId: null, label: 'Atlas' })
      st.pinRow(errands.id, 'atlas::local')
      st.setHomeIcon(errands.id, 'shopping-cart')
    })
    const before = useHomesStore.getState().homes[1]
    render(<Wired />)
    fireEvent.contextMenu(groupButton('Errands'))
    fireEvent.click(modal().querySelector('[data-group-delete]') as HTMLElement)
    fireEvent.click(modal().querySelector('[data-group-delete-yes]') as HTMLElement)
    // The nav behind already shows it gone.
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Home', 'Later'])
    expect(within(screen.getByRole('toolbar', { name: 'Groups' })).queryByRole('button', { name: 'Errands' })).toBeNull()
    const line = modal().querySelector('[data-group-undo]') as HTMLElement
    expect(line.getAttribute('role')).toBe('status')
    expect(line.textContent).toBe('Errands deleted.Undo')
    expect(document.activeElement).toBe(line.querySelector('[data-group-undo-button]'))
    fireEvent.click(line.querySelector('[data-group-undo-button]') as HTMLElement)
    const homes = useHomesStore.getState().homes
    expect(homes.map((h) => h.name)).toEqual(['Home', 'Errands', 'Later'])
    expect(homes[1]).toEqual(before)
    expect(pinnedRows(homes[1]).map((r) => r.address)).toEqual(['atlas::local'])
    expect(selectedHome(useHomesStore.getState()).id).toBe(errands.id)
    expect(modal().querySelector('[data-group-undo]')).toBeNull()
  })

  it('Undo lasts until the dialog closes', () => {
    setHomeNames(['Home', 'Errands'])
    render(<Wired />)
    fireEvent.contextMenu(groupButton('Errands'))
    fireEvent.click(modal().querySelector('[data-group-delete]') as HTMLElement)
    fireEvent.click(modal().querySelector('[data-group-delete-yes]') as HTMLElement)
    expect(modal().querySelector('[data-group-undo]')).not.toBeNull()
    fireEvent.click(modal().querySelector('[data-group-edit-done]') as HTMLElement)
    fireEvent.click(screen.getByRole('button', { name: 'Edit groups (Home)' }))
    expect(modal().querySelector('[data-group-undo]')).toBeNull()
  })

  it('a rename whose save fails keeps the text, edges the field and offers Try again', () => {
    setHomeNames(['Home', 'Errands'])
    render(<Wired />)
    fireEvent.contextMenu(groupButton('Errands'))
    const input = modal().querySelector('[data-group-name-input]') as HTMLInputElement
    const real = Storage.prototype.setItem
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key: string, value: string) {
      if (key === HOMES_STORAGE_KEY) throw new Error('QuotaExceededError')
      real.call(this, key, value)
    })
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    fireEvent.change(input, { target: { value: 'Shopping' } })
    expect(input.value).toBe('Shopping')
    expect(input.getAttribute('aria-invalid')).toBe('true')
    const err = modal().querySelector('[data-group-name-error]') as HTMLElement
    expect(err.getAttribute('role')).toBe('alert')
    expect(err.textContent).toBe('Couldn’t save the name. The server didn’t answer, so your text is kept. Try again')
    expect(input.getAttribute('aria-describedby')).toBe(err.id)
    expect(modal().querySelector('[data-group-saved]')).toBeNull()
    // Still failing: still the error.
    fireEvent.click(err.querySelector('[data-group-name-retry]') as HTMLElement)
    expect(modal().querySelector('[data-group-name-error]')).not.toBeNull()
    // Storage works again: Try again saves it.
    write.mockRestore()
    fireEvent.click(modal().querySelector('[data-group-name-retry]') as HTMLElement)
    expect(modal().querySelector('[data-group-name-error]')).toBeNull()
    expect(input.hasAttribute('aria-invalid')).toBe(false)
    expect(JSON.parse(localStorage.getItem(HOMES_STORAGE_KEY) ?? '{}').homes[1].name).toBe('Shopping')
    warn.mockRestore()
  })
})
