// Home M5 — the room write allowlist (kessel/room-writes.ts) and the usable
// remote-room scope. A remote room writes ONLY to its own server and ONLY the
// routes on the list; this computer's commands stay off in it (MS57/MS67).

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { ROOM_WRITES, ROOM_WRITE_ROUTES } from '@/kessel/room-writes'
import {
  RoomWriteRefusedError,
  ViewOnlyWriteError,
  assertScopeMayWrite,
  primaryScope,
  remoteRoomScope,
  scopeForHost,
  scopeMayWrite,
  viewOnlyScope,
  __resetServerScopesForTests,
} from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { LOCAL_ONLY_ACTIONS, mayRunLocalActions, roomMayRun } from '@/lib/local-only-actions'
import { createPinnedRoom } from '@/stores/room'
import { createStore } from 'zustand/vanilla'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { RoomProjectsStore } from '@/stores/room'

const HERE = dirname(fileURLToPath(import.meta.url))
const REPO = join(HERE, '..', '..', '..')
const RENDERER = join(HERE, '..')

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: 59_998,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}

beforeEach(() => {
  __resetServerScopesForTests()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
})

/** `/cli/<route>` → [method kind, floor] from the daemon's route policy table. */
function routePolicy(): Map<string, { kind: string; floor: string }> {
  const src = readFileSync(join(REPO, 'crates/k2-daemon/src/routes/route_policy.rs'), 'utf8')
  const out = new Map<string, { kind: string; floor: string }>()
  for (const m of src.matchAll(/^\s*(get|post|both)\("\/cli\/([^"]+)",\s*(\w+)\)/gm)) {
    out.set(m[2], { kind: m[1], floor: m[3] })
  }
  if (out.size < 100) throw new Error(`route policy table parse found only ${out.size} routes`)
  return out
}

/** Writes the daemon takes as a GET (older verb routes). */
const GET_SHAPED_WRITES = new Set(['heartbeat/launch', 'heartbeat/enable', 'workspace/set-chat-session', 'agents/lock'])

/** Room writes whose daemon floor is above Member. The room lists them, and
 *  the client sends them only for a login that clears the floor (0.43.2 Q3:
 *  Keep awake changes the host's power, so Admin and up). */
const ABOVE_MEMBER_ROOM_WRITES = new Map([['power/keep-awake', 'Admin']])

describe('the room write allowlist', () => {
  it('every route is a real daemon route; all but the listed Admin ones are open to a Member (route_policy.rs)', () => {
    const policy = routePolicy()
    for (const route of ROOM_WRITE_ROUTES) {
      const p = policy.get(route)
      if (!p) throw new Error(`${route} is on the room allowlist but not in route_policy.rs`)
      const want = GET_SHAPED_WRITES.has(route) ? ['get', 'both'] : ['post', 'both']
      expect([route, want.includes(p.kind)]).toEqual([route, true])
      const floor = ABOVE_MEMBER_ROOM_WRITES.has(route) ? ABOVE_MEMBER_ROOM_WRITES.get(route) : 'Member'
      expect([route, p.floor]).toEqual([route, floor])
    }
    for (const route of ABOVE_MEMBER_ROOM_WRITES.keys()) {
      expect([route, ROOM_WRITE_ROUTES.has(route)]).toEqual([route, true])
    }
  })

  it('every route says why a room sends it', () => {
    for (const [route, why] of Object.entries(ROOM_WRITES)) expect([route, why.length > 10]).toEqual([route, true])
  })

  it('server management never comes from a room', () => {
    for (const route of [
      'projects/delete',
      'projects/update',
      'projects/reorder',
      'workspace-layouts/delete',
      'presets/create',
      'presets/reset',
      'fs/open-finder',
      'fs/open-external',
      'daemon/restart',
      'daemon/update/apply',
      'presence/kick',
      'publish/run',
      'api-keys/create',
      'skills/create',
      'mail/server/enable',
      'project-group/create',
      'focus-groups/create',
      'themes/delete',
      'agents/save-agent-md',
      'sandbox/open',
    ]) {
      expect([route, ROOM_WRITE_ROUTES.has(route)]).toEqual([route, false])
    }
  })
})

