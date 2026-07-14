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

import { useHeartbeatSessionsStore } from './heartbeat-sessions'

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
