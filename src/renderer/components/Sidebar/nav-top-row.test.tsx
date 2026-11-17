// @vitest-environment jsdom
//
// The Agents nav's top row is the tab strip's height (TAB_STRIP_HEIGHT, the
// room tab bar's and the drawers' strips' height) with its bottom line, as
// Home's top row is, so its bottom edge lines up with the tab bar's across
// the gap. Whichever comes first in the nav takes it:
//   - agents are pinned: the Pinned header (the focus group picker or the
//     Workspaces header sits under the pins as before);
//   - nothing pinned, focus groups on: the focus group picker's row;
//   - neither: the Workspaces header.
// Exactly one row is the top row. Folding Pinned folds the section's end
// rule away with the pins, so the top row's own line is the only one left.
// Alignment itself (pixels) is checked in real-app shots, not in jsdom.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

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
vi.mock('@/stores/activity', () => ({
  useActivity: () => 'idle',
  workspaceDisplay: () => 'idle',
  projectHasUnseen: () => false,
}))
vi.mock('@/components/AgentStatus/useAgentStatus', () => ({
  useAgentStatus: () => ({ heartbeat: false, live: null, working: false, status: 'idle', title: '', workingTitle: '' }),
}))
vi.mock('@/hooks/useGit', () => ({ useGitInfo: () => ({ data: undefined }) }))
vi.mock('./ProjectAvatar', () => ({ default: () => <span /> }))
vi.mock('@/components/Presence/PresenceWorkspaceAvatars', () => ({ default: () => null }))
vi.mock('./NavProjectTags', () => ({ NavProjectTags: () => null }))
vi.mock('./ActiveBar', () => ({ default: () => null }))
vi.mock('./ResizeHandle', () => ({ default: () => null }))
vi.mock('./WorktreeDialog', () => ({ default: () => null }))
vi.mock('@/components/GrokBots/GrokBotRows', () => ({ GrokBotSidebarSection: () => null }))
vi.mock('@/stores/grokbots', () => ({ closeGrokChat: () => undefined }))

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  const useProjectsStore = create(() => ({
    projects: [] as unknown[],
    activeProjectId: null as string | null,
    activeWorkspaceId: null as string | null,
    setActiveWorkspace: () => undefined,
    reorderProjects: async () => undefined,
    fetchProjects: async () => undefined,
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create(() => ({
    focusGroups: [] as Array<{ id: string; name: string; color: string | null }>,
    activeFocusGroupId: null as string | null,
    focusGroupsEnabled: false,
    setActiveFocusGroup: () => undefined,
  }))
  return { useFocusGroupsStore }
})

import Sidebar from './Sidebar'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { TAB_STRIP_HEIGHT } from '@/lib/tab-strip'

type StoreProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]

function proj(id: string, pinned = false): StoreProject {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    pinned: pinned ? 1 : 0,
    focusGroupId: 'fg-1',
    iconUrl: null,
    handle: id,
    workspaces: [{ id: `ws-${id}`, type: 'main', navVisible: 0 }],
  } as unknown as StoreProject
}

function setup({ pinned, focusGroups }: { pinned: boolean; focusGroups: boolean }): void {
  useProjectsStore.setState({ projects: [proj('a', pinned), proj('b', pinned), proj('c'), proj('d')] })
  useFocusGroupsStore.setState({
    focusGroupsEnabled: focusGroups,
    focusGroups: focusGroups ? [{ id: 'fg-1', name: 'Client work', color: '#4f8cff', tabOrder: 0, createdAt: 1700000000 }] : [],
    activeFocusGroupId: focusGroups ? 'fg-1' : null,
  })
}

/** The one top row: tab-strip height with the strip's bottom line, its
 *  contents centred. */
