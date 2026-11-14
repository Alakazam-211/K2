// @vitest-environment jsdom
//
// The selected agent row drops the dividers around it, like the active tab
// (Rosson 2026-11-14: "if you select an agent the dividers around the agent
// you selected should go away (like the tabs in the tab area)"). The real
// Agents nav is rendered; its row leaves are stubbed. The Home nav's case is
// in components/Home/HomePage.test.tsx.
//
// Asserted, against the rule in globals.css (jsdom matches :has):
//   - the line under the selected row and the line over it are hidden;
//     every other line shows;
//   - a selected first or last row hides only the one line it touches;
//   - with a worktree row under the selected agent, the line between them
//     is the one hidden, not the line under the worktree row;
//   - the Pinned section's end rule goes under a selected last pin, and
//     comes back while the section is folded;
//   - the rule never reads :hover, so a hovered neighbour changes nothing;
//   - inside a Garden nothing is hidden.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
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
  const useProjectsStore = create<{
    projects: unknown[]
    activeProjectId: string | null
    activeWorkspaceId: string | null
    setActiveWorkspace: (projectId: string, workspaceId: string) => void
    reorderProjects: () => Promise<void>
    fetchProjects: () => Promise<void>
  }>((set) => ({
    projects: [],
    activeProjectId: null,
    activeWorkspaceId: null,
    setActiveWorkspace: (projectId, workspaceId) => set({ activeProjectId: projectId, activeWorkspaceId: workspaceId }),
    reorderProjects: async () => undefined,
    fetchProjects: async () => undefined,
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create(() => ({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: false,
    setActiveFocusGroup: () => undefined,
  }))
  return { useFocusGroupsStore }
})

import Sidebar from './Sidebar'
import { useProjectsStore } from '@/stores/projects'

const dir = dirname(fileURLToPath(import.meta.url))
const globals = readFileSync(join(dir, '../../globals.css'), 'utf8')

/** The selectors of the rule that hides the lines beside the selected row. */
function hideSelectors(): string[] {
  const at = globals.indexOf('Row dividers beside the selected agent row')
  if (at < 0) throw new Error('missing the "Row dividers beside the selected agent row" rule')
  const start = globals.indexOf('*/', at) + 2
  const open = globals.indexOf('{', start)
  const body = globals.slice(open, globals.indexOf('}', open))
  expect(body).toMatch(/visibility:\s*hidden/)
  return globals.slice(start, open).split(',').map((s) => s.trim()).filter(Boolean)
}

/** Each [data-row-divider] in the nav, top to bottom: hidden or shown. */
function lines(root: ParentNode): Array<'hidden' | 'shown'> {
  const sels = hideSelectors()
  return Array.from(root.querySelectorAll('[data-row-divider]')).map((el) => (sels.some((s) => el.matches(s)) ? 'hidden' : 'shown'))
}

type StoreProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]

/** The fields the nav reads; the store's type is the real one (mocked above). */
function proj(id: string, worktree = false, pinned = false): StoreProject {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    pinned: pinned ? 1 : 0,
    focusGroupId: null,
    iconUrl: null,
    handle: id,
    workspaces: [
      { id: `ws-${id}`, type: 'main', navVisible: 0 },
      ...(worktree ? [{ id: `wt-${id}`, type: 'worktree', navVisible: 1, name: 'feature', branch: 'feature' }] : []),
    ],
  } as unknown as StoreProject
}

function select(id: string): void {
  useProjectsStore.setState({ activeProjectId: id, activeWorkspaceId: `ws-${id}` })
}

beforeEach(() => {
  localStorage.clear()
  useProjectsStore.setState({ projects: ['a', 'b', 'c', 'd', 'e'].map((id) => proj(id)), activeProjectId: null, activeWorkspaceId: null })
})

afterEach(() => cleanup())

describe('the selected agent row drops the dividers around it (Agents nav)', () => {
  it('the rule is keyed on the selection only, never on hover', () => {
    const sels = hideSelectors()
    expect(sels).toHaveLength(4)
    for (const s of sels) {
      expect(s).toContain('[data-agent-row][aria-current="true"]')
      expect(s).not.toContain(':hover')
      expect(s).toContain(':not([data-zen-root] *)')
    }
  })

  it('a middle row: no line under it, none over it; the others show', () => {
    select('c')
    const { container } = render(<Sidebar />)
    // Lines after a, b, c, d (none after the last row).
    expect(lines(container)).toEqual(['shown', 'hidden', 'hidden', 'shown'])
  })

  it('the first row: only the line under it goes', () => {
    select('a')
    const { container } = render(<Sidebar />)
    expect(lines(container)).toEqual(['hidden', 'shown', 'shown', 'shown'])
  })

  it('the last row: only the line over it goes', () => {
    select('e')
    const { container } = render(<Sidebar />)
    expect(lines(container)).toEqual(['shown', 'shown', 'shown', 'hidden'])
  })

  it('nothing selected: every line shows', () => {
    const { container } = render(<Sidebar />)
    expect(lines(container)).toEqual(['shown', 'shown', 'shown', 'shown'])
  })

  it('with a worktree row under the selected agent, the line between them goes, not the one under the worktree', () => {
    useProjectsStore.setState({ projects: [proj('a'), proj('b', true), proj('c')] })
    select('b')
    const { container } = render(<Sidebar />)
    // a's line (over b), b → worktree, under b's worktree row.
    expect(lines(container)).toEqual(['hidden', 'hidden', 'shown'])
  })

  it('inside a Garden nothing is hidden', () => {
    select('c')
    const { container } = render(<div data-zen-root=""><Sidebar /></div>)
    expect(lines(container)).toEqual(['shown', 'shown', 'shown', 'shown'])
  })

  it('a selected last pin drops the Pinned section\'s end rule; folded, the rule is back', () => {
    useProjectsStore.setState({ projects: [proj('a', false, true), proj('b', false, true), proj('c'), proj('d')] })
    select('b')
    const { container } = render(<Sidebar />)
    // The section's end rule, then the line under c.
    expect(lines(container)).toEqual(['hidden', 'shown'])
    fireEvent.click(screen.getByRole('button', { expanded: true }))
    expect(lines(container)).toEqual(['shown', 'shown'])
  })

  it('a selected first pin keeps the section\'s end rule (it touches the last pin, not this one)', () => {
    useProjectsStore.setState({ projects: [proj('a', false, true), proj('b', false, true), proj('c'), proj('d')] })
    select('a')
    const { container } = render(<Sidebar />)
    expect(lines(container)).toEqual(['shown', 'shown'])
  })
})
