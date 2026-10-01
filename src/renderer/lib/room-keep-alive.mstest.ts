// Home M4 — keep-alive against two REAL daemons (MS39, GH#22; MS53 step 3).
// Run with `bun run test:multiserver`.
//
// The same checkout is registered on A and on B and dismissed from both
// Active sets. A room held on B (`hostPool.keepRoomAlive` → `projects/activate`
// on B's scope) puts B's workspace back in B's Active set, so B's reaper
// spares it. A's Active set does not move, and nothing goes to A's port.

import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

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
  return { mem, requests: [] as string[] }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'available', port: Number(process.env.K2_MS_A_PORT), token: process.env.K2_MS_A_OWNER }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const realFetch = globalThis.fetch
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
  h.requests.push(String(input))
  return realFetch(input, init)
}) as typeof fetch

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { scopeForHost } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { createHostPool, type BootBody, type HostPool } from './host-pool'
import { createLoginCoordinator } from './host-login-coord'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const B_PORT = Number(env('K2_MS_B_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_OWNER = env('K2_MS_B_OWNER')
const B_KEY = `127.0.0.1:${B_PORT}`

const A: ConnectHost = {
  id: 'id-a', label: 'A', hostname: '127.0.0.1', port: A_PORT, secure: false,
  token: A_OWNER, remember: false, lastConnectedAt: null,
}
const B: ConnectHost = {
  id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
  token: B_OWNER, remember: false, lastConnectedAt: null,
}

const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-keepalive-'))

async function daemon<T>(port: number, owner: string, route: string, body?: unknown): Promise<T> {
  const res = await realFetch(
    `http://127.0.0.1:${port}/cli/${route}?token=${encodeURIComponent(owner)}`,
    body === undefined
      ? undefined
      : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) },
  )
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on ${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function register(port: number, owner: string): Promise<string> {
  await daemon(port, owner, 'projects/add-without-git', {
    path: CHECKOUT, seedWiki: false, seedAgentsMd: false, fanout: false,
  })
  const list = await daemon<Array<{ id: string; path: string }>>(port, owner, 'projects/list')
  const p = list.find((x) => x.path === CHECKOUT)
  if (!p) throw new Error(`project for ${CHECKOUT} missing on ${port}`)
  return p.id
}

async function activeIds(port: number, owner: string): Promise<string[]> {
  const snap = await daemon<{ projectIds: string[] }>(port, owner, 'projects/active')
  return snap.projectIds
}

let pool: HostPool
let projA: string
let projB: string

beforeAll(async () => {
  useConnectHostStore.setState({ hosts: [A, B] })
  projA = await register(A_PORT, A_OWNER)
  projB = await register(B_PORT, B_OWNER)
  await daemon(A_PORT, A_OWNER, 'projects/dismiss', { projectId: projA })
  await daemon(B_PORT, B_OWNER, 'projects/dismiss', { projectId: projB })
  pool = createHostPool({
    hosts: () => useConnectHostStore.getState().hosts,
    windowHostKey: () => 'local',
    localCreds: async () => ({ base: `http://127.0.0.1:${A_PORT}`, token: A_OWNER }),
    http: (_key, url, init) => fetch(url, init),
    bootStatus: async (_key, base) => {
      const res = await realFetch(`${base}/boot-status`)
      return res.ok ? ((await res.json()) as BootBody) : null
    },
    resolvePassword: async () => null,
    login: async () => {
      throw new Error('no logins in this test')
    },
    dropSessionInMemory: () => {
      throw new Error('no session drops in this test')
    },
    coord: createLoginCoordinator({
      storage: { getItem: (k) => h.mem.get(k) ?? null, setItem: (k, v) => void h.mem.set(k, v), removeItem: (k) => void h.mem.delete(k) },
      now: () => Date.now(),
      windowId: 'ms-keepalive',
      settle: async () => {},
    }),
    noteVersion: () => {},
    // The production wiring (lib/host-pool-instance.ts): B's own scope.
    activate: async (hostKey, projectId) => {
      await daemonCliPost(scopeForHost(hostKey), 'projects/activate', { projectId })
    },
    now: () => Date.now(),
  })
}, 60_000)

afterAll(() => {
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe('a room held on B keeps B’s Active set (MS39)', () => {
  it('starts with the workspace dismissed on both servers', async () => {
    expect(await activeIds(A_PORT, A_OWNER)).not.toContain(projA)
    expect(await activeIds(B_PORT, B_OWNER)).not.toContain(projB)
  })

  it('keepRoomAlive on B activates it on B only, with B’s token', async () => {
    h.requests = []
    expect(await pool.keepRoomAlive(B_KEY, projB)).toBe('sent')
    const sent = [...h.requests]
    expect(sent.length).toBe(1)
    const url = new URL(sent[0])
    expect(url.port).toBe(String(B_PORT))
    expect(url.pathname).toBe('/cli/projects/activate')
    expect(url.searchParams.get('token')).toBe(B_OWNER)

    expect(await activeIds(B_PORT, B_OWNER)).toContain(projB)
    expect(await activeIds(A_PORT, A_OWNER)).not.toContain(projA)
  })

  it('a second call within 10 min posts nothing', async () => {
    h.requests = []
    expect(await pool.keepRoomAlive(B_KEY, projB)).toBe('deduped')
    expect(h.requests).toEqual([])
  })
})
