// @vitest-environment jsdom
//
// Agent counts in the two nav pickers (Rosson 2026-10-16): the Agents
// nav's focus-group picker shows how many agents the nav lists for the
// pick, like the Home picker does for a Home; in both, the count sits
// right after the name (not at the far edge), a long name truncates, and
// the count stays.
//
// jsdom has no layout, so "right after the name" and "truncation keeps the
// count" are asserted as the DOM + the classes that produce them: the count
// is the name's next sibling in one label; the label takes the free width
// (flex-1 min-w-0); the name does not grow (no flex-1) but may shrink with
// an ellipsis (min-w-0 truncate); the count never shrinks (flex-shrink-0).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

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
  const useProjectsStore = create<{
    projects: unknown[]
    activeProjectId: string | null
    activeWorkspaceId: string | null
    reorderProjects: () => Promise<void>
    setActiveWorkspace: () => void
    removeProject: () => Promise<void>
    addProject: () => Promise<void>
    fetchProjects: () => Promise<void>
  }>(() => ({
    projects: [],
    activeProjectId: null,
    activeWorkspaceId: null,
    reorderProjects: async () => undefined,
    setActiveWorkspace: () => undefined,
    removeProject: async () => undefined,
    addProject: async () => undefined,
    fetchProjects: async () => { throw new Error('a picker count must not refetch') },
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create<{
    focusGroups: { id: string; name: string; color: string }[]
    activeFocusGroupId: string | null
    focusGroupsEnabled: boolean
    setActiveFocusGroup: (id: string | null) => void
  }>((set) => ({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: true,
    setActiveFocusGroup: (id) => set({ activeFocusGroupId: id }),
  }))
  return { useFocusGroupsStore }
})

import Sidebar from './Sidebar'
import HomePicker from '@/components/Home/HomePicker'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useHomesStore, selectedHome } from '@/stores/homes'

type StoreProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]
type StoreFocusGroup = ReturnType<typeof useFocusGroupsStore.getState>['focusGroups'][number]

function proj(id: string, opts: { pinned?: boolean; group?: string | null } = {}): StoreProject {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    pinned: opts.pinned ? 1 : 0,
    focusGroupId: opts.group ?? null,
    iconUrl: null,
    handle: id,
    workspaces: [{ id: `ws-${id}`, type: 'main', navVisible: 0 }],
  } as unknown as StoreProject
}

// Under HOME_NAME_MAX (60), so the Home keeps it whole.
const LONG = 'A name long enough that the picker has to cut it off'

function group(id: string, name: string): StoreFocusGroup {
  return { id, name, color: '#888888' } as unknown as StoreFocusGroup
}

/** The label's name and count; fails loudly when either is missing. */
function labelParts(scope: Element): { label: HTMLElement; name: HTMLElement; count: HTMLElement } {
  const label = scope.querySelector<HTMLElement>('[data-picker-label]')
  if (!label) throw new Error('no picker label')
  const name = label.querySelector<HTMLElement>('[data-picker-name]')
  const count = label.querySelector<HTMLElement>('[data-picker-count]')
  if (!name || !count) throw new Error('label lacks a name or a count')
  return { label, name, count }
}

/** Count right after the name; name truncates without growing; count never shrinks. */
function expectCountAfterName(scope: Element, name: string, count: number): void {
  const parts = labelParts(scope)
  expect(parts.name.textContent).toBe(name)
  expect(parts.count.textContent).toBe(String(count))
  expect(parts.name.nextElementSibling).toBe(parts.count)
  expect(parts.label.className).toMatch(/\bflex-1\b/)
  expect(parts.label.className).toMatch(/\bmin-w-0\b/)
  expect(parts.name.className).toMatch(/\btruncate\b/)
  expect(parts.name.className).toMatch(/\bmin-w-0\b/)
  expect(parts.name.className).not.toMatch(/\bflex-1\b/)
  expect(parts.count.className).toMatch(/\bflex-shrink-0\b/)
}

function trigger(selector: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(selector)
  if (!el) throw new Error(`missing ${selector}`)
  return el
}

