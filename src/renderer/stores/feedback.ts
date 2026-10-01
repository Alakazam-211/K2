// Feedback F2 — page open/close state + the live waiting-count badge +
// the FeedbackCreated/FeedbackAnswered event hookup.
//
// The daemon fires `HookEvent::FeedbackCreated` / `::FeedbackAnswered` /
// `::FeedbackStatusChanged` / `::FeedbackCommented` (feedback_routes.rs)
// and — remote live-update fix — mirrors every one of them onto the
// HOST-AWARE `/cli/sessions/events` bus as an app-level
// `feedback_changed` refetch signal (`reason` = the hook name minus its
// `feedback:` prefix). This store subscribes via `onFeedbackChanged`
// (stores/session-events.ts), which — unlike the old Tauri
// `listen('feedback:*')` wiring riding the loopback-only `/events` WS —
// also fires when the app is connected to a REMOTE host over K2 Connect.
// Each reason bumps `revision` (the page refetches on it) and refreshes
// the badge count (`commented` skips the badge — a comment never changes
// status); `created` additionally fires a desktop notification when the
// app is unfocused or the Feedback page isn't visible (PRD §6 F2) — it
// is the ONLY reason that notifies. The lean signal carries no item
// payload, so the notification uses generic copy (the old event's
// agentName/title rode the legacy payload).

import { create } from 'zustand'
import { onFeedbackChanged } from '@/stores/session-events'
import { usePageViewStore } from '@/stores/page-view'
import { useWindowFocusStore } from '@/stores/window-focus'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchWaitingCount } from '@/components/Feedback/feedback-api'
import { primaryScope } from '@/kessel/server-scope'
import { homeHostKey } from '@/lib/host-key'
import { subscribeSoftResync } from '@/lib/soft-resync'

interface FeedbackState {
  /** Whether the full-page Feedback view is shown. Mirrors
   *  `usePageViewStore.page === 'feedback'` (§6.0 switcher — the SSOT);
   *  kept as a field so pre-switcher consumers (the page gate, the
   *  notification visibility check, tests) read it unchanged. */
  isOpen: boolean
  /** Waiting tickets on the window's server that the page can show (top-bar
   *  badge; 0 hides). Belongs to the window's server: reset on a switch. */
  waitingCount: number
  /** The last refresh failed, or the connection dropped since the last good
   *  one: `waitingCount` may be out of date. The badge dims and says so. */
  waitingStale: boolean
  /** The window's server does not report `tickets-list-all`: its count would
   *  include tickets nobody can open, so the badge shows a dimmed `?`. */
  waitingUnsupported: boolean
  /** Bumped on every feedback event so the open page refetches. */
  revision: number
  open: () => void
  close: () => void
  toggle: () => void
  /** Re-read the host-wide waiting count from the window's server. */
  refreshWaitingCount: () => Promise<void>
}

/** Bumped on every server switch; a refresh that started under an older
 *  epoch drops its result, so server A's late answer never lands on B. */
let hostEpoch = 0

export const useFeedbackStore = create<FeedbackState>((set) => ({
  isOpen: false,
  waitingCount: 0,
  waitingStale: false,
  waitingUnsupported: false,
  revision: 0,
  // open/close/toggle delegate to the page-view SSOT (§6.0): opening the
  // Feedback page IS selecting the Feedback tab; closing returns to the
  // Agents page (today's default view). The subscription below mirrors
  // the result back into `isOpen`.
  open: () => usePageViewStore.getState().setPage('feedback'),
  close: () => usePageViewStore.getState().setPage('agents'),
  toggle: () =>
    usePageViewStore
      .getState()
      .setPage(usePageViewStore.getState().page === 'feedback' ? 'agents' : 'feedback'),
  refreshWaitingCount: async () => {
    const epoch = hostEpoch
    if (!primaryScope().serverSupports('tickets-list-all')) {
      // Older server: its waiting-count also counts tickets from removed
      // workspaces (and a pre-0.40.106 one has no count at all), so no
      // number it gives is one the page can back up.
      set({ waitingCount: 0, waitingStale: false, waitingUnsupported: true })
      return
    }
    try {
      const count = await fetchWaitingCount()
      if (epoch !== hostEpoch) return
      set({ waitingCount: count, waitingStale: false, waitingUnsupported: false })
    } catch (err) {
      if (epoch !== hostEpoch) return
      console.warn('[feedback] waiting-count refresh failed:', err)
      // Keep the last number; say it may be out of date.
      set({ waitingStale: true })
    }
  },
}))

