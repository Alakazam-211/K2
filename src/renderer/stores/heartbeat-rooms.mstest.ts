// Heartbeat S4 (prd-heartbeat-firing-v1 HB28, T-S4b) against two REAL
// daemons. Run with `bun run test:multiserver`.
//
// The window is on A (`local`). The same checkout path is a workspace on A
// and on B, each with its own heartbeat. Opening B's Home row gives a room
// whose heartbeat store reads B: its rows are B's rows, with B's daemon
// `nextFireAt` / `waitReason`, and no heartbeat request goes to A. The
// feature check is B's own version: this checkout's daemon is the
// pre-release of the version that ships `heartbeat-next-fire`, so until
// the release bump B reads as older and the row shows schedule text only
// (HB30). Either way the formatter never says "now".

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
  return { requests: [] as Array<{ url: string; method: string }> }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    // `local` (the window's server) is daemon A.
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
  h.requests.push({ url: String(input), method: (init?.method ?? 'GET').toUpperCase() })
  return realFetch(input, init)
}) as typeof fetch

import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { openHomeRow } from '@/lib/home-open'
import { homeRooms } from '@/stores/home-rooms'
import { workspaceHandle } from '@/lib/home-address'
import { heartbeatStatusText } from '@/lib/heartbeat-wait'
import { FEATURES, gte } from '@/lib/server-capabilities'
import type { PinnedRoom } from '@/stores/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { HomeRow } from '@/stores/homes'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const B_PORT = Number(env('K2_MS_B_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_OWNER = env('K2_MS_B_OWNER')
const B_USER = env('K2_MS_B_USER')
const B_KEY = `127.0.0.1:${B_PORT}`

// The same checkout path on both servers: only the server tells them apart.
const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-hb-room-'))

