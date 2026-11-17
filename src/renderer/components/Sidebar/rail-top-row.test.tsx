// @vitest-environment jsdom
//
// The collapsed Agents rail opens with a top row the tab strip's height
// (TAB_STRIP_HEIGHT) with the strip's bottom line, as the open nav's top row
// is. » (open the nav) sits centred in it, so its centre lines up with the
// tab strip's own » across the gap, and the line carries on from the room's
// tab bar. Nothing sits above the row (the rail has no top padding). The
// pinned icons and search come under it. Pixels are checked in real-app
// shots, not in jsdom.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))
vi.mock('./ProjectAvatar', () => ({ default: () => <span /> }))
vi.mock('./ActiveBar', () => ({ default: () => null, useActiveBarItems: () => [] }))

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  const useProjectsStore = create(() => ({
    projects: [] as unknown[],
    activeProjectId: null as string | null,
    setActiveProject: () => undefined,
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create(() => ({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: false,
  }))
  return { useFocusGroupsStore }
})

import IconRail from './IconRail'
import { useProjectsStore } from '@/stores/projects'
import { useSidebarStore } from '@/stores/sidebar'
import { TAB_STRIP_HEIGHT } from '@/lib/tab-strip'

function proj(id: string, pinned: boolean): unknown {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    pinned: pinned ? 1 : 0,
    focusGroupId: null,
    iconUrl: null,
    handle: id,
    workspaces: [{ id: `ws-${id}`, type: 'main', navVisible: 0 }],
  }
}

const OPEN = 'Open workspaces sidebar'

function classes(el: Element): string[] {
  return el.className.split(/\s+/)
}

/** The rail's first child is its top row: the strip's height and line,
 *  contents centred, no padding that would push » off centre. */
function topRow(rail: HTMLElement): HTMLElement {
  const row = rail.firstElementChild as HTMLElement
  expect(row.hasAttribute('data-rail-top-row')).toBe(true)
  const cls = classes(row)
  expect(cls).toContain(TAB_STRIP_HEIGHT)
  expect(cls).toContain('border-b')
  expect(cls).toContain('border-[var(--color-border)]')
  expect(cls).toContain('items-center')
  expect(cls).toContain('flex-shrink-0')
  expect(cls.some((c) => /^(pt|pb|py|mt|my)-/.test(c))).toBe(false)
  return row
}

beforeEach(() => {
  useSidebarStore.setState({ isCollapsed: true })
})

afterEach(() => cleanup())

describe('the Agents rail top row', () => {
  for (const pinned of [false, true]) {
    it(`is the tab strip's height, with » in it${pinned ? ' (agents pinned)' : ''}`, () => {
      useProjectsStore.setState({ projects: [proj('a', pinned), proj('b', false)] as never })
      const { container } = render(<IconRail />)
      const rail = container.querySelector('[data-nav-rail]') as HTMLElement
      expect(rail).not.toBeNull()
      // Nothing above the row: the rail's own padding is only at the bottom.
      expect(classes(rail).some((c) => /^(pt|py)-/.test(c))).toBe(false)
      const row = topRow(rail)
      const buttons = row.querySelectorAll('button')
      expect(buttons).toHaveLength(1)
      expect(buttons[0].getAttribute('aria-label')).toBe(OPEN)
      // One » on the rail: it moved up, not copied.
      expect(container.querySelectorAll(`[aria-label="${OPEN}"]`)).toHaveLength(1)
      // The pins and search sit under the row.
      const search = container.querySelector('[title="Search Workspaces (⌘K)"]')!
      expect(row.compareDocumentPosition(search) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
      if (pinned) {
        const pin = container.querySelector('[title^="Agent a"]')!
        expect(row.compareDocumentPosition(pin) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
      }
      fireEvent.click(buttons[0])
      expect(useSidebarStore.getState().isCollapsed).toBe(false)
    })
  }
})
