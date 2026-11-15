// @vitest-environment jsdom
// The Workspace tab mounts the Apps section right next to Published
// (Rosson 2026-11-15: "add apps just above/below the published area").
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 0) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(() => new Promise(() => undefined)),
  daemonCliPost: vi.fn(() => new Promise(() => undefined)),
}))
vi.mock('@/components/HeartbeatsPanel/HeartbeatsPanel', () => ({ HeartbeatsPanel: () => <div data-section="heartbeats" /> }))
vi.mock('./ConnectedAgentsSection', () => ({ ConnectedAgentsSection: () => <div data-section="connected" /> }))
vi.mock('./UrlsPortsSection', () => ({ UrlsPortsSection: () => <div data-section="published" /> }))
vi.mock('./AppsSection', () => ({
  AppsSection: ({ projectId }: { projectId: string }) => <div data-section="apps" data-project={projectId} />,
}))
vi.mock('./WorkspaceApiSection', () => ({ WorkspaceApiSection: () => <div data-section="api" /> }))
vi.mock('./WorkspaceCompletionSoundToggle', () => ({ WorkspaceCompletionSoundBell: () => null }))
vi.mock('./WorkspacePhonePushToggle', () => ({ WorkspacePhonePushBell: () => null }))

import WorkspacePanel from './WorkspacePanel'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { renderInRoom, testRoom } from '@/test-utils/room'

const project = {
  id: 'p1',
  name: 'cortana',
  path: '/ws/cortana',
  color: '#3b82f6',
  iconUrl: null,
  handle: 'cortana',
  agentMode: 'custom',
  workspaces: [],
} as unknown as ProjectWithWorkspaces

afterEach(() => cleanup())

describe('WorkspacePanel: Apps beside Published', () => {
  it('mounts Apps for this workspace directly above Published', () => {
    const room = testRoom({
      tabs: createStore(() => ({ openAgentPane: vi.fn() })),
      projects: [project],
      activeProjectId: 'p1',
    })
    renderInRoom(room, <WorkspacePanel />)
    const order = Array.from(document.querySelectorAll('[data-section]')).map((el) => el.getAttribute('data-section'))
    expect(order).toEqual(['heartbeats', 'connected', 'apps', 'published', 'api'])
    expect(document.querySelector('[data-section="apps"]')?.getAttribute('data-project')).toBe('p1')
  })
})
