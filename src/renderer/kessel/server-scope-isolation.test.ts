// Home M1 — two servers that share a workspace path and project id never
// share a request target. The two checkouts are byte-identical
// (`/Users/z3thon/DevProjects/K2`, project `p-k2`) on purpose: z3mbp and
// z3mbpZ really do have the same path.

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

import { primaryScope, scopeForHost, __resetServerScopesForTests } from './server-scope'
import { getDaemonWs, invalidateDaemonWs } from './daemon-ws'
import {
  daemonCliGet,
  daemonCliGetText,
  daemonCliPost,
  RecoveringError,
  remoteCliInflightForTests,
} from '@/lib/daemon-cli'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'
import { reviveRemoteSession, __resetRemoteSessionForTests } from '@/lib/remote-session'

const SAME_PATH = '/Users/z3thon/DevProjects/K2'
const SAME_PROJECT = 'p-k2'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'a',
    label: 'A',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'tok-a',
    remember: false,
    lastConnectedAt: null,
    ...over,
  }
}
const A = host({})
const B = host({ id: 'b', label: 'B', hostname: 'z3thon.k2.dev', token: 'tok-b' })

function okRes(body: unknown = {}): Response {
  return {
    ok: true,
    status: 200,
    url: '',
    text: async () => JSON.stringify(body),
  } as unknown as Response
}

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  mem.clear()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  invalidateDaemonWs()
  __resetRemoteSessionForTests()
  invokeMock.mockReset()
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 50123, token: 'local-tok' }
    return null
  })
  useConnectHostStore.getState().addHost(A)
  useConnectHostStore.getState().addHost(B)
  fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => okRes({ ok: true }))
  vi.stubGlobal('fetch', fetchMock)
})

function urls(): string[] {
  return fetchMock.mock.calls.map((c) => String(c[0]))
}

describe('two scopes, same workspace path + project id', () => {
  it('daemonCliGet / Post / GetText never share a target URL or token', async () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const params = { path: SAME_PATH, project_id: SAME_PROJECT }
    await daemonCliGet(a, 'fs/read-dir', params)
    await daemonCliGet(b, 'fs/read-dir', params)
    await daemonCliPost(a, 'workspace-layouts/save', { projectId: SAME_PROJECT })
    await daemonCliPost(b, 'workspace-layouts/save', { projectId: SAME_PROJECT })
    await daemonCliGetText(a, 'timer/entries-export', params)
    await daemonCliGetText(b, 'timer/entries-export', params)
    const got = urls()
    expect(got).toHaveLength(6)
    const forA = got.filter((u) => u.startsWith('https://rosson.k2.dev/cli/'))
    const forB = got.filter((u) => u.startsWith('https://z3thon.k2.dev/cli/'))
    expect(forA).toHaveLength(3)
    expect(forB).toHaveLength(3)
    for (const u of forA) {
      expect(u).toContain('token=tok-a')
      expect(u).not.toContain('tok-b')
    }
    for (const u of forB) {
      expect(u).toContain('token=tok-b')
      expect(u).not.toContain('tok-a')
    }
    expect(new Set(got).size).toBe(6)
  })

  it('getDaemonWs resolves each scope to its own server', async () => {
    const ca = await getDaemonWs(scopeForHost(A))
    const cb = await getDaemonWs(scopeForHost(B))
    expect(ca).toEqual({ port: 443, token: 'tok-a', host: 'rosson.k2.dev', secure: true })
    expect(cb).toEqual({ port: 443, token: 'tok-b', host: 'z3thon.k2.dev', secure: true })
  })

  it('the primary scope and a pinned scope differ while the window is on another server', async () => {
    useConnectHostStore.getState().selectHost(A)
    await daemonCliGet(primaryScope(), 'fs/read-dir', { path: SAME_PATH })
    await daemonCliGet(scopeForHost(B), 'fs/read-dir', { path: SAME_PATH })
    const [p, pinned] = urls()
    expect(p.startsWith('https://rosson.k2.dev/cli/fs/read-dir?')).toBe(true)
    expect(pinned.startsWith('https://z3thon.k2.dev/cli/fs/read-dir?')).toBe(true)
  })

  it("A switching servers mid-flight does not drop a pinned request to B", async () => {
    useConnectHostStore.getState().selectHost(A)
    let release: (r: Response) => void = () => {
      throw new Error('fetch never started')
    }
    fetchMock.mockImplementationOnce(
      () =>
        new Promise<Response>((resolve) => {
          release = resolve
        }),
    )
    const pending = daemonCliGet(scopeForHost(B), 'projects/list')
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1))
    useConnectHostStore.getState().selectHost('local')
    release(okRes({ from: 'b' }))
    await expect(pending).resolves.toEqual({ from: 'b' })
  })

  it("A's recovery does not block B; it still blocks the primary scope", async () => {
    useConnectHostStore.getState().selectHost(A)
    useConnectHostStore.getState().setRecovery({ kind: 'reauthenticating' })
    await expect(daemonCliGet(primaryScope(), 'projects/list')).rejects.toBeInstanceOf(RecoveringError)
    await expect(daemonCliGet(scopeForHost(A), 'projects/list')).rejects.toBeInstanceOf(RecoveringError)
    await expect(daemonCliGet(scopeForHost(B), 'projects/list')).resolves.toEqual({ ok: true })
    expect(urls()).toEqual([expect.stringMatching(/^https:\/\/z3thon\.k2\.dev\/cli\/projects\/list\?/)])
  })

  it('the remote request cap is per server: four in flight on A leave B free', async () => {
    const releases: Array<(r: Response) => void> = []
    fetchMock.mockImplementation(
      () =>
        new Promise<Response>((resolve) => {
          releases.push(resolve)
        }),
    )
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const onA = [1, 2, 3, 4, 5].map(() => daemonCliGet(a, 'projects/list'))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(4))
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(4)
    const onB = daemonCliGet(b, 'projects/list')
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(5))
    expect(remoteCliInflightForTests('z3thon.k2.dev')).toBe(1)
    expect(String(fetchMock.mock.calls[4][0]).startsWith('https://z3thon.k2.dev/')).toBe(true)
    while (releases.length > 0) releases.shift()!(okRes({}))
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(6))
    releases.shift()!(okRes({}))
    await Promise.all([...onA, onB])
    expect(remoteCliInflightForTests('rosson.k2.dev')).toBe(0)
    expect(remoteCliInflightForTests('z3thon.k2.dev')).toBe(0)
  })
})

