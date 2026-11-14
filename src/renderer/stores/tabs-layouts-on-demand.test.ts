// Speed pass P4: the full layouts map is not part of the first load.
//
// The window used to fetch every workspace's layout (`workspace-layouts/
// load-all`) the moment the tabs store was made, beside the projects list.
// Now the shown workspace reads its own layout (`workspace-layouts/load`)
// and the rest load in the background once the first projects list has
// landed (`requestWorkspaceLayoutsLoad`, called by the projects store). A
// background load fills only what the window does not hold yet: a layout
// already read or edited here is never replaced by its older row.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const h = vi.hoisted(() => ({
  gets: [] as string[],
  rows: [] as Array<{ projectId: string; workspaceId: string; layoutJson: string; revision?: number }>,
  connected: [] as Array<() => void>,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(vi.fn(async (route: string) => {
      h.gets.push(route)
      if (route === 'workspace-layouts/load-all') return h.rows
      return []
    })),
    daemonCliPost: primaryOnly(vi.fn(async () => ({}))),
  }
})
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn((fn: () => void) => {
    h.connected.push(fn)
    return () => undefined
  }),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
  setChatSession: vi.fn(async () => undefined),
}))

import { useTabsStore } from './tabs'

const layout = (title: string): string =>
  JSON.stringify({ version: 2, tabs: [{ id: `t-${title}`, title }], activeTabId: `t-${title}` })

const loadAllCalls = (): number => h.gets.filter((r) => r === 'workspace-layouts/load-all').length

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

describe('layouts on demand (speed pass P4)', () => {
  beforeEach(() => {
    h.rows = []
  })

  // Runs first: the store was made at import.
  it('the full layouts map is not fetched when the store is made, nor on a daemon (re)connect', async () => {
    await settle()
    expect(loadAllCalls()).toBe(0)
    for (const fn of h.connected) fn()
    await settle()
    expect(loadAllCalls()).toBe(0)
  })

  it('loads once, in the background, when asked; never over a layout this window already holds', async () => {
    // The shown workspace's layout was read (and edited) here first.
    useTabsStore.setState({
      workspaceLayouts: { 'pA:wA': JSON.parse(layout('fresh')) },
    })
    h.rows = [
      { projectId: 'pA', workspaceId: 'wA', layoutJson: layout('older'), revision: 3 },
      { projectId: 'pB', workspaceId: 'wB', layoutJson: layout('other'), revision: 5 },
    ]
    useTabsStore.getState().requestWorkspaceLayoutsLoad()
    // Not in the caller's own turn: after it.
    expect(loadAllCalls()).toBe(0)
    await settle()
    expect(loadAllCalls()).toBe(1)
    const cached = useTabsStore.getState().workspaceLayouts
    expect(cached['pA:wA']?.tabs?.[0]).toMatchObject({ title: 'fresh' })
    expect(cached['pB:wB']?.tabs?.[0]).toMatchObject({ title: 'other' })

    // Asked again (another list refresh): nothing more.
    useTabsStore.getState().requestWorkspaceLayoutsLoad()
    await settle()
    expect(loadAllCalls()).toBe(1)
  })
})
