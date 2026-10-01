// Home M2 — the app's pool wired into the real request layer and store.
//
//   - MS52 d / MS36 / MS59: a `daemonCli*` call on a pinned scope for
//     another server that answers 401/403 revives through the pool. With no
//     remembered password: no keychain delete, no CLI token delete, no
//     full-screen overlay; the pool says signin-required.
//   - With a remembered password: ONE login, then ONE replay of the call.
//   - MS25: at most 4 requests in flight per server (the pool's own status
//     checks share the cap); one quick retry on a connection-level failure.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const h = vi.hoisted(() => {
  const mem = new Map<string, string>()
  const storage = {
    getItem: (k: string) => (mem.has(k) ? (mem.get(k) as string) : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
    key: (i: number) => Array.from(mem.keys())[i] ?? null,
    get length() {
      return mem.size
    },
  }
  ;(globalThis as { localStorage?: unknown }).localStorage = storage
  return { mem, keychain: new Map<string, string>(), invokes: [] as Array<{ cmd: string; args: unknown }> }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    h.invokes.push({ cmd, args })
    switch (cmd) {
      case 'k2_secret_get':
        return h.keychain.get(`${args!.service}:${args!.account}`) ?? null
      case 'k2_secret_set':
        h.keychain.set(`${args!.service}:${args!.account}`, args!.secret as string)
        return null
      case 'k2_secret_delete':
        h.keychain.delete(`${args!.service}:${args!.account}`)
        return null
      default:
        return null
    }
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

import { daemonCliGet, remoteCliInflightForTests } from './daemon-cli'
import { hostPool, __resetHostPoolForTests } from './host-pool-instance'
import { scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import {
  K2_CONNECT_KEYCHAIN_SERVICE,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

const B: ConnectHost = {
  id: 'id-b',
  label: 'Box B',
  hostname: 'b.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok-old',
  remember: true,
  lastConnectedAt: null,
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status })
}

let loginPosts = 0
let validTokens: Set<string>

beforeEach(() => {
  h.mem.clear()
  h.keychain.clear()
  h.invokes.length = 0
  loginPosts = 0
  validTokens = new Set()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  __resetHostPoolForTests()
  useConnectHostStore.getState().addHost(B)
  h.keychain.set(`${K2_CONNECT_KEYCHAIN_SERVICE}:id-b`, 'tok-old')
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const u = new URL(url)
      const token = u.searchParams.get('token') ?? ''
      if (u.pathname === '/cli/auth/login') {
        loginPosts += 1
        validTokens.add('tok-new')
        return json(200, { token: 'tok-new', username: 'rosson', expiresAt: '2026-10-07T00:00:00Z', mustChangePassword: false })
      }
      if (!validTokens.has(token)) return json(403, { error: 'Invalid or missing auth token' })
      if (u.pathname === '/cli/auth/whoami') return json(200, { username: 'rosson', owner: false, role: 'member' })
      if (u.pathname === '/cli/agents/list') return json(200, { agents: ['anna'] })
      throw new Error(`unexpected ${init?.method ?? 'GET'} ${url}`)
    }),
  )
})

const DELETES = ['k2_secret_delete', 'connect_cli_token_delete']

