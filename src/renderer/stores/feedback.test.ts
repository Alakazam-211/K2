// Feedback store — event wiring for the live-update path.
//
// The daemon fires feedback:created / feedback:answered /
// feedback:status-changed / feedback:commented on its /events WS and
// the daemon_events bridge re-emits them on the Tauri bus. This suite
// proves the store's `initFeedbackEvents` wiring per event:
//   - all four events are subscribed;
//   - feedback:commented bumps `revision` (the open page/thread
//     refetch on it) and does NOT touch the waiting-count badge (a
//     comment never changes status);
//   - feedback:commented NEVER fires the desktop notification — the
//     frozen contract says only NEW items notify — while
//     feedback:created still does (the control that proves the
//     notification seam is live, so the "not called" half isn't
//     vacuous);
//   - feedback:answered / feedback:status-changed bump revision AND
//     re-count the badge.
//
// vitest env is node — the Tauri event bus + notification plugin are
// mocked at the module boundary (daemon-broadcasts.test.ts idiom).

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store imports) ──────────────────

// Tauri event bus: record each named listener so tests can fire events.
const bus = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, fn: (event: { payload: unknown }) => void) => {
    bus.listeners.set(name, fn)
    return () => undefined
  }),
}))

// Desktop-notification plugin: permission always granted so a notify
// path that runs WILL reach sendNotification.
const notification = vi.hoisted(() => ({ send: vi.fn() }))
vi.mock('@tauri-apps/plugin-notification', () => ({
  isPermissionGranted: vi.fn(async () => true),
  requestPermission: vi.fn(async () => 'granted'),
  sendNotification: notification.send,
}))

// Badge-count fan-out — recorded, inert.
const api = vi.hoisted(() => ({ fetchWaitingCount: vi.fn(async () => 3) }))
vi.mock('@/components/Feedback/feedback-api', () => ({
  fetchWaitingCount: api.fetchWaitingCount,
}))

// Projects store: the count fan-out only needs `projects`.
vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [] }) },
}))

import { useFeedbackStore, initFeedbackEvents } from '@/stores/feedback'
import { useWindowFocusStore } from '@/stores/window-focus'

/** Both listen() registration and notifyDesktop ride dynamic imports —
 *  flush a macrotask so their .then chains settle. */
async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0))
}

function fire(name: string, payload: unknown): void {
  const fn = bus.listeners.get(name)
  if (!fn) throw new Error(`no listener registered for ${name}`)
  fn({ payload })
}

describe('feedback store event wiring', () => {
  beforeEach(async () => {
    // initFeedbackEvents is idempotent per module instance — the first
    // beforeEach wires it (notify=true, the MAIN-window role), the rest
    // no-op. Unfocused so feedback:created WOULD notify.
    useWindowFocusStore.setState({ isFocused: false })
    initFeedbackEvents(true)
    await flush()
    notification.send.mockClear()
    api.fetchWaitingCount.mockClear()
  })

  it('subscribes to all four feedback events', () => {
    for (const name of [
      'feedback:created',
      'feedback:answered',
      'feedback:status-changed',
      'feedback:commented',
    ]) {
      expect(bus.listeners.has(name), `listener for ${name}`).toBe(true)
    }
  })

  it('feedback:commented bumps revision without touching the badge', async () => {
    const before = useFeedbackStore.getState().revision
    fire('feedback:commented', { id: 'fb-1', projectPath: '/tmp/ws', author: 'scout' })
    expect(useFeedbackStore.getState().revision).toBe(before + 1)
    await flush()
    expect(api.fetchWaitingCount).not.toHaveBeenCalled()
  })

  it('feedback:commented never fires the desktop notification', async () => {
    // Fire a comment, then a created CONTROL through the same pipeline
    // — waiting for the control's notification proves the notify seam
    // was live the whole time, so the comment's silence isn't vacuous.
    fire('feedback:commented', { id: 'fb-1', projectPath: '/tmp/ws', author: 'owner' })
    fire('feedback:created', {
      id: 'fb-2',
      projectPath: '/tmp/ws',
      title: 'Which port?',
      kind: 'question',
      priority: 3,
      agentName: 'scout',
    })
    await vi.waitFor(() => expect(notification.send).toHaveBeenCalled())
    await flush()
    expect(notification.send).toHaveBeenCalledTimes(1)
    expect(notification.send).toHaveBeenCalledWith({ title: 'scout', body: 'Which port?' })
  })

  it('feedback:answered and feedback:status-changed bump revision and re-count the badge', async () => {
    const before = useFeedbackStore.getState().revision
    fire('feedback:answered', { id: 'fb-3', projectPath: '/tmp/ws' })
    fire('feedback:status-changed', { id: 'fb-3', projectPath: '/tmp/ws', status: 'resolved' })
    expect(useFeedbackStore.getState().revision).toBe(before + 2)
    await flush()
    expect(api.fetchWaitingCount).toHaveBeenCalledTimes(2)
    expect(notification.send).not.toHaveBeenCalled()
  })
})
