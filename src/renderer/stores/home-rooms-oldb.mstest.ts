// Home M5 — a view-only room on a RELEASED B (up to 0.41.6) never starts a
// session there. Run with `bun run test:multiserver`.
//
// Released daemons ignore unknown body fields, so `sessions/v2/spawn` with
// `attach_only: true` SPAWNS on them. This file runs a third daemon, "old B",
// under the debug-build hook `K2_TEST_SIMULATE_NO_ATTACH_ONLY=1`: it does not
// report `spawn-attach-only` in `/boot-status` and its spawn ignores
// `attach_only`, exactly like a released one. Its layout reads also lose
// `with_revision` (the fetch below), so the room opens view only, as on a
// released B. The window is on A (`local`).
//
//   - the control: old B really does spawn on an `attach_only` request;
//   - the room's scope reads `spawn-attach-only` as unsupported on old B and
//     supported on the current B;
//   - for every tab in the view-only room whose session is not live, the
//     pane's spawn plan is `not-live`: no spawn request reaches old B and no
//     session appears there;
//   - a tab whose session IS live on old B attaches to it (reused, nothing new).
//
// 0.43.2 floor (prd-home-seamless-0432 Z5, Z42; T1.3), against the same real
// old B, with the setting on:
//   - the view-only room's bar offers "Switch to Old B", with B's real
//     version in the copy;
//   - old B reporting a version below the floor (its `/boot-status` version
//     rewritten below) makes `openHomeRow` return `switching` and call
//     `pickHost` once, with one toast; no room and no request for a room
//     reach old B;
//   - old B reporting no version does the same;
//   - a row opened before the pool knew old B's version opens a pending
//     room; when the read lands below the floor the room is gone and the
//     window switches once (Z42).

import { describe, it, expect, beforeAll, afterAll, afterEach, vi } from 'vitest'
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
  return {
    requests: [] as Array<{ url: string; method: string; body: string | null; status: number | null }>,
    /** Old B's port: its layout reads lose `with_revision`. `bootVersion`
     *  rewrites its `/boot-status` version (a string, or null = no version);
     *  undefined = old B's own answer. */
    oldB: { port: 0, bootVersion: undefined as string | null | undefined },
  }
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
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  let url = String(input)
  if (h.oldB.port !== 0 && url.includes(`:${h.oldB.port}/cli/workspace-layouts/load`)) {
    const u = new URL(url)
    u.searchParams.delete('with_revision')
    url = u.toString()
  }
  const rec = {
    url,
    method: (init?.method ?? 'GET').toUpperCase(),
    body: typeof init?.body === 'string' ? init.body : null,
    status: null as number | null,
  }
  h.requests.push(rec)
  const res = await realFetch(url, init)
  rec.status = res.status
  if (h.oldB.port !== 0 && h.oldB.bootVersion !== undefined && url.endsWith(`:${h.oldB.port}/boot-status`)) {
    const body = (await res.json()) as Record<string, unknown>
    if (h.oldB.bootVersion === null) delete body.version
    else body.version = h.oldB.bootVersion
    return new Response(JSON.stringify(body), { status: res.status, headers: { 'Content-Type': 'application/json' } })
  }
  return res
}) as typeof fetch

import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { openHomeRow } from '@/lib/home-open'
import { homeRooms } from '@/stores/home-rooms'
import { workspaceHandle } from '@/lib/home-address'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { scopeForHost } from '@/kessel/server-scope'
import { hostPool } from '@/lib/host-pool-instance'
import { planRoomSpawn } from '@/lib/room-spawn'
import { roomAccessCopy } from '@/components/Home/room/HomeRemoteRooms'
import { HOME_ROOM_FLOOR } from '@/lib/home-room-floor'
import { __resetOldServerToastsForTests } from '@/lib/home-switch'
import { clearHostSelect, peekHostSelect } from '@/lib/home-pending-select'
import { useToastStore } from '@/stores/toast'
import type { PinnedRoom } from '@/stores/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { HomeRow } from '@/stores/homes'
import type { TerminalItemData } from '@/stores/tabs'
import { seedMember, spawnDaemon, type Spawned } from '@/test-utils/two-daemons.global-setup'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const B_PORT = Number(env('K2_MS_B_PORT'))
const B_USER = env('K2_MS_B_USER')
const B_PASSWORD = env('K2_MS_B_PASSWORD')

const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-home-oldb-'))

let oldB: Spawned
let OLD_KEY = ''
let proj: ProjectWithWorkspaces
let row: HomeRow
let room: PinnedRoom

