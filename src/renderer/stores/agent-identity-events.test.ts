// Agent rename live update — APP-LEVEL `agent_identity_changed` dispatch
// through the REAL session-events module (the workspace_resources_changed
// pattern), against a fake WebSocket at the transport boundary.

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
  close(): void {}
}
vi.stubGlobal('WebSocket', FakeWebSocket)

import {
  openAppBus,
  subscribeToActiveState,
  onAgentIdentityChanged,
  type AgentIdentityChangedEvent,
  type UnsubscribeFn,
} from './session-events'
import { CARRIED_KINDS, APP_SOCKET_KINDS } from './session-event-kinds'
import { primaryScope } from '@/kessel/server-scope'

async function openAppSocket(): Promise<{ ws: FakeWebSocket; unsub: UnsubscribeFn }> {
  const unsub = subscribeToActiveState(primaryScope())
  await vi.waitFor(() => {
    expect(FakeWebSocket.instances.length).toBeGreaterThan(0)
  })
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  ws.onopen?.()
  return { ws, unsub }
}

const frame = {
  kind: 'agent_identity_changed',
  projectId: 'proj-1',
  workspacePath: '/w/sales',
  fields: ['displayName', 'name'],
  name: 'Revenue Desk',
  displayName: 'Revenue Desk',
  handle: 'sales',
  color: '#ff00aa',
}

const cleanups: UnsubscribeFn[] = []
beforeEach(() => {
  FakeWebSocket.instances = []
})
afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()!()
})

describe('onAgentIdentityChanged (app-level agent_identity_changed)', () => {
  it('is an app kind a Home room carrier forwards too', () => {
    expect(APP_SOCKET_KINDS.has('agent_identity_changed')).toBe(true)
    expect(CARRIED_KINDS.has('agent_identity_changed')).toBe(true)
  })

  it('fires with the carried current values; unsubscribe stops it', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)
    const seen: AgentIdentityChangedEvent[] = []
    const off = onAgentIdentityChanged(primaryScope(), (e) => seen.push(e))
    ws.onmessage?.({ data: JSON.stringify(frame) })
    expect(seen).toEqual([frame])
    off()
    ws.onmessage?.({ data: JSON.stringify({ ...frame, name: 'Again' }) })
    expect(seen.length).toBe(1)
  })

  it('the per-server app bus facade exposes it', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)
    const seen: string[] = []
    cleanups.push(openAppBus(primaryScope()).onAgentIdentityChanged((e) => seen.push(e.displayName)))
    ws.onmessage?.({ data: JSON.stringify(frame) })
    ws.onmessage?.({ data: JSON.stringify({ kind: 'projects_changed' }) })
    expect(seen).toEqual(['Revenue Desk'])
  })
})
