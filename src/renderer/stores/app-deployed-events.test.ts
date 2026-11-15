// Workspace drawer Apps — APP-LEVEL `app_deployed` dispatch through the
// REAL session-events module (the publish_services_changed pattern), against
// a fake WebSocket at the transport boundary.

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
  onAppDeployed,
  type UnsubscribeFn,
} from './session-events'

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

describe('onAppDeployed (app-level app_deployed dispatch)', () => {
  it('fires with the App and its workspace, until unsubscribed', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const seen: string[] = []
    const off = onAppDeployed(primaryScope(), (e) => seen.push(`${e.projectId}/${e.appId}`))

    const frame = { kind: 'app_deployed', appId: 'shop', serviceId: 'svc-1', projectId: 'proj-1', seed: null, deployment: null, deployKind: 'restart' }
    pushFrame(ws, frame)
    expect(seen).toEqual(['proj-1/shop'])

    off()
    pushFrame(ws, frame)
    expect(seen).toEqual(['proj-1/shop'])
  })

  it('does not fire for publish_services_changed', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const seen: string[] = []
    cleanups.push(onAppDeployed(primaryScope(), (e) => seen.push(e.appId)))

    pushFrame(ws, { kind: 'publish_services_changed', projectId: 'proj-1' })
    expect(seen).toEqual([])
  })
})
