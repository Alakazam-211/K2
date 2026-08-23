// Workspace Resources — APP-LEVEL `workspace_resources_changed` dispatch
// through the REAL session-events module (the publish_services_changed
// pattern).
//
// Pins, against a fake WebSocket at the transport boundary:
//   - onWorkspaceResourcesChanged fires with the carried workspaceId
//   - unsubscribe deregisters
//   - a different kind does not leak into this registry

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
  FEATURES: {},
}))

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  url: string
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: ((ev: { code: number; reason?: string }) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  close(): void {
    // No onclose echo — the unsubscribe path must not depend on it.
  }
}
vi.stubGlobal('WebSocket', FakeWebSocket)

import {
  subscribeToActiveState,
  onWorkspaceResourcesChanged,
  type UnsubscribeFn,
} from './session-events'

async function openAppSocket(): Promise<{ ws: FakeWebSocket; unsub: UnsubscribeFn }> {
  const unsub = subscribeToActiveState()
  await vi.waitFor(() => {
    expect(FakeWebSocket.instances.length).toBeGreaterThan(0)
  })
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  ws.onopen?.()
  return { ws, unsub }
}

function pushFrame(ws: FakeWebSocket, frame: unknown): void {
  ws.onmessage?.({ data: JSON.stringify(frame) })
}

const cleanups: UnsubscribeFn[] = []

beforeEach(() => {
  FakeWebSocket.instances = []
})

afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()!()
})

describe('onWorkspaceResourcesChanged (app-level workspace_resources_changed dispatch)', () => {
  it('fires the registered callback with the carried workspaceId', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const seen: string[] = []
    cleanups.push(onWorkspaceResourcesChanged((e) => seen.push(e.workspaceId)))

    pushFrame(ws, { kind: 'workspace_resources_changed', workspaceId: 'proj-1' })
    pushFrame(ws, { kind: 'workspace_resources_changed', workspaceId: 'proj-2' })
    expect(seen).toEqual(['proj-1', 'proj-2'])
  })

  it('stops firing after the returned unsubscribe fn runs', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const seen: string[] = []
    const off = onWorkspaceResourcesChanged((e) => seen.push(e.workspaceId))

    pushFrame(ws, { kind: 'workspace_resources_changed', workspaceId: 'proj-1' })
    expect(seen).toEqual(['proj-1'])

    off()
    pushFrame(ws, { kind: 'workspace_resources_changed', workspaceId: 'proj-2' })
    expect(seen).toEqual(['proj-1'])
  })

  it('does not fire for a different app-level kind', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const seen: string[] = []
    cleanups.push(onWorkspaceResourcesChanged((e) => seen.push(e.workspaceId)))

    pushFrame(ws, { kind: 'publish_services_changed', projectId: 'proj-1' })
    expect(seen).toEqual([])
  })
})
