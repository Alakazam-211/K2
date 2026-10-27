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
// count and bumps `revision` so an open panel reloads.
//
// 0.46.5: the count is every OWNED server's, not only the window's
// (`lib/app-approvals-servers.ts`: the Home pool's servers signed in as
// owner). The window's server pushes; the others are re-read on focus, when
// the pool's login state for a server changes, and on open.

import { create } from 'zustand'
import { onAppApprovalsChanged } from '@/stores/session-events'
import { useConnectHostStore } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import { fetchAppApprovals } from '@/components/Settings/sections/app-approvals-api'
import { hostPool } from '@/lib/host-pool-instance'
import { approvalServers, fetchApprovalsAcross, type ApprovalServer } from '@/lib/app-approvals-servers'

interface AppApprovalsState {
  /** Held App requests waiting on every owned server (0 hides the badge). */
  pending: number
  /** The window's server's share (its push replaces it). */
  windowPending: number
  /** The other owned servers' share, from the last refresh. */
  otherPending: number
  /** Bumped on every push: the open App Approvals panel reloads on it. */
  revision: number
  refresh: () => Promise<void>
}

/** The servers to read right now (the window's own first). */
export function currentApprovalServers(): ApprovalServer[] {
  return approvalServers(hostPool.store.getState().entries, primaryScope())
}

function countPending(requests: ReadonlyArray<{ status: string }>): number {
  return requests.filter((x) => x.status === 'pending').length
}

export const useAppApprovalsStore = create<AppApprovalsState>((set) => ({
  pending: 0,
  windowPending: 0,
  otherPending: 0,
  revision: 0,
  refresh: async () => {
    const host = useConnectHostStore.getState().activeHost
    const results = await fetchApprovalsAcross(
      currentApprovalServers(),
      'pending',
      (state, scope) => fetchAppApprovals(state, scope),
      () => '',
    )
    // A late answer from the server the window just left never lands.
    if (useConnectHostStore.getState().activeHost !== host) return
    set((s) => {
      // An older or unreachable daemon (or a login that can't list them)
      // keeps its last share; the next push, focus or pool change re-reads.
      const own = results.find((r) => r.server.isWindow)
      const windowPending = own?.list ? countPending(own.list.requests) : s.windowPending
      const others = results.filter((r) => !r.server.isWindow)
      const otherPending = others.some((r) => r.list)
        ? others.reduce((n, r) => n + (r.list ? countPending(r.list.requests) : 0), 0)
        : others.length === 0
          ? 0
          : s.otherPending
      return { windowPending, otherPending, pending: windowPending + otherPending }
    })
  },
}))

let initialized = false

/** Wire the push and the refreshes ONCE per window (idempotent). */
export function initAppApprovalsEvents(): void {
  if (initialized) return
  initialized = true
  onAppApprovalsChanged(primaryScope(), (windowPending) => {
    useAppApprovalsStore.setState((s) => ({
      windowPending,
      pending: windowPending + s.otherPending,
      revision: s.revision + 1,
    }))
  })
  // A server switch: the count belongs to the old server until the new one
  // answers.
  useConnectHostStore.subscribe((s, prev) => {
    if (s.activeHost !== prev.activeHost) {
      useAppApprovalsStore.setState((st) => ({ pending: 0, windowPending: 0, otherPending: 0, revision: st.revision + 1 }))
      void useAppApprovalsStore.getState().refresh()
    }
  })
  // Another owned server signed in, or one left (sign-out, role, removed):
  // re-read when the SET of servers changes, never on the pool's routine
  // presence checks.
  let serversKey = currentApprovalServers()
    .map((x) => x.hostKey)
    .join('\n')
  hostPool.store.subscribe(() => {
    const next = currentApprovalServers()
      .map((x) => x.hostKey)
      .join('\n')
    if (next === serversKey) return
    serversKey = next
    useAppApprovalsStore.setState((st) => ({ revision: st.revision + 1 }))
    void useAppApprovalsStore.getState().refresh()
  })
  if (typeof window !== 'undefined') {
    window.addEventListener('focus', () => void useAppApprovalsStore.getState().refresh())
  }
  void useAppApprovalsStore.getState().refresh()
}