async function daemon<T>(port: number, owner: string, route: string, init?: { body?: unknown; query?: Record<string, string> }): Promise<T> {
  const q = new URLSearchParams({ ...(init?.query ?? {}), token: owner })
  const res = await realFetch(`http://127.0.0.1:${port}/cli/${route}?${q}`, init?.body === undefined
    ? undefined
    : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) })
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on ${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function bootFeatures(port: number): Promise<unknown> {
  const res = await realFetch(`http://127.0.0.1:${port}/boot-status`)
  return ((await res.json()) as { features?: unknown }).features
}

async function sessionsOnOldB(): Promise<string[]> {
  const rows = await daemon<Array<{ agentName: string }>>(oldB.port, oldB.owner, 'sessions/list-for-workspace', {
    query: { path: CHECKOUT },
  })
  return rows.map((r) => r.agentName)
}

/** `{version:2}` layout whose terminal items carry their terminal ids. */
function layoutJson(tabIds: string[]): string {
  return JSON.stringify({
    version: 2,
    tabs: tabIds.map((id) => ({
      id,
      title: id,
      mosaicTree: `pg-${id}`,
      paneGroups: {
        [`pg-${id}`]: {
          id: `pg-${id}`,
          items: [{ id: `item-${id}`, type: 'terminal', paneGroupId: `pg-${id}`, data: { terminalId: `term-${id}`, cwd: CHECKOUT, renderer: 'kessel' } }],
          activeItemIndex: 0,
        },
      },
      locked: false,
    })),
  })
}

/** The agent name each terminal pane in the room spawns / attaches as. */
function paneAgents(r: PinnedRoom): string[] {
  const out: string[] = []
  for (const tab of r.tabs.getState().tabs) {
    if (tab.isSystemAgent) continue
    for (const group of tab.paneGroups.values()) {
      for (const item of group.items) {
        if (item.type !== 'terminal') continue
        const id = (item.data as TerminalItemData | undefined)?.terminalId
        if (!id) throw new Error(`terminal item ${item.id} in the room has no terminal id`)
        out.push(`tab-${id}`)
      }
    }
  }
  return out
}

function spawnsToOldB(): typeof h.requests {
  return h.requests.filter((r) => r.url.includes(`:${oldB.port}/cli/sessions/v2/spawn`))
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 15_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

beforeAll(async () => {
  oldB = await spawnDaemon('oldb', { K2_TEST_SIMULATE_NO_ATTACH_ONLY: '1' })
  await seedMember(oldB)
  h.oldB.port = oldB.port
  OLD_KEY = `127.0.0.1:${oldB.port}`

  await daemon(oldB.port, oldB.owner, 'projects/add-without-git', {
    body: { path: CHECKOUT, seedWiki: false, seedAgentsMd: false, fanout: false },
  })
  const list = await daemon<ProjectWithWorkspaces[]>(oldB.port, oldB.owner, 'projects/list')
  const p = list.find((x) => x.path === CHECKOUT)
  if (!p || !p.workspaces || p.workspaces.length === 0) throw new Error(`no workspace for ${CHECKOUT} on old B`)
  proj = p
  await daemon(oldB.port, oldB.owner, 'workspace-layouts/save', {
    body: { projectId: proj.id, workspaceId: proj.workspaces[0].id, layoutJson: layoutJson(['old1', 'old2']) },
  })

  const oldHost: ConnectHost = {
    id: 'id-oldb', label: 'Old B', hostname: '127.0.0.1', port: oldB.port, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  }
  const bHost: ConnectHost = {
    id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  }
  const loginOld = await loginToHost(oldHost, B_PASSWORD)
  if (!loginOld.ok) throw new Error(`login on old B failed: ${loginOld.reason}`)
  const loginB = await loginToHost(bHost, B_PASSWORD)
  if (!loginB.ok) throw new Error(`login on B failed: ${loginB.reason}`)
  useConnectHostStore.setState({
    hosts: [{ ...oldHost, token: loginOld.token }, { ...bHost, token: loginB.token }],
    activeHost: 'local',
  } as never)
  useRemoteRoomsPreviewStore.setState({ enabled: true })

  const handle = workspaceHandle(proj)
  if (!handle) throw new Error("old B's workspace has no handle")
  row = { address: `${handle}::${OLD_KEY}`, workspaceId: proj.id, label: proj.name }
  expect(openHomeRow(row)).toBe('room')
  await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
  const opened = homeRooms.store.getState().entries[row.address]?.room
  if (!opened) throw new Error('the room opened without a room')
  await until(() => opened.tabs.getState().activeWorkspaceKey !== null, "old B's layout to load")
  room = opened
}, 60_000)

afterAll(async () => {
  await homeRooms.closeAll()
  oldB?.child.kill('SIGKILL')
  if (oldB) rmSync(oldB.home, { recursive: true, force: true })
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe('a view-only room on a released B (no spawn-attach-only) never starts a session there (Home M5)', () => {
  it('the control: old B does not report the key, and SPAWNS on an attach_only request', async () => {
    // Scoped to the key under test: other reported keys (tickets-list-all)
    // are not part of the attach-only simulation.
    expect(await bootFeatures(oldB.port)).not.toContain('spawn-attach-only')
    expect(await bootFeatures(B_PORT)).toContain('spawn-attach-only')
    const res = await realFetch(`http://127.0.0.1:${oldB.port}/cli/sessions/v2/spawn?token=${oldB.owner}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ agent_name: 'tab-control', cwd: CHECKOUT, command: null, args: null, cols: 80, rows: 24, attach_only: true }),
    })
    const text = await res.text()
    if (!res.ok) throw new Error(`control spawn on old B: ${res.status} ${text}`)
    const spawn = JSON.parse(text) as { reused: boolean; attachOnly?: boolean }
    expect(spawn.reused).toBe(false)
    expect(spawn.attachOnly).toBe(undefined)
    await until(async () => (await sessionsOnOldB()).includes('tab-control'), 'the control session on old B')
    await daemon(oldB.port, oldB.owner, 'sessions/v2/close', { body: { agent_name: 'tab-control', force: true, reason: 'tab_close' } })
    await until(async () => !(await sessionsOnOldB()).includes('tab-control'), 'the control session to end')
  })

  it('the room is view only, and its scope reads spawn-attach-only as unsupported (current B: supported)', async () => {
    const entry = homeRooms.store.getState().entries[row.address]
    if (!entry) throw new Error('no entry for the row')
    expect(entry.access).toBe('view-older-server')
    expect(room.readOnly).toBe(true)
    expect(room.scope.viewOnly).toBe(true)
    expect(room.scope.hostKey).toBe(OLD_KEY)
    expect(room.scope.serverSupports('spawn-attach-only')).toBe(false)
    await hostPool.check(`127.0.0.1:${B_PORT}`)
    expect(scopeForHost(`127.0.0.1:${B_PORT}`).serverSupports('spawn-attach-only')).toBe(true)
  })

  it('no tab that is not live on old B gets a spawn: the plan is not-live and nothing reaches old B', async () => {
    const agents = paneAgents(room)
    expect(agents).toEqual(['tab-pg-old1', 'tab-pg-old2'])
    const before = await sessionsOnOldB()
    for (const agent of agents) expect(before).not.toContain(agent)
    const mark = h.requests.length
    for (const agent of agents) {
      expect([agent, await planRoomSpawn(room, agent, CHECKOUT)]).toEqual([agent, { kind: 'not-live' }])
    }
    expect(spawnsToOldB()).toEqual([])
    expect(await sessionsOnOldB()).toEqual(before)
    // The live read went to old B, on the room's scope.
    const lists = h.requests.slice(mark).filter((r) => r.url.includes('/cli/sessions/list-for-workspace'))
    expect(lists.length).toBe(agents.length)
    for (const l of lists) expect(new URL(l.url).port).toBe(String(oldB.port))
  })

  it('a tab whose session IS live on old B attaches to it: reused, nothing new started', async () => {
    const agent = paneAgents(room)[0]
    const started = await daemon<{ sessionId: string; reused: boolean }>(oldB.port, oldB.owner, 'sessions/v2/spawn', {
      body: { agent_name: agent, cwd: CHECKOUT, command: 'sh', args: [], cols: 80, rows: 24 },
    })
    expect(started.reused).toBe(false)
    await until(async () => (await sessionsOnOldB()).includes(agent), 'the live session on old B')
    const before = await sessionsOnOldB()

    const plan = await planRoomSpawn(room, agent, CHECKOUT)
    expect(plan).toEqual({ kind: 'spawn', attachOnly: true })
    // The pane's spawn, as TerminalPane sends it for that plan.
    const creds = await getDaemonWs(room.scope)
    expect(creds.port).toBe(oldB.port)
    const res = await fetch(`${daemonHttpBase(creds)}/cli/sessions/v2/spawn?token=${encodeURIComponent(creds.token)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ agent_name: agent, cwd: CHECKOUT, command: null, args: null, cols: 80, rows: 24, attach_only: true }),
    })
    const text = await res.text()
    if (!res.ok) throw new Error(`attach on old B: ${res.status} ${text}`)
    const attached = JSON.parse(text) as { sessionId: string; reused: boolean }
    expect(attached.reused).toBe(true)
    expect(attached.sessionId).toBe(started.sessionId)
    expect(await sessionsOnOldB()).toEqual(before)
    // The other tab is still not live, and still gets no spawn.
    const other = paneAgents(room)[1]
    expect(await planRoomSpawn(room, other, CHECKOUT)).toEqual({ kind: 'not-live' })
    expect(spawnsToOldB().length).toBe(1)
    expect(await sessionsOnOldB()).not.toContain(other)
    await daemon(oldB.port, oldB.owner, 'sessions/v2/close', { body: { agent_name: agent, force: true, reason: 'tab_close' } })
  })
})

describe('the 0.43.2 floor against a real old B (Z5, Z42; T1.3)', () => {
  let pickHost: ReturnType<typeof vi.fn>

  function requestsForRoomOnOldB(mark: number): typeof h.requests {
    return h.requests
      .slice(mark)
      .filter((r) => r.url.includes(`:${oldB.port}/cli/`) && !r.url.includes('/cli/auth/') && !r.url.includes('/cli/presence/'))
  }

  function toasts(): string[] {
    return useToastStore.getState().toasts.map((t) => t.message)
  }

  afterEach(async () => {
    h.oldB.bootVersion = undefined
    clearHostSelect()
    await homeRooms.closeAll()
  })

  it('the view-only room on old B offers "Switch to Old B", naming its real version', async () => {
    const entry = homeRooms.store.getState().entries[row.address]
    if (!entry) throw new Error('no entry for the row')
    const version = hostPool.entry(OLD_KEY)?.boot?.version
    if (!version) throw new Error('the pool has no version for old B')
    const copy = roomAccessCopy(entry.access, room.scope.label, version)
    expect(copy.chip).toBe('View only')
    expect(copy.switchLabel).toBe('Switch to Old B')
    expect(copy.title).toBe(`Old B runs K2 ${version}, which can’t save this room’s tabs safely.`)
  })

  it('old B below the floor: the row switches the window once, with one toast, and no room reaches old B', async () => {
    await homeRooms.closeAll()
    __resetOldServerToastsForTests()
    useToastStore.setState({ toasts: [] })
    pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost } as never)
    h.oldB.bootVersion = '0.40.150'
    await hostPool.check(OLD_KEY)
    expect(hostPool.entry(OLD_KEY)?.boot?.version).toBe('0.40.150')
    expect(HOME_ROOM_FLOOR).toBe('0.41.0')

    const mark = h.requests.length
    expect(openHomeRow(row)).toBe('switching')
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect((pickHost.mock.calls[0][0] as ConnectHost).port).toBe(oldB.port)
    expect(peekHostSelect()).toMatchObject({ hostId: 'id-oldb', row: { address: row.address } })
    expect(homeRooms.store.getState().entries[row.address]).toBe(undefined)
    expect(toasts()).toEqual(['Old B runs K2 0.40.150. It opens by switching this window. Update it to open it here.'])
    await new Promise((r) => setTimeout(r, 200))
    expect(requestsForRoomOnOldB(mark)).toEqual([])
    expect(useConnectHostStore.getState().activeHost).toBe('local')
  })

  it('old B with no version: the same switch', async () => {
    __resetOldServerToastsForTests()
    useToastStore.setState({ toasts: [] })
    pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost } as never)
    h.oldB.bootVersion = null
    await hostPool.check(OLD_KEY)
    expect(hostPool.entry(OLD_KEY)?.boot?.version).toBe(null)
    const mark = h.requests.length
    expect(openHomeRow(row)).toBe('switching')
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(toasts()).toEqual(['Old B runs an older K2. It opens by switching this window. Update it to open it here.'])
    expect(homeRooms.store.getState().entries[row.address]).toBe(undefined)
    expect(requestsForRoomOnOldB(mark)).toEqual([])
  })

  it('Z42: opened before the pool knew old B, then the read lands below the floor: the room is gone and the window switches once', async () => {
    __resetOldServerToastsForTests()
    useToastStore.setState({ toasts: [] })
    pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost } as never)
    h.oldB.bootVersion = '0.40.150'
    hostPool.forget(OLD_KEY)
    expect(hostPool.entry(OLD_KEY)?.boot ?? null).toBe(null)

    const mark = h.requests.length
    expect(openHomeRow(row)).toBe('room')
    expect(pickHost).not.toHaveBeenCalled()
    await until(() => pickHost.mock.calls.length === 1, 'the floor fallback to switch the window')
    expect(homeRooms.store.getState().entries[row.address]).toBe(undefined)
    expect(homeRooms.store.getState().shown).toBe(null)
    expect(peekHostSelect()).toMatchObject({ hostId: 'id-oldb', row: { address: row.address } })
    expect(toasts()).toEqual(['Old B runs K2 0.40.150. It opens by switching this window. Update it to open it here.'])
    // Old B answered its boot-status read only: no list, no layout, no socket.
    const toOldB = requestsForRoomOnOldB(mark)
    expect(toOldB).toEqual([])
    expect(h.requests.slice(mark).some((r) => r.url.endsWith(`:${oldB.port}/boot-status`))).toBe(true)
  })
})
