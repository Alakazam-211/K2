// @vitest-environment jsdom
// 0.46.0 (Rosson 2026-10-16): the Workspace tab no longer shows the agent's
// display name or handle (they live on the Agent tab). Its header puts View
// Wiki on the left and keeps the notifications bell on the right.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 0) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))
vi.mock('@/components/HeartbeatsPanel/HeartbeatsPanel', () => ({ HeartbeatsPanel: () => null }))
vi.mock('./ConnectedAgentsSection', () => ({ ConnectedAgentsSection: () => null }))
vi.mock('./UrlsPortsSection', () => ({ UrlsPortsSection: () => null }))
vi.mock('./WorkspaceApiSection', () => ({ WorkspaceApiSection: () => null }))
vi.mock('./WorkspaceCompletionSoundToggle', () => ({
  WorkspaceCompletionSoundBell: () => <button type="button" data-testid="completion-bell">bell</button>,
}))

import WorkspacePanel from './WorkspacePanel'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { usePageViewStore } from '@/stores/page-view'

const project = {
  id: 'p1',
  name: 'cortana',
  path: '/ws/cortana',
  color: '#3b82f6',
  iconUrl: null,
  handle: 'cortana-handle',
  agentMode: 'custom',
  workspaces: [],
} as unknown as ProjectWithWorkspaces

function render(isPrimary: boolean): void {
  const tabs = createStore(() => ({ openAgentPane: vi.fn() }))
  const room = testRoom({
    tabs,
    isPrimary,
    scope: isPrimary ? undefined : fakeScope('b.k2.dev'),
    localCommands: isPrimary,
    projects: [project],
    activeProjectId: 'p1',
    key: isPrimary ? 'primary' : 'b',
  })
  renderInRoom(room, <WorkspacePanel />)
}

afterEach(() => {
  cleanup()
})

describe('Workspace tab header', () => {
  it('shows no display name or handle', () => {
    render(true)
    expect(screen.queryByText('Display Name')).toBeNull()
    expect(screen.queryByText('Handle')).toBeNull()
    expect(screen.queryByText('cortana-handle')).toBeNull()
  })

  it('puts View Wiki on the left and keeps the bell on the right', () => {
    render(true)
    const header = screen.getByTestId('workspace-panel-header')
    const kids = Array.from(header.children)
    expect(kids).toHaveLength(2)
    expect(kids[0].textContent).toBe('View Wiki')
    expect(kids[1]).toBe(screen.getByTestId('workspace-panel-bell'))
    expect(kids[1].className).toContain('ml-auto')
    expect(kids[1].contains(screen.getByTestId('completion-bell'))).toBe(true)
    expect(header.className).not.toContain('justify-end')
  })

  it('View Wiki opens this workspace\'s wiki directly; close returns to the page (0.46.0)', () => {
    usePageViewStore.getState().setPage('home')
    render(true)
    fireEvent.click(screen.getByText('View Wiki'))
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage]).toEqual(['wiki', '/ws/cortana', 'home'])
    s.closeWiki()
    expect(usePageViewStore.getState().page).toBe('home')
    usePageViewStore.getState().setPage('agents')
  })

  it('a room on another server keeps the bell and offers no Wiki', () => {
    render(false)
    const header = screen.getByTestId('workspace-panel-header')
    expect(Array.from(header.children)).toEqual([screen.getByTestId('workspace-panel-bell')])
    expect(screen.getByTestId('workspace-panel-bell').className).toContain('ml-auto')
    expect(screen.queryByText('View Wiki')).toBeNull()
  })
})
