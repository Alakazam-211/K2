// Zen v1 S6 — the data verbs against two REAL daemons (prd-zen-mode-v1 Z35,
// Z41, vs-live Z52, Z54). Run with `bun run test:multiserver`.
//
// The window is on A (`local` = daemon A). B is a saved server, signed in as
// the Member `anna`, and "Open agents from other servers here" is OFF.
//   - previews: one `GET /cli/thread/latest` per server, read with each
//     server's own login, and parsed from both daemons' real answers;
//   - a remote row opens in place through the bridge: B's room opens, the
//     window stays on A, and the conversation's Thread is read on B at the
//     address the Agents page's resolver gives (never `limit=0`).

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
  return { requests: [] as string[] }
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
  h.requests.push(String(input instanceof Request ? input.url : input))
  return realFetch(input, init)
}) as typeof fetch

import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useHomesStore } from '@/stores/homes'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { hostPool } from '@/lib/host-pool-instance'
import { homeRooms } from '@/stores/home-rooms'
import { workspaceHandle } from '@/lib/home-address'
import { createZenBridge, type ZenBridgeHost } from '@/lib/zen/zen-bridge'
import { BUILTIN_TEXTING_PAGE } from '@/lib/zen/zen-page'
import {
  __resetZenDataForTests,
  installZenDataVerbs,
  refreshZenPreviews,
  zenAgentRows,
  type ZenAgentRow,
} from '@/lib/zen/zen-data'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_PORT = Number(env('K2_MS_B_PORT'))
const B_OWNER = env('K2_MS_B_OWNER')
const B_USER = env('K2_MS_B_USER')
const B_KEY = `127.0.0.1:${B_PORT}`

const DIR_A = mkdtempSync(join(tmpdir(), 'k2-ms-zen-a-'))
const DIR_B = mkdtempSync(join(tmpdir(), 'k2-ms-zen-b-'))

