// Feedback F2 — page open/close state + the live waiting-count badge +
// the FeedbackCreated/FeedbackAnswered event hookup.
//
// The daemon fires `HookEvent::FeedbackCreated` / `::FeedbackAnswered` on
// its `/events` WS (feedback_routes.rs); src-tauri's daemon_events.rs
// subscriber re-emits every frame on the Tauri event bus under the
// HookEvent name — so the renderer listens for `feedback:created` /
// `feedback:answered` / `feedback:status-changed` /
// `feedback:commented` exactly like active-agents listens for
// `agent:lifecycle`. Each event bumps `revision` (the page refetches on
// it) and refreshes the badge count (commented skips the badge — a
// comment never changes status); FeedbackCreated additionally fires a
// desktop notification when the app is unfocused or the Feedback page
// isn't visible (PRD §6 F2) — it is the ONLY event that notifies.

import { create } from 'zustand'
import { useProjectsStore } from '@/stores/projects'
import { useWindowFocusStore } from '@/stores/window-focus'
import { fetchWaitingCount } from '@/components/Feedback/feedback-api'

interface FeedbackState {
  /** Whether the full-page Feedback view is shown. */
  isOpen: boolean
  /** Waiting-item count across all workspaces (top-bar badge; 0 hides). */
  waitingCount: number
  /** Bumped on every feedback event so the open page refetches. */
  revision: number
  open: () => void
  close: () => void
  toggle: () => void
  /** Re-count status=waiting across the already-loaded projects store. */
  refreshWaitingCount: () => Promise<void>
}

export const useFeedbackStore = create<FeedbackState>((set) => ({
  isOpen: false,
  waitingCount: 0,
  revision: 0,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
  refreshWaitingCount: async () => {
    const projects = useProjectsStore.getState().projects
    try {
      const count = await fetchWaitingCount(projects)
      set({ waitingCount: count })
    } catch (err) {
      console.warn('[feedback] waiting-count refresh failed:', err)
    }
  },
}))

/** Event payload contract — frozen in feedback_routes.rs. */
interface FeedbackCreatedPayload {
  id: string
  projectPath: string
  title: string
  kind: string
  priority: number
  agentName: string
}

/** Desktop notification via tauri-plugin-notification: agent name +
 *  ask title. Permission is requested lazily on the first send. */
async function notifyDesktop(agentName: string, title: string): Promise<void> {
  try {
    const { isPermissionGranted, requestPermission, sendNotification } =
      await import('@tauri-apps/plugin-notification')
    let granted = await isPermissionGranted()
    if (!granted) {
      granted = (await requestPermission()) === 'granted'
    }
    if (granted) {
      sendNotification({ title: agentName, body: title })
    }
  } catch (err) {
    console.warn('[feedback] desktop notification failed:', err)
  }
}

let eventsInitialized = false

/** Wire the feedback event listeners ONCE per window (idempotent — the
 *  top-bar button mounts/unmounts across layout switches but the
 *  subscription must survive them, so it's never torn down).
 *
 *  `notify` gates the desktop-notification side effect: only the MAIN
 *  window fires it, so a focus window viewing the same daemon doesn't
 *  double-notify. Badge/revision updates run in every window. */
export function initFeedbackEvents(notify: boolean): void {
  if (eventsInitialized) return
  eventsInitialized = true

  void import('@tauri-apps/api/event').then(({ listen }) => {
    // listen() rejects outside Tauri (vitest) — warn, don't blow up.
    listen<FeedbackCreatedPayload>('feedback:created', (event) => {
      const store = useFeedbackStore.getState()
      useFeedbackStore.setState((s) => ({ revision: s.revision + 1 }))
      void store.refreshWaitingCount()
      // Notify ONLY when the ask can't already be on screen: app
      // unfocused OR the Feedback page not visible (PRD §6 F2).
      const focused = useWindowFocusStore.getState().isFocused
      const pageVisible = useFeedbackStore.getState().isOpen
      if (notify && (!focused || !pageVisible)) {
        const { agentName, title } = event.payload
        void notifyDesktop(agentName || 'Agent', title || 'New feedback')
      }
    }).catch((err) => console.warn('[feedback] listen() unavailable:', err))
    listen('feedback:answered', () => {
      useFeedbackStore.setState((s) => ({ revision: s.revision + 1 }))
      void useFeedbackStore.getState().refreshWaitingCount()
    }).catch((err) => console.warn('[feedback] listen() unavailable:', err))
    // Resolve / dismiss / reopen-to-waiting (`/cli/feedback/resolve`) —
    // same refresh as answered: bump revision + re-count the badge.
    listen('feedback:status-changed', () => {
      useFeedbackStore.setState((s) => ({ revision: s.revision + 1 }))
      void useFeedbackStore.getState().refreshWaitingCount()
    }).catch((err) => console.warn('[feedback] listen() unavailable:', err))
    // A stored comment (`/cli/feedback/comment` — agent or human — and
    // the thread entry the answer route creates). INTERNAL refresh
    // signal only: bump revision so the open list refreshes comment
    // counts and the open thread refetches. A comment never changes
    // status (an answering first comment rides feedback:answered), so
    // the badge count is untouched — and it NEVER notifies (frozen
    // contract: only NEW items notify, via feedback:created).
    listen('feedback:commented', () => {
      useFeedbackStore.setState((s) => ({ revision: s.revision + 1 }))
    }).catch((err) => console.warn('[feedback] listen() unavailable:', err))
  })
}
