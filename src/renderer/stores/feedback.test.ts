// Feedback store — event wiring for the live-update path.
//
// Remote live-update fix: the daemon mirrors every `feedback:*`
// HookEvent onto the HOST-AWARE /cli/sessions/events bus as an
// app-level `feedback_changed` refetch signal, and the store subscribes
// via `onFeedbackChanged(reason)` (stores/session-events.ts) instead of
// the old loopback-only Tauri `listen('feedback:*')` wiring. This suite
// proves the store's `initFeedbackEvents` wiring per reason:
//   - one registration on the session-events registry;
//   - reason 'commented' bumps `revision` (the open page/thread refetch
//     on it) and does NOT touch the waiting-count badge (a comment
//     never changes status);
//   - 'commented' NEVER fires the desktop notification — the frozen
//     contract says only NEW items notify — while 'created' still does
//     (the control that proves the notification seam is live, so the
//     "not called" half isn't vacuous). The lean signal carries no
//     agentName/title, so 'created' notifies with generic copy;
//   - 'answered' / 'status-changed' bump revision AND re-count the badge.
//
// vitest env is node — the session-events registry + notification
// plugin are mocked at the module boundary.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store imports) ──────────────────

// session-events registry: record the store's handler so tests can fire
// reasons through it (the daemon-broadcasts.test.ts idiom).
const ev = vi.hoisted(() => ({
  handlers: [] as Array<(reason: string) => void>,
}))
vi.mock('@/stores/session-events', () => ({
  onFeedbackChanged: vi.fn((fn: (reason: string) => void) => {
    ev.handlers.push(fn)
    return () => void (ev.handlers = ev.handlers.filter((f) => f !== fn))
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

/** notifyDesktop rides a dynamic import — flush a macrotask so its
 *  .then chain settles. */
async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0))
}

function fire(reason: string): void {
  if (ev.handlers.length === 0) throw new Error('no feedback handler registered')
  for (const fn of [...ev.handlers]) fn(reason)
}

describe('feedback store event wiring', () => {
  beforeEach(async () => {
    // initFeedbackEvents is idempotent per module instance — the first
    // beforeEach wires it (notify=true, the MAIN-window role), the rest
    // no-op. Unfocused so reason 'created' WOULD notify.
    useWindowFocusStore.setState({ isFocused: false })
    initFeedbackEvents(true)
    await flush()
    notification.send.mockClear()
    api.fetchWaitingCount.mockClear()
  })

  it('registers ONE handler on the session-events registry', () => {
    // Idempotence — repeated beforeEach inits must not stack.
    expect(ev.handlers).toHaveLength(1)
  })

  it("reason 'commented' bumps revision without touching the badge", async () => {
    const before = useFeedbackStore.getState().revision
    fire('commented')
    expect(useFeedbackStore.getState().revision).toBe(before + 1)
    await flush()
    expect(api.fetchWaitingCount).not.toHaveBeenCalled()
  })

  it("reason 'commented' never fires the desktop notification", async () => {
    // Fire a comment, then a 'created' CONTROL through the same pipeline
    // — waiting for the control's notification proves the notify seam
    // was live the whole time, so the comment's silence isn't vacuous.
    fire('commented')
    fire('created')
    await vi.waitFor(() => expect(notification.send).toHaveBeenCalled())
    await flush()
    expect(notification.send).toHaveBeenCalledTimes(1)
    // The lean refetch signal carries no agentName/title — generic copy.
    expect(notification.send).toHaveBeenCalledWith({ title: 'Agent', body: 'New feedback' })
  })

  it("reason 'created' also re-counts the badge", async () => {
    const before = useFeedbackStore.getState().revision
    fire('created')
    expect(useFeedbackStore.getState().revision).toBe(before + 1)
    await flush()
    expect(api.fetchWaitingCount).toHaveBeenCalledTimes(1)
  })

  it("reasons 'answered' and 'status-changed' bump revision and re-count the badge", async () => {
    const before = useFeedbackStore.getState().revision
    fire('answered')
    fire('status-changed')
    expect(useFeedbackStore.getState().revision).toBe(before + 2)
    await flush()
    expect(api.fetchWaitingCount).toHaveBeenCalledTimes(2)
    expect(notification.send).not.toHaveBeenCalled()
  })
})
