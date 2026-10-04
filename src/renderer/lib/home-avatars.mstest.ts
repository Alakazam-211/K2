// Home avatars against two REAL daemons (prd-home-picker-and-remote-avatars-v1
// S3 + S4). Run with `bun run test:multiserver`.
//
// The window is on A (`local` = daemon A, owner token). B is a saved server,
// signed in as the Member `anna`. Two Home rows live on B: one workspace
// with an uploaded icon, one with none. The app's sync:
//   - lists B with anna's login and asks B's `get-icon` for the one with no
//     image (it has none: recorded as missing);
//   - puts the bytes into A's cache, which serves them back as the same
//     data URL;
//   - sends anna's token only to B and A's owner token only to A;
//   - asks nobody again on a second pass the same day.

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
import { workspaceHandle } from '@/lib/home-address'
import { homeAvatarSync, useHomeAvatarStore } from '@/lib/home-avatars'
import type { HomeRow } from '@/stores/homes'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_PORT = Number(env('K2_MS_B_PORT'))
const B_OWNER = env('K2_MS_B_OWNER')
const B_KEY = `127.0.0.1:${B_PORT}`

// A real 1×1 PNG, so A's magic-byte check passes.
const PNG =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=='

const DIR_ICON = mkdtempSync(join(tmpdir(), 'k2-ms-avatar-icon-'))
const DIR_PLAIN = mkdtempSync(join(tmpdir(), 'k2-ms-avatar-plain-'))

async function on<T>(port: number, owner: string, route: string, body?: unknown): Promise<T> {
  const res = await realFetch(
    `http://127.0.0.1:${port}/cli/${route}${route.includes('?') ? '&' : '?'}token=${encodeURIComponent(owner)}`,
    body === undefined
      ? undefined
      : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) },
  )
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on :${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

interface Listed {
  id: string
  name: string
  path: string
  handle?: string | null
}

async function register(path: string): Promise<Listed> {
  await on(B_PORT, B_OWNER, 'projects/add-without-git', { path, seedWiki: false, seedAgentsMd: false, fanout: false })
  const list = await on<Listed[]>(B_PORT, B_OWNER, 'projects/list')
  const p = list.find((x) => x.path === path)
  if (!p) throw new Error(`project for ${path} missing on B`)
  return p
}

let iconRow: HomeRow
let plainRow: HomeRow
let annaToken = ''

beforeAll(async () => {
  const withIcon = await register(DIR_ICON)
  const plain = await register(DIR_PLAIN)
  await on(B_PORT, B_OWNER, 'projects/set-icon', { projectId: withIcon.id, dataUrl: PNG })
  const hi = workspaceHandle(withIcon)
  const hp = workspaceHandle(plain)
  if (!hi || !hp) throw new Error('a workspace on B has no handle')
  iconRow = { address: `${hi}::${B_KEY}`, workspaceId: withIcon.id, label: withIcon.name }
  plainRow = { address: `${hp}::${B_KEY}`, workspaceId: plain.id, label: plain.name }
  const bHost = {
    id: 'id-b',
    label: 'B',
    hostname: '127.0.0.1',
    port: B_PORT,
    secure: false,
    username: env('K2_MS_B_USER'),
    token: '',
    remember: false,
    lastConnectedAt: null,
  } as ConnectHost
  const login = await loginToHost(bHost, env('K2_MS_B_PASSWORD'))
  if (!login.ok) throw new Error(`login on B failed: ${login.reason}`)
  annaToken = login.token
  useConnectHostStore.setState({ hosts: [{ ...bHost, token: login.token }], activeHost: 'local', connectionStatus: 'connected' } as never)
})

afterAll(() => {
  rmSync(DIR_ICON, { recursive: true, force: true })
  rmSync(DIR_PLAIN, { recursive: true, force: true })
})

describe('Home avatars across two real daemons', () => {
  it('fetches from B with B’s login, caches on A, serves the same data URL back, and asks nobody twice', async () => {
    h.requests.length = 0
    const input = {
      rows: [iconRow, plainRow],
      connectedKey: 'local',
      connectedProjects: [],
      reachable: (k: string) => k === B_KEY,
    }
    await homeAvatarSync.refresh(input)

    const fromA = await on<{ avatars: Record<string, { dataUrl: string | null; missing: boolean; fetchedAt: number; sha256: string | null }> }>(
      A_PORT,
      A_OWNER,
      `home/avatars?addresses=${encodeURIComponent(`${iconRow.address},${plainRow.address}`)}`,
    )
    expect(fromA.avatars[iconRow.address]?.dataUrl).toBe(PNG)
    expect(fromA.avatars[iconRow.address]?.sha256).toMatch(/^[0-9a-f]{64}$/)
    expect(fromA.avatars[plainRow.address]?.missing).toBe(true)
    expect(fromA.avatars[plainRow.address]?.dataUrl).toBeNull()
    expect(useHomeAvatarStore.getState().cached[iconRow.address]?.dataUrl).toBe(PNG)

    const toB = h.requests.filter((u) => u.startsWith(`http://127.0.0.1:${B_PORT}/`))
    const toA = h.requests.filter((u) => u.startsWith(`http://127.0.0.1:${A_PORT}/`))
    expect(toB.map((u) => new URL(u).pathname)).toEqual(['/cli/projects/list', '/cli/projects/get-icon'])
    for (const u of toB) expect(new URL(u).searchParams.get('token')).toBe(annaToken)
    for (const u of toA) expect(new URL(u).searchParams.get('token')).toBe(A_OWNER)
    expect(toA.map((u) => new URL(u).pathname)).toContain('/cli/home/avatars/put')
    expect(h.requests.length).toBe(toA.length + toB.length)

    // B's own Connect login can't touch A's cache.
    const refused = await realFetch(
      `http://127.0.0.1:${B_PORT}/cli/home/avatars?addresses=x::y&token=${encodeURIComponent(annaToken)}`,
    )
    expect(refused.status).toBe(403)

    // Same day: the cache is fresh, nobody is asked.
    h.requests.length = 0
    await homeAvatarSync.refresh(input)
    expect(h.requests.filter((u) => u.startsWith(`http://127.0.0.1:${B_PORT}/`))).toEqual([])

    // Prune with nothing listed keeps fresh entries (the 120 s grace).
    const pruned = await on<{ removed: number; kept: number }>(A_PORT, A_OWNER, 'home/avatars/prune', { keep: [] })
    expect(pruned).toMatchObject({ removed: 0, kept: 2 })
  })
})