afterEach(() => cleanup())

describe('focus-group picker count (Agents nav)', () => {
  beforeEach(() => {
    localStorage.clear()
    useFocusGroupsStore.setState({
      focusGroups: [group('g1', LONG), group('g2', 'Two')],
      focusGroupsEnabled: true,
      activeFocusGroupId: 'g1',
    })
    useProjectsStore.setState({
      projects: [
        proj('a', { group: 'g1' }),
        proj('b', { group: 'g1' }),
        proj('c', { group: 'g2' }),
        proj('n'), // ungrouped: listed in every group view
        proj('p', { pinned: true, group: 'g1' }), // pinned: its own Pinned list
      ],
    })
  })

  it('the button shows how many agents the list under it shows, right after the name', () => {
    const { container } = render(<Sidebar />)
    const listed = container.querySelectorAll('[data-project-id]')
    // Pinned list (p) + g1 list (a, b, n).
    expect(listed).toHaveLength(4)
    expectCountAfterName(trigger('[data-focus-group-trigger]'), LONG, 3)
  })

  it('updates live when agents are added, removed, or moved between groups', () => {
    render(<Sidebar />)
    const count = (): string | null => labelParts(trigger('[data-focus-group-trigger]')).count.textContent
    expect(count()).toBe('3')
    act(() => { useProjectsStore.setState({ projects: [...useProjectsStore.getState().projects, proj('d', { group: 'g1' })] }) })
    expect(count()).toBe('4')
    act(() => {
      useProjectsStore.setState({
        projects: useProjectsStore.getState().projects.map((p) => (p.id === 'a' ? proj('a', { group: 'g2' }) : p)),
      })
    })
    expect(count()).toBe('3')
    act(() => { useProjectsStore.setState({ projects: useProjectsStore.getState().projects.filter((p) => p.id !== 'n') }) })
    expect(count()).toBe('2')
    act(() => { useFocusGroupsStore.setState({ activeFocusGroupId: 'g2' }) })
    expect(labelParts(trigger('[data-focus-group-trigger]')).name.textContent).toBe('Two')
    expect(count()).toBe('2') // c + a
  })

  it('each menu row carries its group\'s count right after the name', () => {
    render(<Sidebar />)
    fireEvent.click(trigger('[data-focus-group-trigger]'))
    const menu = trigger('[data-focus-group-menu]')
    const rows = Array.from(menu.querySelectorAll('button'))
    expect(rows).toHaveLength(2)
    expectCountAfterName(rows[0], LONG, 3)
    expectCountAfterName(rows[1], 'Two', 2) // c + n
  })
})

describe('Home picker count', () => {
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
    const st = useHomesStore.getState()
    const id = st.createHome(LONG)
    if (!id) throw new Error('createHome refused')
    for (const r of [...selectedHome(useHomesStore.getState()).rows]) st.removeRow(id, r.address)
    st.addRow(id, { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' })
    st.addRow(id, { address: 'nova::local', workspaceId: 'pn', label: 'Nova' })
  })

  it('the button shows the Home\'s agent count right after the name', () => {
    render(<HomePicker />)
    expectCountAfterName(trigger('[aria-haspopup="menu"]'), LONG, 2)
  })

  it('updates live when an agent is added or removed', () => {
    render(<HomePicker />)
    const home = selectedHome(useHomesStore.getState())
    act(() => { useHomesStore.getState().addRow(home.id, { address: 'bee::b.k2.dev', workspaceId: 'pb', label: 'Bee' }) })
    expect(labelParts(trigger('[aria-haspopup="menu"]')).count.textContent).toBe('3')
    act(() => { useHomesStore.getState().removeRow(home.id, 'cortana::local') })
    expect(labelParts(trigger('[aria-haspopup="menu"]')).count.textContent).toBe('2')
  })

  it('the menu row for each Home carries its count right after the name', () => {
    render(<HomePicker />)
    fireEvent.click(trigger('[aria-haspopup="menu"]'))
    const current = screen.getByRole('menuitemradio', { checked: true })
    expectCountAfterName(current, LONG, 2)
  })
})
