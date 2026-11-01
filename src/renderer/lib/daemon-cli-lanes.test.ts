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
import { daemonCliGet, daemonCliPost, isPriorityCliRoute, remoteCliInflightForTests, withHostCliSlot } from '@/lib/daemon-cli'
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

describe('Fix 1: the Thread lane', () => {
  function holdAll(): Array<{ url: string; resolve: (r: Response) => void }> {
    const held: Array<{ url: string; resolve: (r: Response) => void }> = []
    fetchMock.mockImplementation((url: string) => new Promise<Response>((resolve) => held.push({ url, resolve })))
    return held
  }

  it('Thread reads and posts are the lane; room reads are not', () => {
    for (const r of ['thread', 'thread/activity', 'thread/post', 'project-group/msg']) expect(isPriorityCliRoute(r)).toBe(true)
    for (const r of ['thread/latest', 'projects/list', 'workspace-layouts/load', 'fs/read-dir', 'auth/whoami']) {
      expect(isPriorityCliRoute(r)).toBe(false)
    }
  })

  it('a Thread read goes out at once while 4 room reads are in flight and 6 more wait', async () => {
    const held = holdAll()
    const a = scopeForHost(A)
    const room = Array.from({ length: 10 }, (_, i) => daemonCliGet(a, i % 2 ? 'fs/read-dir' : 'workspace-layouts/load'))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(4))
    const thread = daemonCliGet(a, 'thread', { addr: 'k2', limit: 25 })
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(5))
    expect(routeOf(fetchMock.mock.calls[4])).toBe('thread')
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(5)
    held[4].resolve(okRes({ items: [] }))
    await expect(thread).resolves.toEqual({ items: [] })
    expect(getCliTimings().find((t) => t.route === 'thread')).toMatchObject({ lane: 'priority' })
    // Freeing the extra slot does not let a room read past the normal cap.
    await new Promise((r) => setTimeout(r, 10))
    expect(fetchMock).toHaveBeenCalledTimes(5)
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(4)
    while (held.length > 0 || fetchMock.mock.calls.length < 11) {
      const next = held.shift()
      if (next) next.resolve(okRes({}))
      else await new Promise((r) => setTimeout(r, 1))
    }
    await Promise.all(room)
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(0)
  })

  it('the lane is capped: at most 4 + 2 in flight, and a waiting Thread call takes the next free slot before room reads', async () => {
    const held = holdAll()
    const a = scopeForHost(A)
    const room = Array.from({ length: 4 }, () => daemonCliGet(a, 'fs/read-dir'))
    const roomWaiting = daemonCliGet(a, 'projects/list')
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(4))
    const threads = Array.from({ length: 3 }, (_, i) => daemonCliPost(a, 'thread/post', { addr: 'k2', text: `m${i}` }))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(6))
    await new Promise((r) => setTimeout(r, 10))
    expect(fetchMock).toHaveBeenCalledTimes(6)
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(6)
    // A room read finishes: the third Thread post goes, not the room read
    // that waited longer.
    held[0].resolve(okRes({}))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(7))
    expect(routeOf(fetchMock.mock.calls[6])).toBe('thread/post')
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(6)
    for (let i = 1; i < held.length; i++) held[i].resolve(okRes({}))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(8))
    expect(routeOf(fetchMock.mock.calls[7])).toBe('projects/list')
    held[7].resolve(okRes({}))
    await Promise.all([...room, roomWaiting, ...threads])
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(0)
  })

  it('pool checks (withHostCliSlot) stay on the normal slots', async () => {
    const releases: Array<() => void> = []
    const a = scopeForHost(A)
    const checks = Array.from({ length: 5 }, () => withHostCliSlot(a, () => new Promise<void>((r) => releases.push(r))))
    await vi.waitFor(() => expect(releases).toHaveLength(4))
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(4)
    await daemonCliGet(a, 'thread', { addr: 'k2' })
    releases.shift()!()
    await vi.waitFor(() => expect(releases).toHaveLength(4))
    while (releases.length > 0) releases.shift()!()
    await Promise.all(checks)
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(0)
  })
})
