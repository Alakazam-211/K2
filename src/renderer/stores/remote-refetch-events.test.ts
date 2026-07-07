// Remote live-update fix — app-level `project_groups_changed` /
// `feedback_changed` dispatch through the REAL session-events module.
//
// The daemon mirrors every `project-group:*` / `feedback:*` HookEvent
// (which ride the loopback-only /events WS and never reach a K2 Connect
// client) onto the host-aware /cli/sessions/events bus as payload-lean
// refetch signals: `{ kind: 'project_groups_changed', reason }` and
// `{ kind: 'feedback_changed', reason }`. This suite pins, against the
// REAL session-events module (fake WebSocket at the boundary — the
// open-url-events.test.ts pattern):
//   - `onProjectGroupsChanged` / `onFeedbackChanged` registration: a
//     frame on the app-level socket fires the callback with the
//     unwrapped `reason` string,
//   - the returned unsubscribe fns deregister (no further callbacks),
//   - the two kinds route to their OWN registries (no cross-talk).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the module imports) ─────────────────
// Real session-events runs on top of this transport boundary — creds
// resolve so `subscribeToActiveState` constructs the (fake) WebSocket.
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
// Keep the hello-driven `refreshActiveSnapshot` inert (capability off) —
// this suite owns the refetch-signal path only.
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
  FEATURES: {},
}))

// ── Fake WebSocket (recording) ────────────────────────────────────────────
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
  onProjectGroupsChanged,
  onFeedbackChanged,
  type UnsubscribeFn,
} from './session-events'

/** Open the app-level subscription and wait for the fake socket. */
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

describe('onProjectGroupsChanged (app-level project_groups_changed dispatch)', () => {
  it('fires the registered callback with the unwrapped reason', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const reasons: string[] = []
    cleanups.push(onProjectGroupsChanged((reason) => reasons.push(reason)))

    pushFrame(ws, { kind: 'project_groups_changed', reason: 'members-changed' })
    pushFrame(ws, { kind: 'project_groups_changed', reason: 'message-created' })
    expect(reasons).toEqual(['members-changed', 'message-created'])
  })

  it('stops firing after the returned unsubscribe fn runs', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const reasons: string[] = []
    const off = onProjectGroupsChanged((reason) => reasons.push(reason))

    pushFrame(ws, { kind: 'project_groups_changed', reason: 'poc-changed' })
    expect(reasons).toEqual(['poc-changed'])

    off()
    pushFrame(ws, { kind: 'project_groups_changed', reason: 'groups-changed' })
    expect(reasons).toEqual(['poc-changed'])
  })
})

describe('onFeedbackChanged (app-level feedback_changed dispatch)', () => {
  it('fires the registered callback with the unwrapped reason', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const reasons: string[] = []
    cleanups.push(onFeedbackChanged((reason) => reasons.push(reason)))

    pushFrame(ws, { kind: 'feedback_changed', reason: 'created' })
    pushFrame(ws, { kind: 'feedback_changed', reason: 'commented' })
    expect(reasons).toEqual(['created', 'commented'])
  })

  it('stops firing after the returned unsubscribe fn runs', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const reasons: string[] = []
    const off = onFeedbackChanged((reason) => reasons.push(reason))

    pushFrame(ws, { kind: 'feedback_changed', reason: 'answered' })
    expect(reasons).toEqual(['answered'])

    off()
    pushFrame(ws, { kind: 'feedback_changed', reason: 'status-changed' })
    expect(reasons).toEqual(['answered'])
  })
})

describe('registry isolation', () => {
  it('each kind reaches only its own registry (no cross-talk)', async () => {
    const { ws, unsub } = await openAppSocket()
    cleanups.push(unsub)

    const groups: string[] = []
    const feedback: string[] = []
    cleanups.push(onProjectGroupsChanged((r) => groups.push(r)))
    cleanups.push(onFeedbackChanged((r) => feedback.push(r)))

    pushFrame(ws, { kind: 'project_groups_changed', reason: 'layout-changed' })
    pushFrame(ws, { kind: 'feedback_changed', reason: 'created' })

    expect(groups).toEqual(['layout-changed'])
    expect(feedback).toEqual(['created'])
  })
})
