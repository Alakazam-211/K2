// Home P1 — a server switch never changes a Home (R1 / vs-live H19 a, b),
// and a Home row opened on another server wins over that server's
// last-session restore (H19 g). Drives the REAL connect-host, projects,
// page-view, and homes modules; only the daemon/Tauri boundaries are mocked
// (host-switch-reset.test.ts idiom).

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
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/lib/is-web', () => ({
  isWebClient: () => false,
}))

class MemoryStorage {
  private map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
  removeItem(k: string): void {
    this.map.delete(k)
  }
  clear(): void {
    this.map.clear()
  }
  key(i: number): string | null {
    return Array.from(this.map.keys())[i] ?? null
  }
  get length(): number {
    return this.map.size
  }
}
vi.stubGlobal('localStorage', new MemoryStorage())
vi.stubGlobal('sessionStorage', new MemoryStorage())

const h = vi.hoisted(() => ({
  remoteProjects: [
    { id: 'pa', name: 'Alpha', path: '/srv/alpha', handle: 'alpha', workspaces: [{ id: 'wa', tabOrder: 0 }] },
    { id: 'pb', name: 'Bee', path: '/srv/bee', handle: 'bee', workspaces: [{ id: 'wb', tabOrder: 0 }] },
  ],
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: vi.fn(primaryOnly(async (route: string) => {
      if (route === 'projects/list') return h.remoteProjects
      return []
    })),
    daemonCliPost: vi.fn(primaryOnly(async () => ({}))),
  }
})
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ lastActiveProjectId: 'pa', lastActiveWorkspaceId: 'wa' })),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))

import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from './connect-host'
import { useProjectsStore } from './projects'
import { useSettingsStore } from './settings'
import { usePageViewStore } from './page-view'
import { useHomesStore, HOMES_STORAGE_KEY } from './homes'
import { requestHostSelect, clearHostSelect, peekHostSelect } from '@/lib/home-pending-select'

function remote(): ConnectHost {
  return {
    id: 'host-b',
    label: 'Box B',
    hostname: 'b.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'session-token',
    remember: false,
    lastConnectedAt: null,
  }
}

async function waitFor(pred: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (pred()) return
    await new Promise((r) => setTimeout(r, 5))
  }
  throw new Error(`timed out waiting for ${what}`)
}

describe('Homes survive a server switch', () => {
  beforeEach(() => {
    __resetConnectHostStoreForTests()
    clearHostSelect()
  })

  it('Homes, the selected Home, and the Home page are unchanged by selectHost', () => {
    const st = useHomesStore.getState()
    const second = st.createHome('Work') as string
    expect(st.addRow(second, { address: 'bee::b.k2.dev', workspaceId: 'pb', label: 'Bee' })).toBe(true)
    expect(useHomesStore.getState().addRow(second, { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' })).toBe(true)
    usePageViewStore.getState().setPage('home')

    const before = JSON.stringify(useHomesStore.getState().homes)
    const storedBefore = localStorage.getItem(HOMES_STORAGE_KEY)
    const selectedBefore = useHomesStore.getState().selectedId
    expect(selectedBefore).toBe(second)

    useConnectHostStore.getState().selectHost(remote())
    expect(JSON.stringify(useHomesStore.getState().homes)).toBe(before)
    expect(useHomesStore.getState().selectedId).toBe(selectedBefore)
    expect(localStorage.getItem(HOMES_STORAGE_KEY)).toBe(storedBefore)
    expect(usePageViewStore.getState().page).toBe('home')

    useConnectHostStore.getState().selectHost('local')
    expect(JSON.stringify(useHomesStore.getState().homes)).toBe(before)
    expect(useHomesStore.getState().selectedId).toBe(selectedBefore)
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('without a Home pick, the new server restores its last session (control)', async () => {
    useSettingsStore.setState({ loaded: true, lastActiveProjectId: 'pa', lastActiveWorkspaceId: 'wa' })
    useConnectHostStore.getState().selectHost(remote())
    await waitFor(() => useProjectsStore.getState().activeProjectId !== null, 'restore')
    expect(useProjectsStore.getState().activeProjectId).toBe('pa')
  })

  it('a Home pick for that server wins over the restore, then goes to Agents', async () => {
    useSettingsStore.setState({ loaded: true, lastActiveProjectId: 'pa', lastActiveWorkspaceId: 'wa' })
    usePageViewStore.getState().setPage('home')
    const onSelected = vi.fn(() => usePageViewStore.getState().setPage('agents'))
    requestHostSelect('host-b', { address: 'bee::b.k2.dev', workspaceId: 'pb' }, onSelected)

    useConnectHostStore.getState().selectHost(remote())
    await waitFor(() => useProjectsStore.getState().activeProjectId !== null, 'restore')
    expect(useProjectsStore.getState().activeProjectId).toBe('pb')
    expect(useProjectsStore.getState().activeWorkspaceId).toBe('wb')
    expect(onSelected).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(peekHostSelect()).toBeNull()
  })

  it('a pick for a DIFFERENT server is left alone by this restore', async () => {
    useSettingsStore.setState({ loaded: true, lastActiveProjectId: 'pa', lastActiveWorkspaceId: 'wa' })
    requestHostSelect('host-c', { address: 'bee::c.k2.dev', workspaceId: 'pb' })
    useConnectHostStore.getState().selectHost(remote())
    await waitFor(() => useProjectsStore.getState().activeProjectId !== null, 'restore')
    expect(useProjectsStore.getState().activeProjectId).toBe('pa')
    expect(peekHostSelect()?.hostId).toBe('host-c')
  })
})