async function daemon<T>(port: number, owner: string, route: string, init?: { body?: unknown; query?: Record<string, string> }): Promise<T> {
  const q = new URLSearchParams({ ...(init?.query ?? {}), token: owner })
  const res = await realFetch(`http://127.0.0.1:${port}/cli/${route}?${q}`, init?.body === undefined
    ? undefined
    : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) })
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on ${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function register(port: number, owner: string): Promise<ProjectWithWorkspaces> {
  await daemon(port, owner, 'projects/add-without-git', {
    body: { path: CHECKOUT, seedWiki: false, seedAgentsMd: false, fanout: false },
  })
  const list = await daemon<ProjectWithWorkspaces[]>(port, owner, 'projects/list')
  const p = list.find((x) => x.path === CHECKOUT)
  if (!p) throw new Error(`project for ${CHECKOUT} missing on ${port}`)
  return p
}

async function addHeartbeat(port: number, owner: string, name: string, everySeconds: number): Promise<void> {
  await daemon(port, owner, 'heartbeat/add', {
    query: {
      project: CHECKOUT,
      name,
      frequency: 'hourly',
      spec: JSON.stringify({ frequency: 'hourly', every_seconds: everySeconds }),
      instructions: 'check the inbox',
    },
  })
}

interface ListRow {
  name: string
  enabled: boolean
  nextFireAt?: string | null
  waitReason?: string | null
  waitDetail?: string | null
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

let projB: ProjectWithWorkspaces
let room: PinnedRoom
/** Does B's version carry `heartbeat-next-fire`? From B's /boot-status. */
let bHasNextFire: boolean

beforeAll(async () => {
  await register(A_PORT, A_OWNER)
  projB = await register(B_PORT, B_OWNER)
  // A and B each have their own heartbeat on the same path.
  await addHeartbeat(A_PORT, A_OWNER, 'a-only-beat', 3600)
  await addHeartbeat(B_PORT, B_OWNER, 'b-only-beat', 900)
  const boot = (await (await realFetch(`http://127.0.0.1:${B_PORT}/boot-status`)).json()) as { version?: unknown }
  if (typeof boot.version !== 'string') throw new Error(`B's /boot-status has no version: ${JSON.stringify(boot)}`)
  bHasNextFire = gte(boot.version, FEATURES['heartbeat-next-fire'])

  const bHost: ConnectHost = {
    id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  }
  const login = await loginToHost(bHost, env('K2_MS_B_PASSWORD'))
  if (!login.ok) throw new Error(`login as ${B_USER} on B failed: ${login.reason}`)
  useConnectHostStore.setState({ hosts: [{ ...bHost, token: login.token }], activeHost: 'local' } as never)
  useRemoteRoomsPreviewStore.setState({ enabled: true })

  const handle = workspaceHandle(projB)
  if (!handle) throw new Error("B's workspace has no handle")
  const row: HomeRow = { address: `${handle}::${B_KEY}`, workspaceId: projB.id, label: projB.name }
  expect(openHomeRow(row)).toBe('room')
  await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
  const opened = homeRooms.store.getState().entries[row.address]?.room
  if (!opened) throw new Error('the room opened without a room')
  room = opened
}, 60_000)

afterAll(async () => {
  await homeRooms.closeAll()
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe("B's room shows B's heartbeat truth (Heartbeat S4, HB28)", () => {
  it("the room's scope is B and its feature check is B's version", () => {
    expect(room.scope.hostKey).toBe(B_KEY)
    expect(room.scope.serverSupports('heartbeat-next-fire')).toBe(bHasNextFire)
  })

  it("loads B's rows through B, with B's nextFireAt and waitReason", async () => {
    const onB = await daemon<ListRow[]>(B_PORT, B_OWNER, 'heartbeat/list', { query: { project: CHECKOUT } })
    const bRow = onB.find((r) => r.name === 'b-only-beat')
    if (!bRow) throw new Error("B's heartbeat is missing on B")
    if (!bRow.nextFireAt) throw new Error(`B's daemon has no nextFireAt: ${JSON.stringify(bRow)}`)
    if (!bRow.waitReason) throw new Error(`B's daemon has no waitReason: ${JSON.stringify(bRow)}`)

    h.requests = []
    await room.heartbeats.getState().refresh(CHECKOUT)
    const s = room.heartbeats.getState()
    expect(s.lastError).toBeNull()
    expect(s.loadedFor).toBe(CHECKOUT)
    expect(s.active.map((e) => e.row.name)).toEqual(['b-only-beat'])
    const shown = s.active[0].row
    expect(shown.nextFireAt).toBe(bRow.nextFireAt)
    expect(shown.waitReason).toBe(bRow.waitReason)

    const hbRequests = h.requests.filter((r) => r.url.includes('/cli/heartbeat/'))
    expect(hbRequests.length).toBeGreaterThan(0)
    expect(hbRequests.every((r) => new URL(r.url).port === String(B_PORT))).toBe(true)
  })

  it("the formatter renders B's next fire, never now", () => {
    const shown = room.heartbeats.getState().active[0].row
    // B's daemon decides the reason: a fresh workspace with no agent waits
    // as `no_agent`; with one it is `scheduled` 900 s after creation.
    const withB = heartbeatStatusText(shown, { now: Date.now(), nextFire: true })
    if (withB === null) throw new Error("expected status text for B's row")
    expect(withB).not.toMatch(/\bnow\b|0m 0s|0m 00s/)
    if (shown.waitReason === 'scheduled') expect(withB).toMatch(/^in 1[45]m \d\ds$/)
    else expect(withB).toBe('waiting: no agent in this workspace')

    // What the room renders today, by B's version (HB30 when older).
    const asRoom = heartbeatStatusText(shown, { now: Date.now(), nextFire: room.scope.serverSupports('heartbeat-next-fire') })
    if (bHasNextFire) expect(asRoom).toBe(withB)
    else if (shown.waitReason === 'scheduled') expect(asRoom).toBeNull()
    else expect(asRoom).toBe(withB)
  })
})
