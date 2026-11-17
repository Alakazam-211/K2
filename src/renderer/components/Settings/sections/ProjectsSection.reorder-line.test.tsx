// @vitest-environment jsdom

// Settings → Workspaces: dragging a workspace row moves no row. An accent
// line floats over the card where the row will land (top, mid-list,
// bottom), spanning the rows' text; none over its own place or off the
// card. The drop writes the same full order as before.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})), settingsReset: vi.fn() }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { ProjectsSection } from './ProjectsSection'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

const ROW_H = 44

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function workspace(id: string): Record<string, unknown> {
  return { id, name: id, path: `/Users/sam/DevProjects/${id}`, color: '#3b82f6', workspaces: [], pinned: 0, focusGroupId: null }
}

/** Rows ROW_H tall from y=0 in their card; a card as tall as its rows, 600 wide. */
function stubRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const zone = this.closest('[data-reorder-zone]')
    const rows = zone ? Array.from(zone.querySelectorAll('[data-settings-project-id]')) : []
    const i = rows.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    const height = this.hasAttribute('data-reorder-zone') ? rows.length * ROW_H : ROW_H
    return { top, height, bottom: top + height, left: 0, right: 600, width: 600, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

const reorder = vi.fn(async () => {})

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  // jsdom has no hit testing: nothing under the pointer is a group header.
  document.elementFromPoint = () => null
  stubRects()
  reorder.mockClear()
  useSettingsStore.setState({ initialProjectId: null, projectsPage: null, initialWorkspaceTab: null })
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false } as never)
  useProjectsStore.setState({ projects: ['a', 'b', 'c', 'd'].map(workspace), reorderProjects: reorder } as never)
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  useProjectsStore.setState({ projects: [] } as never)
})

function zone(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-reorder-zone="flat"]')
  if (!el) throw new Error('the workspace card is not rendered')
  return el
}
const line = (): HTMLElement | null => zone().querySelector<HTMLElement>(':scope > [data-row-drop-line]')
const rowEl = (id: string): HTMLElement => zone().querySelector<HTMLElement>(`[data-settings-project-id="${id}"]`)!

describe('Settings → Workspaces drag', () => {
  it('no row moves; the line marks the top, mid-list and bottom; the drop writes the full order', () => {
    render(<ProjectsSection />)
    const rowsBefore = Array.from(zone().querySelectorAll('[data-settings-project-id]'))
    const childrenBefore = Array.from(zone().children)
    fireEvent.mouseDown(within(rowEl('b')).getByText('b'), { button: 0, clientX: 40, clientY: ROW_H * 1.5 })
    const topAt = (y: number): string | null => {
      fireEvent.mouseMove(document, { clientX: 40, clientY: y })
      expect(Array.from(zone().querySelectorAll('[data-settings-project-id]'))).toEqual(rowsBefore)
      // The rows' boxes are the same children; the line is the only addition.
      expect(Array.from(zone().children).filter((c) => !c.hasAttribute('data-row-drop-line'))).toEqual(childrenBefore)
      for (const r of rowsBefore) expect((r as HTMLElement).style.transform).toBe('')
      return line()?.style.top ?? null
    }
    expect(topAt(5)).toBe(`${3 - 1}px`)
    expect(topAt(ROW_H * 2.6)).toBe(`${3 * ROW_H - 1}px`)
    expect(topAt(ROW_H * 3.9)).toBe(`${4 * ROW_H - 3 - 1}px`)
    // Its own place: no line.
    expect(topAt(ROW_H * 1.2)).toBeNull()
    // The line spans the rows' text (16px in each side) in the accent colour.
    topAt(5)
    expect(line()!.style.left).toBe('16px')
    expect(line()!.style.right).toBe('16px')
    expect(line()!.className).toContain('bg-[var(--color-accent)]')
    fireEvent.mouseUp(document)
    expect(line()).toBeNull()
    expect(reorder).toHaveBeenCalledWith(['b', 'a', 'c', 'd'])
  })

  it('off the card: no line, and a release there writes nothing', () => {
    render(<ProjectsSection />)
    fireEvent.mouseDown(within(rowEl('a')).getByText('a'), { button: 0, clientX: 40, clientY: ROW_H / 2 })
    fireEvent.mouseMove(document, { clientX: 40, clientY: ROW_H * 3.9 })
    expect(line()).not.toBeNull()
    fireEvent.mouseMove(document, { clientX: 700, clientY: ROW_H * 3.9 })
    expect(line()).toBeNull()
    fireEvent.mouseUp(document)
    expect(reorder).not.toHaveBeenCalled()
  })
})