function deleteInvokes(): string[] {
  return invokeMock.mock.calls
    .map((c) => String(c[0]))
    .filter((cmd) => cmd === 'k2_secret_delete' || cmd === 'connect_cli_token_delete')
}

describe('G1 / MS59 / MS77 — background auth failures never delete a saved login', () => {
  it("a pinned scope's 401 makes no revive, no rotation, and deletes nothing", async () => {
    useConnectHostStore.getState().selectHost(A)
    fetchMock.mockImplementation(async (url: string) => {
      if (url.startsWith('https://z3thon.k2.dev/')) {
        return {
          ok: false,
          status: 401,
          url,
          text: async () => JSON.stringify({ error: 'Invalid or missing auth token' }),
        } as unknown as Response
      }
      throw new Error(`unexpected fetch ${url}`)
    })
    invokeMock.mockClear()
    await expect(daemonCliGet(scopeForHost(B), 'projects/list')).rejects.toThrow(
      'Invalid or missing auth token',
    )
    // Exactly the one request: no whoami probe, no login POST, no replay.
    expect(urls()).toEqual([expect.stringMatching(/^https:\/\/z3thon\.k2\.dev\/cli\/projects\/list\?/)])
    expect(deleteInvokes()).toEqual([])
    const s = useConnectHostStore.getState()
    expect(s.hosts.find((h) => h.id === 'b')!.token).toBe('tok-b')
    expect(s.pendingSignIn).toBe(null)
    expect(s.activeHost === 'local' ? 'local' : s.activeHost.id).toBe('a')
  })

  it('a failed background revive drops the token in memory only (keychain + CLI mirror kept)', async () => {
    useConnectHostStore.getState().addHost({ ...B, username: 'rosson' })
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) {
        return {
          ok: false,
          status: 403,
          url,
          text: async () => JSON.stringify({ error: 'Invalid or missing auth token' }),
        } as unknown as Response
      }
      throw new Error(`unexpected fetch ${url}`)
    })
    invokeMock.mockClear()
    // No remembered password → signin-required for this background host.
    await expect(reviveRemoteSession('b')).resolves.toBe('signin-required')
    expect(deleteInvokes()).toEqual([])
    const s = useConnectHostStore.getState()
    expect(s.hosts.find((h) => h.id === 'b')!.token).toBe('')
    expect(s.pendingSignIn).toBe(null)
    expect(s.activeHost).toBe('local')
  })

  it("the window's own server still expires in the foreground and forgets its token", async () => {
    useConnectHostStore.getState().addHost({ ...A, username: 'rosson' })
    useConnectHostStore.getState().selectHost(useConnectHostStore.getState().hosts.find((h) => h.id === 'a')!)
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) {
        return {
          ok: false,
          status: 403,
          url,
          text: async () => JSON.stringify({ error: 'Invalid or missing auth token' }),
        } as unknown as Response
      }
      throw new Error(`unexpected fetch ${url}`)
    })
    invokeMock.mockClear()
    await expect(reviveRemoteSession('a')).resolves.toBe('signin-required')
    await vi.waitFor(() => expect(deleteInvokes()).toContain('k2_secret_delete'))
    expect(useConnectHostStore.getState().pendingSignIn?.id).toBe('a')
  })
})
