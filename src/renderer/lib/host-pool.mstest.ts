// Home M2 — the connection pool against two REAL daemons (MS51 / MS74).
// Run with `bun run test:multiserver` (vitest.multiserver.config.ts spawns
// both daemons from this checkout's target/debug/k2-daemon).
//
// A is saved with its owner token (a legacy raw-token server). B is saved
// with the Connect user `anna` and NO token, her password remembered. The
// pool must keep the two apart: each answers its own boot-status and
// instanceId, B signs in with exactly one real login POST, B's token works
// on B and is refused by A, and a kick on B leaves A alone.

import { describe, it, expect, beforeAll, vi } from 'vitest'

const h = vi.hoisted(() => {
  const mem = new Map<string, string>()
  ;(globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => (mem.has(k) ? (mem.get(k) as string) : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
    key: (i: number) => Array.from(mem.keys())[i] ?? null,
    get length() {
      return mem.size
    },
  }
  return { mem, keychain: new Map<string, string>(), deletes: [] as string[] }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === 'k2_secret_set') h.keychain.set(`${args!.service}:${args!.account}`, args!.secret as string)
    if (cmd === 'k2_secret_get') return h.keychain.get(`${args!.service}:${args!.account}`) ?? null
    if (cmd === 'k2_secret_delete' || cmd === 'connect_cli_token_delete') h.deletes.push(cmd)
    return null
  }),
}))

import { createHostPool, type BootBody, type HostPool } from './host-pool'
import { createLoginCoordinator } from './host-login-coord'
import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const B_PORT = Number(env('K2_MS_B_PORT'))
const A_KEY = `127.0.0.1:${A_PORT}`
const B_KEY = `127.0.0.1:${B_PORT}`

const A: ConnectHost = {
  id: 'id-a',
  label: 'A',
  hostname: '127.0.0.1',
  port: A_PORT,
  secure: false,
  token: env('K2_MS_A_OWNER'),
  remember: false,
  lastConnectedAt: null,
}
const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: B_PORT,
  secure: false,
  username: env('K2_MS_B_USER'),
  token: '',
  remember: true,
  lastConnectedAt: null,
}

let pool: HostPool
const loginPosts: string[] = []

beforeAll(() => {
  useConnectHostStore.setState({ hosts: [A, B] })
  pool = createHostPool({
    hosts: () => useConnectHostStore.getState().hosts,
    windowHostKey: () => 'local',
    localCreds: async () => {
      throw new Error('no local daemon in this harness')
    },
    http: (_key, url, init) => fetch(url, init),
    bootStatus: async (_key, base) => {
      try {
        const res = await fetch(`${base}/boot-status`)
        return res.ok ? ((await res.json()) as BootBody) : null
      } catch {
        return null
      }
    },
    resolvePassword: async (hostId) => (hostId === 'id-b' ? env('K2_MS_B_PASSWORD') : null),
    login: async (host, password) => {
      loginPosts.push(`${host.hostname}:${host.port}`)
      return loginToHost(host, password)
    },
    dropSessionInMemory: (hostId) => useConnectHostStore.getState().dropSessionInMemory(hostId),
    coord: createLoginCoordinator({
      storage: { getItem: (k) => h.mem.get(k) ?? null, setItem: (k, v) => void h.mem.set(k, v), removeItem: (k) => void h.mem.delete(k) },
      now: () => Date.now(),
      windowId: 'ms-window',
      settle: async () => {},
    }),
    noteVersion: () => {},
    now: () => Date.now(),
  })
})

async function whoami(port: number, token: string): Promise<number> {
  return (await fetch(`http://127.0.0.1:${port}/cli/auth/whoami?token=${encodeURIComponent(token)}`)).status
}

describe('the pool against two real daemons', () => {
  it('each server answers for itself: live, its own instanceId and role', async () => {
    const a = await pool.check(A_KEY)
    expect(a.reach).toBe('live')
    expect(a.auth).toBe('ok')
    expect(a.role).toBe('owner')
    const b = await pool.check(B_KEY)
    expect(b.reach).toBe('live')
    expect(b.auth).toBe('ok')
    expect(b.role).toBe('member')
    expect(a.boot?.instanceId).toBeTruthy()
    expect(b.boot?.instanceId).toBeTruthy()
    expect(a.boot?.instanceId).not.toBe(b.boot?.instanceId)
    // B signed in with its remembered password: one real login POST, to B.
    expect(loginPosts).toEqual([B_KEY])
  })

  it('B’s fresh token works on B and is refused by A (MS32)', async () => {
    const tokB = useConnectHostStore.getState().hosts.find((x) => x.id === 'id-b')!.token
    expect(tokB.length).toBeGreaterThan(0)
    expect(await whoami(B_PORT, tokB)).toBe(200)
    expect(await whoami(A_PORT, tokB)).toBe(403)
    expect(await whoami(B_PORT, A.token)).toBe(403)
    // A second check of both posts no new login.
    await pool.check(A_KEY)
    await pool.check(B_KEY)
    expect(loginPosts).toEqual([B_KEY])
  })

  it('a kick on B (4001) leaves B signed out with no new login POST, and A untouched', async () => {
    const kick = await fetch(`http://127.0.0.1:${B_PORT}/cli/presence/kick?token=${env('K2_MS_B_OWNER')}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username: env('K2_MS_B_USER') }),
    })
    expect(kick.status).toBe(200)
    // The client's event socket would deliver this close code (the daemon
    // half is pinned in crates/k2-daemon/tests/home_two_daemons_integration).
    pool.noteSocketClose(B_KEY, 4001)
    const b = await pool.check(B_KEY)
    expect(b.auth).toBe('kicked')
    expect(loginPosts).toEqual([B_KEY])
    const a = await pool.check(A_KEY)
    expect(a.auth).toBe('ok')
    // Nothing was deleted from the keychain or the CLI token mirror.
    expect(h.deletes).toEqual([])
  })
})
