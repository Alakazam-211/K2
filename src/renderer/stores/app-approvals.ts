// Settings → App Approvals: the pending count (the nav badge) and the
// reload signal for the open panel (0.46.4 field report: a held
// `k2 app install` sat in `k2 app approvals`, but the owner's App Approvals
// showed nothing — it loaded once on mount and on window focus, never on
// the daemon's push, and nothing counted it anywhere).
//
// Thin client: the daemon owns the list and the count. It pushes
// `app_approvals_changed {pending}` on the host-aware `/cli/sessions/events`
// bus whenever a held App request is filed, asked again or decided, so a
// window connected over K2 Connect hears it too. This store keeps the
// window's server's count and bumps `revision` so an open panel reloads.

import { create } from 'zustand'
import { onAppApprovalsChanged } from '@/stores/session-events'
import { useConnectHostStore } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import { fetchAppApprovals } from '@/components/Settings/sections/app-approvals-api'

interface AppApprovalsState {
  /** Held App requests waiting on the window's server (0 hides the badge). */
  pending: number
  /** Bumped on every push: the open App Approvals panel reloads on it. */
  revision: number
  refresh: () => Promise<void>
}

export const useAppApprovalsStore = create<AppApprovalsState>((set) => ({
  pending: 0,
  revision: 0,
  refresh: async () => {
    const host = useConnectHostStore.getState().activeHost
    try {
      const r = await fetchAppApprovals('pending')
      // A late answer from the server the window just left never lands.
      if (useConnectHostStore.getState().activeHost !== host) return
      set({ pending: r.requests.filter((x) => x.status === 'pending').length })
    } catch {
      // An older or unreachable daemon (or a login that can't list them):
      // keep the last count; the next push or focus re-reads it.
    }
  },
}))

let initialized = false

/** Wire the push and the refreshes ONCE per window (idempotent). */
export function initAppApprovalsEvents(): void {
  if (initialized) return
  initialized = true
  onAppApprovalsChanged(primaryScope(), (pending) => {
    useAppApprovalsStore.setState((s) => ({ pending, revision: s.revision + 1 }))
  })
  // A server switch: the count belongs to the old server until the new one
  // answers.
  useConnectHostStore.subscribe((s, prev) => {
    if (s.activeHost !== prev.activeHost) {
      useAppApprovalsStore.setState((st) => ({ pending: 0, revision: st.revision + 1 }))
      void useAppApprovalsStore.getState().refresh()
    }
  })
  if (typeof window !== 'undefined') {
    window.addEventListener('focus', () => void useAppApprovalsStore.getState().refresh())
  }
  void useAppApprovalsStore.getState().refresh()
}
