// @vitest-environment jsdom
//
// Reordering groups in the Groups dialog by dragging a row. The gesture is
// plain mouse events (press, move, release), the same shared drag as the
// agent rows, because a desktop window's own drag-and-drop handling keeps
// HTML drag events from reaching the page. While dragging no row moves: a
// drop line floats where the row will land. The new order saves to the
// Homes store, which Home sync sends on like any other edit, so the group
// bar and the phone get it. ⌥↑/⌥↓ move the focused group and say where it
// went.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import GroupEditModal, { type GroupEditModalProps } from './GroupEditModal'
import type { GroupBarItem } from './HomeGroupBar'
import { useHomeGroups } from './useHomeGroups'
import { DROP_LINE_CAP_PX } from '@/components/Sidebar/row-drag-reorder'
import { useHomesStore, type Home } from '@/stores/homes'
import { buildLocal, diffLocal, emptySyncState, type SyncState } from '@/lib/homes-sync/engine'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [] },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [] },
  { id: 'g3', name: 'Later', icon: 'rocket', order: 2, recentFirst: false, pinned: [] },
]

function props(over: Partial<GroupEditModalProps> = {}): GroupEditModalProps {
  return {
    groups: G,
    currentId: 'g1',
    initialId: 'g2',
    onRename: vi.fn(() => 'saved' as const),
    onSetIcon: vi.fn(),
    onCreate: vi.fn(() => null),
    onDelete: vi.fn(),
    onSetRecentFirst: vi.fn(),
    onReorderGroups: vi.fn(),
    onClose: vi.fn(),
    ...over,
  }
}

const ROW_H = 34

function row(id: string): HTMLElement {
  const el = document.querySelector(`[data-group-list-row="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no list row ${id}`)
  return el
}

function rowIds(): string[] {
  return Array.from(document.querySelectorAll('[data-group-list-row]')).map((r) => r.getAttribute('data-group-list-row') ?? '')
}

function line(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-row-drop-line]')
}

function detailId(): string | null {
  return document.querySelector('[data-group-detail]')?.getAttribute('data-group-detail') ?? null
}

/** Rows ROW_H tall from y=0 in a list as tall as its rows; the list pane
 *  (the drag's bounds) is 220 × 400. */
function stubRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const rows = Array.from(document.querySelectorAll('[data-group-list-row]'))
    const i = rows.indexOf(this)
    let top = 0
    let height = 0
    if (i >= 0) {
      top = i * ROW_H
      height = ROW_H
    } else if (this.hasAttribute('data-group-list-body')) height = rows.length * ROW_H
    else if (this.hasAttribute('data-group-list')) height = 400
    return { top, height, bottom: top + height, left: 0, right: 220, width: 220, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

/** Press on a row's name, then move the pointer to `toY`. */
function dragFrom(id: string, toY: number): void {
  const name = row(id).querySelector('[data-group-list-name]') as HTMLElement
  const at = rowIds().indexOf(id) * ROW_H + ROW_H / 2
  fireEvent.mouseDown(name, { button: 0, clientX: 40, clientY: at })
  fireEvent.mouseMove(document, { clientX: 40, clientY: at + (toY > at ? 8 : -8) })
  fireEvent.mouseMove(document, { clientX: 40, clientY: toY })
}

beforeEach(() => {
  localStorage.clear()
  stubRects()
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
})

describe('dragging a group in the Groups dialog', () => {
  it('a mouse drag moves the group; the release is not a pick', () => {
    const p = props()
    render(<GroupEditModal {...p} />)
    // Later, up above Work's middle: the top of the list.
    dragFrom('g3', 6)
    fireEvent.mouseUp(document)
    fireEvent.click(row('g3'))
    expect(p.onReorderGroups).toHaveBeenCalledTimes(1)
    expect(p.onReorderGroups).toHaveBeenLastCalledWith(['g3', 'g1', 'g2'])
    // The click the release fires belongs to the drag: the selection stays.
    expect(detailId()).toBe('g2')
  })

  it('rows show a grab cursor; the dragged row goes faint and the cursor grabs', () => {
    render(<GroupEditModal {...props()} />)
    expect(row('g1').className).toContain('cursor-grab')
    dragFrom('g1', 80)
    expect(row('g1').hasAttribute('data-dragging')).toBe(true)
    expect(row('g1').className).toContain('data-[dragging]:opacity-40')
    expect(row('g2').hasAttribute('data-dragging')).toBe(false)
    expect(document.body.style.cursor).toBe('grabbing')
    for (const id of ['g1', 'g2', 'g3']) expect(row(id).className).toContain('cursor-grabbing')
    fireEvent.mouseUp(document)
    expect(row('g1').hasAttribute('data-dragging')).toBe(false)
    expect(document.body.style.cursor).toBe('')
  })

  it('no row moves while dragging: no gap opens, the rows keep their order and place', () => {
    render(<GroupEditModal {...props()} />)
    const list = document.querySelector('[data-group-list-rows]') as HTMLElement
    const before = Array.from(list.children)
    dragFrom('g3', 6)
    expect(document.querySelector('[data-group-drop-gap]')).toBeNull()
    expect(Array.from(list.children)).toEqual(before)
    expect(rowIds()).toEqual(['g1', 'g2', 'g3'])
    for (const id of ['g1', 'g2', 'g3']) {
      expect(row(id).style.transform).toBe('')
      expect(row(id).style.marginTop).toBe('')
    }
    expect(line()).not.toBeNull()
  })

  it('the drop line: on the seam where the row lands, inset to the row text, none over its own place', () => {
    render(<GroupEditModal {...props()} />)
    // Work, down past Play's middle (51) but above Later's (85): gap 2,
    // the seam between Play and Later at 68.
    dragFrom('g1', 73)
    const l = line() as HTMLElement
    expect(l.style.top).toBe(`${2 * ROW_H - 1}px`)
    expect([l.style.left, l.style.right, l.style.height]).toEqual(['10px', '10px', '2px'])
    expect(l.className).toContain('bg-[var(--color-accent)]')
    expect(l.className).toContain('rounded-[var(--radius-pill)]')
    expect(l.className).toContain('motion-reduce:transition-none')
    const cap = l.querySelector('[data-row-drop-line-cap]') as HTMLElement
    expect([cap.style.width, cap.style.height]).toEqual([`${DROP_LINE_CAP_PX}px`, `${DROP_LINE_CAP_PX}px`])
    // It floats over the list, not inside the listbox: the rows stay its
    // only children.
    const body = document.querySelector('[data-group-list-body]') as HTMLElement
    expect(l.parentElement).toBe(body)
    expect(body.className).toContain('relative')
    expect(l.className).toContain('absolute')
    // Back over its own place: no line, and a release there does nothing.
    fireEvent.mouseMove(document, { clientX: 40, clientY: 20 })
    expect(line()).toBeNull()
    // Past the last row: the list's end, kept half a cap inside.
    fireEvent.mouseMove(document, { clientX: 40, clientY: 200 })
    expect((line() as HTMLElement).style.top).toBe(`${3 * ROW_H - DROP_LINE_CAP_PX / 2 - 1}px`)
    fireEvent.mouseUp(document)
    expect(line()).toBeNull()
  })

  it('a release outside the list pane does nothing', () => {
    const p = props()
    render(<GroupEditModal {...p} />)
    dragFrom('g1', 73)
    fireEvent.mouseMove(document, { clientX: 400, clientY: 73 })
    expect(line()).toBeNull()
    fireEvent.mouseUp(document)
    expect(p.onReorderGroups).not.toHaveBeenCalled()
  })
})

describe('⌥↑/⌥↓ in the Groups dialog', () => {
  function Owner({ onReorder }: { onReorder(ids: string[]): void }): React.JSX.Element {
    const [groups, setGroups] = useState<GroupBarItem[]>(G)
    return (
      <GroupEditModal
        {...props()}
        groups={groups}
        onReorderGroups={(ids) => {
          onReorder(ids)
          setGroups(ids.map((id) => G.find((g) => g.id === id) as GroupBarItem))
        }}
      />
    )
  }

  it('moves the focused group, keeps focus on it and says where it went', () => {
    const onReorder = vi.fn()
    render(<Owner onReorder={onReorder} />)
    row('g1').focus()
    fireEvent.keyDown(row('g1'), { key: 'ArrowDown', altKey: true })
    expect(onReorder).toHaveBeenLastCalledWith(['g2', 'g1', 'g3'])
    expect(rowIds()).toEqual(['g2', 'g1', 'g3'])
    expect(document.activeElement).toBe(row('g1'))
    const live = document.querySelector('[data-group-list-live]') as HTMLElement
    expect([live.getAttribute('aria-live'), live.textContent]).toEqual(['polite', 'Moved Work to position 2 of 3'])
    fireEvent.keyDown(row('g1'), { key: 'ArrowDown', altKey: true })
    expect(live.textContent).toBe('Moved Work to position 3 of 3')
    // At the end it stays put and says nothing new.
    fireEvent.keyDown(row('g1'), { key: 'ArrowDown', altKey: true })
    expect(onReorder).toHaveBeenCalledTimes(2)
    expect(document.activeElement).toBe(row('g1'))
  })
})

describe('the saved order', () => {
  const ctx = { toWire: (a: string) => a, fromWire: (a: string) => a, deviceId: 'dev-a', now: 1000 }
  function merged(): SyncState {
    return { ...emptySyncState('dev-a'), merged: true, serverId: 'aaaaaaaaaaaaaaaa', cursor: 0 }
  }
  function Wired(): React.JSX.Element {
    const { bar } = useHomeGroups()
    return (
      <GroupEditModal
        {...props()}
        groups={bar.groups}
        currentId={bar.currentId}
        initialId={bar.groups[0].id}
        onReorderGroups={bar.onReorderGroups}
      />
    )
  }

  it('a drag saves through the Homes store, and Home sync sends the new order (People moves like any group)', () => {
    const homes = [
      { id: 'g1', name: 'Work', rows: [] },
      { id: 'g2', name: 'Errands', rows: [] },
      { id: 'people', name: 'People', rows: [], builtin: 'people' } as Home,
      { id: 'g3', name: 'Later', rows: [] },
    ] as Home[]
    useHomesStore.setState({ homes, selectedId: 'g1' })
    const synced = diffLocal(homes, merged(), ctx).state

    render(<Wired />)
    // People, up above Work's middle: the top of the list.
    dragFrom('people', 6)
    fireEvent.mouseUp(document)

    const saved = useHomesStore.getState().homes.map((h) => h.id)
    expect(saved).toEqual(['people', 'g1', 'g2', 'g3'])
    // The dialog (and the group bar, which reads the same store) follow it.
    expect(rowIds()).toEqual(saved)
    // Home sync queues the move, and what it now knows reads back in the new order.
    const sent = diffLocal(useHomesStore.getState().homes, synced, { ...ctx, now: 2000 })
    expect(sent.changes.some((c) => c.kind === 'home' && c.id === 'people')).toBe(true)
    expect((buildLocal(sent.state, [], ctx) ?? []).map((h) => h.id)).toEqual(saved)
  })
})
