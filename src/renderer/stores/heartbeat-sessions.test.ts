// Regression test for the HeartbeatsPanel eternal-"Loading…" mask
// (0.40.48). `refresh()`'s failure path used to set `lastError` WITHOUT
// setting `loadedFor`, so the panel's `showingForLoadedProject` gate
// (`loadedFor === projectPath`) never opened after a failure — the error
// branch was unreachable and the panel showed "Loading…" forever. The
// failure path must now (a) mark the load as being FOR the requested
// project so the error renders, (b) clear rows carried over from a
// DIFFERENT project, and (c) keep same-project rows across a transient
// re-refresh failure.

import { describe, it, expect, vi, beforeEach } from 'vitest'

const invokeMock = vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
}))
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
}))
// Host-aware remote path (0.40.48): the store branches on the active host
// and loads a remote host's roster via daemonCliGet. Default to 'local' so
// the pre-existing tests exercise the unchanged local path.
let mockActiveHost: unknown = 'local'
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: {
    getState: () => ({ activeHost: mockActiveHost, recovery: { kind: 'connected' } }),
  },
}))
const daemonCliGetMock = vi.fn<(route: string, params?: unknown) => Promise<unknown>>()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (route: string, params?: unknown) => daemonCliGetMock(route, params),
  RecoveringError: class RecoveringError extends Error {},
}))

import { useHeartbeatSessionsStore } from './heartbeat-sessions'
import { RecoveringError } from '@/lib/daemon-cli'

const ROW = {
  id: 'hb-1',
  agent: 'claude',
  enabled: true,
} as never

function primeLoaded(projectPath: string): void {
  useHeartbeatSessionsStore.setState({
    active: [{ row: ROW, state: 'scheduled', liveTerminalId: null } as never],
    archived: [],
    loadedFor: projectPath,
    loading: false,
    lastError: null,
  })
}

beforeEach(() => {
  invokeMock.mockReset()
  daemonCliGetMock.mockReset()
  mockActiveHost = 'local'
  useHeartbeatSessionsStore.setState({
    active: [],
    archived: [],
    loadedFor: null,
    loading: false,
    lastError: null,
  })
})

describe('heartbeat-sessions refresh failure path (the "Loading…" mask fix)', () => {
  it('a FAILED first load still sets loadedFor so the panel can show the error', async () => {
    invokeMock.mockRejectedValue(new Error('tauri command exploded'))
    await useHeartbeatSessionsStore.getState().refresh('/ws/a')
    const s = useHeartbeatSessionsStore.getState()
    // The gate the panel keys on: loadedFor === projectPath must hold even
    // on failure, or "Loading…" masks the error forever.
    expect(s.loadedFor).toBe('/ws/a')
    expect(s.lastError).toContain('tauri command exploded')
    expect(s.loading).toBe(false)
  })

  it('a failed load for a NEW project clears the previous project\'s rows', async () => {
    primeLoaded('/ws/old')
    invokeMock.mockRejectedValue(new Error('boom'))
    await useHeartbeatSessionsStore.getState().refresh('/ws/new')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.loadedFor).toBe('/ws/new')
    expect(s.active).toEqual([])
    expect(s.archived).toEqual([])
  })

  it('a transient re-refresh failure for the SAME project keeps its rows', async () => {
    primeLoaded('/ws/a')
    invokeMock.mockRejectedValue(new Error('blip'))
    await useHeartbeatSessionsStore.getState().refresh('/ws/a')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.loadedFor).toBe('/ws/a')
    expect(s.lastError).toContain('blip')
    expect(s.active).toHaveLength(1)
  })

  it('a successful load still lands rows + loadedFor and clears lastError', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'k2so_heartbeat_list') return [ROW]
      if (cmd === 'k2so_heartbeat_list_archived') return []
      return null
    })
    await useHeartbeatSessionsStore.getState().refresh('/ws/a')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.loadedFor).toBe('/ws/a')
    expect(s.lastError).toBeNull()
    expect(s.active).toHaveLength(1)
  })
})

describe('remote-host roster (0.40.48 — the panel previously always read the LOCAL machine)', () => {
  const remoteHost = { id: 'h1', label: 'afsrow', hostname: 'afs.k2.dev', port: 443, secure: true }

  it('loads via the ACTIVE host\'s /cli/heartbeat routes, not Tauri invoke', async () => {
    mockActiveHost = remoteHost
    daemonCliGetMock.mockImplementation(async (route: string) => {
      if (route === 'heartbeat/list') return [{ ...(ROW as object), archivedAt: null }]
      if (route === 'heartbeat/list-archived') return []
      throw new Error(`unexpected route ${route}`)
    })
    await useHeartbeatSessionsStore.getState().refresh('/home/k2/ai/Argus')
    const s = useHeartbeatSessionsStore.getState()
    expect(invokeMock).not.toHaveBeenCalled()
    expect(daemonCliGetMock).toHaveBeenCalledWith('heartbeat/list', {
      project: '/home/k2/ai/Argus',
    })
    expect(s.loadedFor).toBe('/home/k2/ai/Argus')
    expect(s.active).toHaveLength(1)
  })

  it('derives live from the daemon-stamped activeTerminalId (no local PTY proxy)', async () => {
    mockActiveHost = remoteHost
    daemonCliGetMock.mockImplementation(async (route: string) => {
      if (route === 'heartbeat/list')
        return [
          { id: 'a', name: 'live-hb', archivedAt: null, activeTerminalId: 'wake-x-1', lastSessionId: null },
          { id: 'b', name: 'resumable-hb', archivedAt: null, activeTerminalId: null, lastSessionId: 'sess-9' },
          { id: 'c', name: 'scheduled-hb', archivedAt: null, activeTerminalId: null, lastSessionId: null },
        ]
      return []
    })
    await useHeartbeatSessionsStore.getState().refresh('/ws/r')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.active.map((e) => e.state)).toEqual(['live', 'resumable', 'scheduled'])
    expect(s.active[0].liveTerminalId).toBe('wake-x-1')
  })

  it('an older remote daemon omitting activeTerminalId never derives falsely live', async () => {
    mockActiveHost = remoteHost
    daemonCliGetMock.mockImplementation(async (route: string) =>
      route === 'heartbeat/list'
        ? [{ id: 'a', name: 'hb', archivedAt: null, lastSessionId: 'sess-1' }]
        : [],
    )
    await useHeartbeatSessionsStore.getState().refresh('/ws/r')
    expect(useHeartbeatSessionsStore.getState().active[0].state).toBe('resumable')
  })

  it('a RecoveringError (host mid-reconnect) keeps current rows and never paints an error', async () => {
    mockActiveHost = remoteHost
    primeLoaded('/ws/r')
    daemonCliGetMock.mockRejectedValue(new RecoveringError('afsrow', 'reconnecting'))
    await useHeartbeatSessionsStore.getState().refresh('/ws/r')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.lastError).toBeNull()
    expect(s.active).toHaveLength(1)
    expect(s.loading).toBe(false)
  })

  it('a real remote failure still surfaces via the loadedFor error path', async () => {
    mockActiveHost = remoteHost
    daemonCliGetMock.mockRejectedValue(new Error('404: no route found'))
    await useHeartbeatSessionsStore.getState().refresh('/ws/r')
    const s = useHeartbeatSessionsStore.getState()
    expect(s.loadedFor).toBe('/ws/r')
    expect(s.lastError).toContain('no route found')
  })
})