async function on<T>(port: number, owner: string, route: string, body?: unknown): Promise<T> {
  const res = await realFetch(
    `http://127.0.0.1:${port}/cli/${route}?token=${encodeURIComponent(owner)}`,
    body === undefined
      ? undefined
      : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) },
  )
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on :${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function register(port: number, owner: string, path: string): Promise<ProjectWithWorkspaces> {
  await on(port, owner, 'projects/add-without-git', { path, seedWiki: false, seedAgentsMd: false, fanout: false })
  const list = await on<ProjectWithWorkspaces[]>(port, owner, 'projects/list')
  const p = list.find((x) => x.path === path)
  if (!p) throw new Error(`project for ${path} missing on :${port}`)
  return p
}

let aRow: { address: string; workspaceId: string; label: string }
let bRow: { address: string; workspaceId: string; label: string }
let aHandle = ''
let bHandle = ''
let uninstall: (() => void) | null = null

const bridgeHost: ZenBridgeHost = {
  homes: () => [{ id: 'zen-ms', name: 'Zen' }],
  selectedHomeId: () => 'zen-ms',
  selectHome: () => {},
  exit: () => {},
  controls: {} as never,
  page: () => BUILTIN_TEXTING_PAGE,
}

beforeAll(async () => {
  const projA = await register(A_PORT, A_OWNER, DIR_A)
  const projB = await register(B_PORT, B_OWNER, DIR_B)
  aHandle = workspaceHandle(projA) ?? ''
  bHandle = workspaceHandle(projB) ?? ''
  if (!aHandle || !bHandle) throw new Error('a workspace has no handle')
  const bHost: ConnectHost = {
    id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  } as ConnectHost
  const login = await loginToHost(bHost, env('K2_MS_B_PASSWORD'))
  if (!login.ok) throw new Error(`login as ${B_USER} on B failed: ${login.reason}`)
  useConnectHostStore.setState({
    hosts: [{ ...bHost, token: login.token }],
    activeHost: 'local',
    connectionStatus: 'connected',
  } as never)
  useRemoteRoomsPreviewStore.setState({ enabled: false })
  useProjectsStore.setState({ projects: await on<ProjectWithWorkspaces[]>(A_PORT, A_OWNER, 'projects/list') })
  aRow = { address: `${aHandle}::local`, workspaceId: projA.id, label: projA.name }
  bRow = { address: `${bHandle}::${B_KEY}`, workspaceId: projB.id, label: projB.name }
  useHomesStore.setState({ homes: [{ id: 'zen-ms', name: 'Zen', rows: [aRow, bRow] }], selectedId: 'zen-ms' })
  const e = await hostPool.check(B_KEY)
  if (e.auth !== 'ok') throw new Error(`B's pool entry is ${e.auth}`)
  __resetZenDataForTests()
  uninstall = installZenDataVerbs()
}, 60_000)

afterAll(async () => {
  uninstall?.()
  __resetZenDataForTests()
  await homeRooms.closeAll()
  rmSync(DIR_A, { recursive: true, force: true })
  rmSync(DIR_B, { recursive: true, force: true })
})

describe('Zen data verbs on two real daemons', () => {
  it('previews: one thread/latest per server, with that server’s login, parsed from both answers', async () => {
    const bEntry = hostPool.store.getState().entries[B_KEY]
    expect(bEntry?.boot?.features).toContain('thread-latest')
    h.requests.length = 0
    await refreshZenPreviews()
    const latest = h.requests.filter((u) => u.includes('/cli/thread/latest'))
    expect(latest.length).toBe(2)
    const toA = latest.filter((u) => u.startsWith(`http://127.0.0.1:${A_PORT}/`))
    const toB = latest.filter((u) => u.startsWith(`http://127.0.0.1:${B_PORT}/`))
    expect(toA.length).toBe(1)
    expect(toB.length).toBe(1)
    expect(new URL(toA[0]).searchParams.get('addrs')).toBe(aHandle)
    expect(new URL(toB[0]).searchParams.get('addrs')).toBe(bHandle)
    // B is read with anna's login, never A's owner token.
    expect(new URL(toB[0]).searchParams.get('token')).not.toBe(A_OWNER)
    expect(h.requests.some((u) => /[?&]limit=0(&|$)/.test(u))).toBe(false)

    const rows: ZenAgentRow[] = zenAgentRows()
    expect(rows.map((r) => [r.address, r.state])).toEqual([
      [aRow.address, 'ok'],
      [bRow.address, 'ok'],
    ])
    // Empty Threads on both: no preview yet. B (0.43.2+) reports activity.
    expect(rows.map((r) => r.preview)).toEqual([null, null])
    expect(rows[1].activity).toBe('idle')
    expect(rows[1].server).toBe('B')
  })

  it('a remote row opens in place through the bridge; its Thread is read on B at the resolved address', async () => {
    const bridge = createZenBridge(bridgeHost, {
      id: 'conversation',
      caps: ['agents:read', 'thread:read', 'thread:post'],
    })
    h.requests.length = 0
    await bridge.call('conversation.open', bRow.address)
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(homeRooms.store.getState().entries[bRow.address]?.phase).toBe('open')
    expect(zenAgentRows().find((r) => r.address === bRow.address)?.selected).toBe(true)
    // vs-live Z52: B's own resolver answered.
    expect(h.requests.some((u) => u.startsWith(`http://127.0.0.1:${B_PORT}/cli/sessions/list-for-workspace`))).toBe(true)
    // Nothing about B's agent went to A.
    expect(h.requests.some((u) => u.startsWith(`http://127.0.0.1:${A_PORT}/cli/sessions/list-for-workspace`))).toBe(false)

    // No pinned Chat on B yet: B itself says so (the read went to B).
    h.requests.length = 0
    await expect(bridge.call('thread.read', bRow.address, { limit: 5 }) as Promise<unknown>).rejects.toThrow(
      /no pinned Chat conversation/,
    )
    const reads = h.requests.filter((u) => u.includes('/cli/thread?'))
    expect(reads.length).toBe(1)
    expect(reads[0].startsWith(`http://127.0.0.1:${B_PORT}/`)).toBe(true)
    expect(new URL(reads[0]).searchParams.get('addr')).toBe(bHandle)
    expect(new URL(reads[0]).searchParams.get('limit')).toBe('5')
  })
})
