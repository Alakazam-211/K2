// @vitest-environment jsdom
// Rosson 2026-10-16: people who log in can always message agents. A Member
// on a REMOTE server gets the compose bar even with the agent's "Let agents
// on other servers message this agent" switch off and the app master off —
// that switch covers federated agents only. Fail loud — no skip.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, screen } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => {
  const scopes: string[] = []
  const daemonCliGet = vi.fn(async (scope: ServerScope, route: string) => {
    scopes.push(scope.id)
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
  const daemonCliPost = vi.fn(async () => ({ ok: true }))
  return { scopes, daemonCliGet, daemonCliPost }
})

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))

import { TerminalComposeBar } from './TerminalComposeBar'
import { useSettingsStore } from '@/stores/settings'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const WS = '/srv/ws/sales'

function project(): ProjectWithWorkspaces {
  // Only the fields the compose bar reads; the switch is explicitly OFF.
  return {
    id: 'p-sales',
    name: 'sales',
    path: WS,
    allowRemoteInstruct: 0,
  } as unknown as ProjectWithWorkspaces
}

function renderRemote(opts: { readOnly: boolean }): void {
  const room = testRoom({
    tabs: {},
    scope: fakeScope('remote.example', { remote: true }),
    isPrimary: false,
    projects: [project()],
    activeProjectId: 'p-sales',
    readOnly: opts.readOnly,
  })
  renderInRoom(room, <TerminalComposeBar sessionId="sess-remote" workspacePath={WS} sendDestination="pty" />)
}

describe('compose bar for a login on a remote server', () => {
  beforeEach(() => {
    cleanup()
    localStorage.clear()
    h.daemonCliGet.mockClear()
    h.daemonCliPost.mockClear()
    // The app master ("…message every agent") OFF as well.
    useSettingsStore.setState({ allowRemoteInstruct: false })
  })

  afterEach(() => {
    cleanup()
  })

  it('shows for a Member with the switch and the master off', () => {
    expect(useSettingsStore.getState().allowRemoteInstruct).toBe(false)
    renderRemote({ readOnly: false })
    expect(screen.getByRole('textbox')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Send message' })).toBeTruthy()
  })

  it('still hides in a view-only room', () => {
    renderRemote({ readOnly: true })
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Send message' })).toBeNull()
  })
})