describe('the usable remote-room scope', () => {
  it('is the same server as its base: same id, host key and creds; one twin per scope', async () => {
    const base = scopeForHost(B)
    const room = remoteRoomScope(base)
    expect(room.remoteRoom).toBe(true)
    expect(base.remoteRoom).toBe(undefined)
    expect(room.id).toBe(base.id)
    expect(room.hostKey).toBe(base.hostKey)
    expect(room.isRemote).toBe(true)
    expect(await room.creds()).toEqual(await base.creds())
    expect(remoteRoomScope(base)).toBe(room)
    expect(remoteRoomScope(room)).toBe(room)
  })

  it('is never the window’s scope, and a view-only scope never becomes usable', () => {
    expect(() => remoteRoomScope(primaryScope())).toThrow(/pinned to one server/)
    expect(() => remoteRoomScope(viewOnlyScope(scopeForHost(B)))).toThrow(/view-only/)
  })

  it('refuses an unlisted write before any request; a listed write goes to that server with its login', async () => {
    const room = remoteRoomScope(scopeForHost(B))
    const calls: string[] = []
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      calls.push(String(input))
      return new Response(JSON.stringify({ success: true }), { status: 200 })
    })
    try {
      await expect(daemonCliPost(room, 'projects/delete', { id: 'p' })).rejects.toBeInstanceOf(RoomWriteRefusedError)
      await expect(daemonCliPost(room, 'fs/open-finder', { target: '/x' })).rejects.toBeInstanceOf(RoomWriteRefusedError)
      expect(calls).toEqual([])
      await daemonCliPost(room, 'chat/rename', { sessionId: 's', name: 'n' })
      expect(calls.length).toBe(1)
      const url = new URL(calls[0])
      expect(url.port).toBe(String(B.port))
      expect(url.pathname).toBe('/cli/chat/rename')
      expect(url.searchParams.get('token')).toBe('tok-b')
    } finally {
      fetchSpy.mockRestore()
    }
  })

  it('assertScopeMayWrite / scopeMayWrite: room list for a usable room, keep-alive only for view-only, open elsewhere', () => {
    const base = scopeForHost(B)
    const usable = remoteRoomScope(base)
    const viewOnly = viewOnlyScope(base)
    expect(scopeMayWrite(usable, 'heartbeat/launch')).toBe(true)
    expect(scopeMayWrite(usable, 'projects/delete')).toBe(false)
    expect(() => assertScopeMayWrite(usable, 'presets/reset')).toThrow(RoomWriteRefusedError)
    expect(scopeMayWrite(viewOnly, 'heartbeat/launch')).toBe(false)
    expect(() => assertScopeMayWrite(viewOnly, 'heartbeat/launch')).toThrow(ViewOnlyWriteError)
    expect(scopeMayWrite(viewOnly, 'projects/activate')).toBe(true)
    expect(scopeMayWrite(base, 'projects/delete')).toBe(true)
    expect(scopeMayWrite(primaryScope(), 'projects/delete')).toBe(true)
  })

  it('writes that skip daemonCliPost are held to the same list (raw spawn, raw close, GET-shaped verbs)', () => {
    const read = (f: string): string => readFileSync(join(RENDERER, f), 'utf8')
    expect(read('kessel-term/TerminalPane.tsx')).toMatch(/scopeMayWrite\(room\.scope, 'sessions\/v2\/spawn'\)/)
    expect(read('stores/tabs.ts')).toMatch(/assertScopeMayPost\(scope, 'sessions\/v2\/close'\)/)
    expect(read('lib/heartbeat-launch.ts')).toMatch(/assertScopeMayWrite\(room\.scope, 'heartbeat\/launch'\)/)
    expect(read('components/HeartbeatsPanel/HeartbeatEntry.tsx')).toMatch(/scopeMayWrite\(room\.scope, 'heartbeat\/enable'\)/)
  })
})

describe('a remote room runs none of this computer’s commands (MS57/MS67)', () => {
  function pinned(scopeRef: 'local' | ConnectHost) {
    const projects = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: [] }))
    return createPinnedRoom({
      scope: scopeForHost(scopeRef),
      workspace: { projectId: 'p', workspaceId: 'w', path: '/srv/p' },
      projects: projects as RoomProjectsStore,
      activateProject: () => {},
    })
  }

  it('a usable room on another server: every local-only action is off', async () => {
    const room = pinned(B)
    try {
      expect(room.readOnly).toBe(false)
      expect(room.scope.remoteRoom).toBe(true)
      expect(mayRunLocalActions(room)).toBe(false)
      for (const name of Object.keys(LOCAL_ONLY_ACTIONS)) expect([name, roomMayRun(room, name)]).toEqual([name, false])
    } finally {
      await room.dispose()
    }
  })

  it('a room on this computer (window on another server) keeps them', async () => {
    const room = pinned('local')
    try {
      expect(mayRunLocalActions(room)).toBe(true)
      expect(roomMayRun(room, 'projects_open_in_finder')).toBe(true)
    } finally {
      await room.dispose()
    }
  })
})