function topRow(container: HTMLElement): HTMLElement {
  const rows = container.querySelectorAll<HTMLElement>('[data-nav-top-row]')
  expect(rows).toHaveLength(1)
  const row = rows[0]
  const cls = row.className.split(/\s+/)
  expect(cls).toContain(TAB_STRIP_HEIGHT)
  expect(cls).toContain('border-b')
  expect(cls).toContain('items-center')
  // No padding that would push its contents off centre in the fixed height.
  expect(cls.some((c) => /^(pt|pb|py)-/.test(c))).toBe(false)
  return row
}

/** The top row is the nav's first row: nothing above it in the column. */
function isFirstInNav(container: HTMLElement, row: HTMLElement): boolean {
  const column = container.querySelector('[data-agents-nav-list]')!.parentElement!
  const first = Array.from(column.children)[0]
  return first === row || first.firstElementChild === row
}

beforeEach(() => {
  localStorage.clear()
  useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null })
})

afterEach(() => cleanup())

describe('the Agents nav top row is the tab strip height', () => {
  it('agents pinned: the Pinned header is the top row; the focus group picker sits under the pins as before', () => {
    setup({ pinned: true, focusGroups: true })
    const { container } = render(<Sidebar />)
    const row = topRow(container)
    expect(row.textContent).toContain('Pinned')
    expect(row.closest('[data-nav-section="pinned"]')).not.toBeNull()
    expect(isFirstInNav(container, row)).toBe(true)
    // The picker's own row keeps its padding and is not a top row.
    const pickerRow = container.querySelector('[data-focus-group-trigger]')!.closest('[data-nav-picker-row]') as HTMLElement
    expect(pickerRow.className.split(/\s+/)).not.toContain(TAB_STRIP_HEIGHT)
    expect(pickerRow.hasAttribute('data-nav-top-row')).toBe(false)
  })

  it('agents pinned, focus groups off: the Pinned header is the top row; Workspaces sits under the pins', () => {
    setup({ pinned: true, focusGroups: false })
    const { container } = render(<Sidebar />)
    expect(topRow(container).textContent).toContain('Pinned')
    const workspaces = screen.getByText('Workspaces').parentElement!
    expect(workspaces.className.split(/\s+/)).not.toContain(TAB_STRIP_HEIGHT)
  })

  it('nothing pinned, focus groups on: the focus group picker\'s row is the top row', () => {
    setup({ pinned: false, focusGroups: true })
    const { container } = render(<Sidebar />)
    const row = topRow(container)
    expect(row.querySelector('[data-focus-group-trigger]')).not.toBeNull()
    expect(isFirstInNav(container, row)).toBe(true)
  })

  it('in the top row the picker is the short one, so it clears the row\'s edges', () => {
    setup({ pinned: false, focusGroups: true })
    const { container } = render(<Sidebar />)
    const trigger = topRow(container).querySelector('[data-focus-group-trigger]')!
    expect(trigger.className.split(/\s+/)).toContain('py-1')
    cleanup()
    setup({ pinned: true, focusGroups: true })
    const below = render(<Sidebar />).container.querySelector('[data-focus-group-trigger]')!
    expect(below.className.split(/\s+/)).toContain('py-1.5')
  })

  it('neither: the Workspaces header is the top row', () => {
    setup({ pinned: false, focusGroups: false })
    const { container } = render(<Sidebar />)
    const row = topRow(container)
    expect(row.textContent).toBe('Workspaces')
    expect(isFirstInNav(container, row)).toBe(true)
  })

  it('folding Pinned folds the section\'s end rule away with the pins (no second line under the top row)', () => {
    setup({ pinned: true, focusGroups: true })
    const { container } = render(<Sidebar />)
    const header = topRow(container)
    const body = document.getElementById(header.getAttribute('aria-controls')!)!
    const endRule = container.querySelector('[data-nav-section="pinned"] [data-nav-section-body] > [data-row-divider]')
    expect(endRule).not.toBeNull()
    expect(body.contains(endRule)).toBe(true)
    fireEvent.click(header)
    expect(header.getAttribute('aria-expanded')).toBe('false')
    expect(body.style.maxHeight).toBe('0px')
  })
})