// Keep `isOpen` in lockstep with the page-view store (one-way: page is
// the SSOT; the mutators above only ever write through it).
usePageViewStore.subscribe((s) => {
  const isOpen = s.page === 'feedback'
  if (useFeedbackStore.getState().isOpen !== isOpen) {
    useFeedbackStore.setState({ isOpen })
  }
})

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

  // Remote live-update fix — one host-aware registration replaces the
  // four Tauri listen('feedback:*') calls (loopback-only bus; see the
  // module header). Never torn down (module-lifetime), exactly like the
  // old listeners.
  onFeedbackChanged(primaryScope(), (reason) => {
    // Every reason bumps revision — the open page/thread refetches on it.
    useFeedbackStore.setState((s) => ({ revision: s.revision + 1 }))
    // A stored comment (`/cli/feedback/comment` — agent or human — and
    // the thread entry the answer route creates) is an INTERNAL refresh
    // signal only: a comment never changes status (an answering first
    // comment rides reason 'answered'), so the badge count is untouched
    // — and it NEVER notifies (frozen contract: only NEW items notify,
    // via reason 'created').
    if (reason === 'commented') return
    // created / answered / status-changed (resolve, dismiss,
    // reopen-to-waiting) — and, forward-compat, any unknown reason —
    // re-count the waiting badge.
    void useFeedbackStore.getState().refreshWaitingCount()
    if (reason === 'created') {
      // Notify ONLY when the ask can't already be on screen: app
      // unfocused OR the Feedback page not visible (PRD §6 F2). The
      // lean refetch signal carries no agentName/title — generic copy.
      const focused = useWindowFocusStore.getState().isFocused
      const pageVisible = useFeedbackStore.getState().isOpen
      if (notify && (!focused || !pageVisible)) {
        void notifyDesktop('Agent', 'New feedback')
      }
    }
  })

  // prd-tickets-badge-orphans TB15/TB16 — the badge's freshness follows the
  // window's connection. A stopped daemon sends no feedback event, so
  // without these the badge would never notice it is out of date.
  let lastHostKey = homeHostKey(useConnectHostStore.getState().activeHost)
  let lastRecoveryKind = useConnectHostStore.getState().recovery.kind
  useConnectHostStore.subscribe((s) => {
    const hostKey = homeHostKey(s.activeHost)
    if (hostKey !== lastHostKey) {
      // TB16: the count belongs to one server. Reset before the refresh the
      // new server's projects trigger, so a dead new server shows no badge
      // instead of the last server's number dimmed.
      lastHostKey = hostKey
      lastRecoveryKind = s.recovery.kind
      hostEpoch++
      useFeedbackStore.setState({ waitingCount: 0, waitingStale: false, waitingUnsupported: false })
      return
    }
    const kind = s.recovery.kind
    if (kind !== lastRecoveryKind) {
      // TB15a: the connection left `connected` → keep the number, mark it
      // stale until a refresh succeeds.
      if (lastRecoveryKind === 'connected' && kind !== 'connected') {
        useFeedbackStore.setState({ waitingStale: true })
      }
      lastRecoveryKind = kind
    }
  })
  // TB15b: a recovery heal or an events-socket reopen re-reads the count.
  subscribeSoftResync(() => {
    void useFeedbackStore.getState().refreshWaitingCount()
  })
}
