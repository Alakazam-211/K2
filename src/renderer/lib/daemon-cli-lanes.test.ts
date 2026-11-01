// research-remote-thread-load-v1: per-request timing (Fix 0) and the
// remote request lanes (Fix 1) of the `/cli` chokepoint, on real pinned
// scopes with a scripted `fetch`.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import { invalidateDaemonWs } from '@/kessel/daemon-ws'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { __resetRemoteSessionForTests } from '@/lib/remote-session'
import {
  getCliTimings,
  lastCliActivityAt,
  resetCliTimingsForTests,
  setCliTimingLog,
  summarizeCliTimings,
} from '@/lib/cli-timing'

const A: ConnectHost = {
  id: 'a',
  label: 'A',
  hostname: 'rosson.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-a',
  remember: false,
  lastConnectedAt: null,
}

function okRes(body: unknown = {}): Response {
  return { ok: true, status: 200, url: '', text: async () => JSON.stringify(body) } as unknown as Response
}

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  mem.clear()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  invalidateDaemonWs()
  __resetRemoteSessionForTests()
  resetCliTimingsForTests()
  invokeMock.mockReset()
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 50123, token: 'local-tok' }
    return null
  })
  useConnectHostStore.getState().addHost(A)
  fetchMock = vi.fn(async () => okRes({ ok: true }))
  vi.stubGlobal('fetch', fetchMock)
})

function routeOf(call: unknown[]): string {
  const u = new URL(String(call[0]))
  return u.pathname.replace(/^\/cli\//, '')
}

describe('Fix 0: per-request timing', () => {
  it('records queued vs network time per call, route and lane, with no token or query', async () => {
    await daemonCliGet(scopeForHost(A), 'projects/list', { path: '/secret/path' })
    await daemonCliPost(scopeForHost(A), 'workspace-layouts/save', { projectId: 'p' })
    const t = getCliTimings()
    expect(t).toHaveLength(2)
    expect(t[0]).toMatchObject({ host: 'rosson.k2.dev', route: 'projects/list', method: 'GET', lane: 'normal', status: 200 })
    expect(t[1]).toMatchObject({ route: 'workspace-layouts/save', method: 'POST', status: 200 })
    for (const row of t) {
      expect(row.queuedMs).toBeGreaterThanOrEqual(0)
      expect(row.networkMs).toBeGreaterThanOrEqual(0)
      expect(row.totalMs).toBeGreaterThanOrEqual(row.networkMs)
      expect(JSON.stringify(row)).not.toContain('tok-a')
      expect(JSON.stringify(row)).not.toContain('/secret/path')
    }
    expect(lastCliActivityAt('rosson.k2.dev')).not.toBeNull()
  })

  it('a request that waited for a slot shows the wait as queued, not network', async () => {
    const releases: Array<(r: Response) => void> = []
    // The fifth answers at once once it is sent: its time is the wait.
    fetchMock.mockImplementation((url: string) =>
      url.includes('/cli/workspaces/list') ? Promise.resolve(okRes({})) : new Promise<Response>((resolve) => releases.push(resolve)),
    )
    const a = scopeForHost(A)
    const first = [1, 2, 3, 4].map(() => daemonCliGet(a, 'projects/list'))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(4))
    const fifth = daemonCliGet(a, 'workspaces/list')
    await new Promise((r) => setTimeout(r, 30))
    expect(fetchMock).toHaveBeenCalledTimes(4)
    releases.shift()!(okRes({}))
    await fifth
    while (releases.length > 0) releases.shift()!(okRes({}))
    await Promise.all(first)
    const row = getCliTimings().find((r) => r.route === 'workspaces/list')!
    expect(row.queuedMs).toBeGreaterThanOrEqual(25)
    expect(row.networkMs).toBeLessThan(row.queuedMs)
  })

  it('a thrown call is still recorded (status null), and the log is quiet unless turned on', async () => {
    const debug = vi.spyOn(console, 'debug').mockImplementation(() => {})
    fetchMock.mockImplementation(async () => {
      throw new TypeError('Load failed')
    })
    await expect(daemonCliGet(scopeForHost(A), 'projects/list')).rejects.toThrow()
    expect(getCliTimings().at(-1)).toMatchObject({ route: 'projects/list', status: null })
    expect(debug).not.toHaveBeenCalled()
    setCliTimingLog(true)
    fetchMock.mockImplementation(async () => okRes({}))
    await daemonCliGet(scopeForHost(A), 'projects/list')
    expect(debug).toHaveBeenCalledWith(expect.stringMatching(/^\[cli-timing\] GET projects\/list host=rosson\.k2\.dev lane=normal/))
    expect(mem.get('k2.debug.cliTiming')).toBe('1')
    setCliTimingLog(false)
    debug.mockRestore()
  })

  it('summarizes per route', async () => {
    await daemonCliGet(scopeForHost(A), 'projects/list')
    await daemonCliGet(scopeForHost(A), 'projects/list')
    await daemonCliGet(scopeForHost(A), 'thread', { addr: 'k2' })
    const s = summarizeCliTimings('rosson.k2.dev')
    expect(s.map((r) => [r.route, r.count])).toEqual([
      ['projects/list', 2],
      ['thread', 1],
    ])
    expect(routeOf(fetchMock.mock.calls[2])).toBe('thread')
  })
})