describe('a pinned scope’s refused request (MS52 d)', () => {
  it('no remembered password: no delete of any saved login, no overlay, state signin-required', async () => {
    await expect(daemonCliGet(scopeForHost('b.k2.dev'), 'agents/list')).rejects.toThrow('Invalid or missing auth token')
    expect(h.invokes.filter((i) => DELETES.includes(i.cmd))).toEqual([])
    expect(h.keychain.get(`${K2_CONNECT_KEYCHAIN_SERVICE}:id-b`)).toBe('tok-old')
    const s = useConnectHostStore.getState()
    expect(s.pendingSignIn).toBeNull()
    expect(s.recovery).toEqual({ kind: 'connected' })
    expect(hostPool.entry('b.k2.dev')?.auth).toBe('signin-required')
    expect(loginPosts).toBe(0)
  })

  it('a remembered password: one login through the pool, then one replay that succeeds', async () => {
    h.keychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:id-b`, 'pw')
    const out = await daemonCliGet<{ agents: string[] }>(scopeForHost('b.k2.dev'), 'agents/list')
    expect(out).toEqual({ agents: ['anna'] })
    expect(loginPosts).toBe(1)
    expect(useConnectHostStore.getState().hosts[0]!.token).toBe('tok-new')
    expect(hostPool.entry('b.k2.dev')?.auth).toBe('ok')
    expect(h.invokes.filter((i) => DELETES.includes(i.cmd))).toEqual([])
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
  })
})

describe('request cap and retry (MS25, MS23)', () => {
  it('at most 4 in flight per server; the rest wait their turn', async () => {
    validTokens.add('tok-old')
    let inflight = 0
    let peak = 0
    const releases: Array<() => void> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        inflight += 1
        peak = Math.max(peak, inflight)
        await new Promise<void>((r) => releases.push(r))
        inflight -= 1
        return json(200, { ok: true })
      }),
    )
    const calls = Array.from({ length: 10 }, () => daemonCliGet(scopeForHost('b.k2.dev'), 'agents/list'))
    await vi.waitFor(() => expect(releases).toHaveLength(4))
    expect(remoteCliInflightForTests('b.k2.dev')).toBe(4)
    while (releases.length > 0 || inflight > 0) {
      const r = releases.shift()
      if (!r) {
        await new Promise((x) => setTimeout(x, 0))
        continue
      }
      r()
      await new Promise((x) => setTimeout(x, 0))
      expect(remoteCliInflightForTests('b.k2.dev')).toBeLessThanOrEqual(4)
    }
    await Promise.all(calls)
    expect(peak).toBe(4)
    expect(remoteCliInflightForTests('b.k2.dev')).toBe(0)
  })

  it('one quick retry on a connection-level failure, and only one', async () => {
    validTokens.add('tok-old')
    let n = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        n += 1
        if (n === 1) throw new TypeError('Load failed')
        return json(200, { agents: [] })
      }),
    )
    await expect(daemonCliGet(scopeForHost('b.k2.dev'), 'agents/list')).resolves.toEqual({ agents: [] })
    expect(n).toBe(2)
    n = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        n += 1
        throw new TypeError('Load failed')
      }),
    )
    await expect(daemonCliGet(scopeForHost('b.k2.dev'), 'agents/list')).rejects.toThrow('Load failed')
    expect(n).toBe(2)
  })

  it('the pool’s status check shares the same per-server slots', async () => {
    validTokens.add('tok-old')
    const releases: Array<() => void> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        await new Promise<void>((r) => releases.push(r))
        if (url.endsWith('/boot-status')) return json(200, { phase: 'ready', version: '0.41.6', protocol: 1, instanceId: 'i-b' })
        return json(200, { username: 'rosson', role: 'member', workspaces: [] })
      }),
    )
    const calls = Array.from({ length: 4 }, () => daemonCliGet(scopeForHost('b.k2.dev'), 'agents/list'))
    await vi.waitFor(() => expect(releases).toHaveLength(4))
    const check = hostPool.check('b.k2.dev')
    await new Promise((r) => setTimeout(r, 0))
    // The status check waits for a slot: still 4 requests out.
    expect(releases).toHaveLength(4)
    expect(remoteCliInflightForTests('b.k2.dev')).toBe(4)
    releases.shift()!()
    await vi.waitFor(() => expect(releases).toHaveLength(4))
    while (releases.length > 0) {
      releases.shift()!()
      await new Promise((r) => setTimeout(r, 0))
    }
    await Promise.all(calls)
    const e = await check
    expect(e.reach).toBe('live')
  })
})
