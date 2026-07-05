// Projects V1 P4 — nav/list state + live-update wiring for project
// GROUPS (prd-projects-v1 §6.1), mirroring stores/feedback.ts:
//
// The daemon fires `project-group:*` HookEvents on its `/events` WS
// (project_group_routes.rs) and src-tauri's daemon_events.rs re-emits
// every frame on the Tauri event bus under the wire name. The renderer
// listens for the three STRUCTURAL events (groups-changed /
// members-changed / poc-changed) and coalesce-refetches the group list on
// a trailing 300ms window (N rapid events → ONE fetch — the
// FeedbackItemView idiom); every event also bumps `revision` immediately
// so the open Projects page refetches its selected group's `show` view.
//
// `project-group:message-created` drives the Projects-tab unread badge
// (§4.4): a per-client last-seen read cursor per group (localStorage —
// canonical-structure vs per-client-view principle) — the badge counts
// groups with messages newer than last-seen. Viewing a group marks it
// seen; boot/refetch reconciles via a messages?after=&limit=1 probe.

import { create } from 'zustand'
import { activeHostKey, onActiveHostChange, useConnectHostStore } from '@/stores/connect-host'
import { usePageViewStore } from '@/stores/page-view'
import {
  fetchProjectGroups,
  fetchUnreadGroupIds,
  type ProjectGroup,
} from '@/components/Projects/projects-api'

// ── Per-client last-seen read cursor (§4.4, resolved Q5) ─────────────────
// Keyed per host + group so remoting doesn't cross-contaminate cursors.

function lastSeenKey(groupId: string): string {
  const host = activeHostKey(useConnectHostStore.getState().activeHost)
  return `k2:project-groups:last-seen:${host}:${groupId}`
}

export function getLastSeen(groupId: string): number {
  try {
    return parseInt(localStorage.getItem(lastSeenKey(groupId)) ?? '0', 10) || 0
  } catch {
    return 0
  }
}

function stampLastSeen(groupId: string): void {
  try {
    localStorage.setItem(lastSeenKey(groupId), String(Math.floor(Date.now() / 1000)))
  } catch {
    /* ignore — badge degrades to per-session */
  }
}

interface ProjectGroupsState {
  /** null = not fetched yet (the page shows a loading state). */
  groups: ProjectGroup[] | null
  /** The nav's selected project (drives the main area + member drawer). */
  selectedGroupId: string | null
  /** Groups with chat messages newer than their last-seen cursor —
   *  `.size` is the Projects-tab badge count. */
  unreadGroupIds: Set<string>
  /** Bumped on every project-group event so open views refetch. */
  revision: number
  fetchGroups: () => Promise<void>
  /** Select a project in the nav; selecting marks it seen (§4.4). */
  selectGroup: (groupId: string | null) => void
  markGroupSeen: (groupId: string) => void
}

export const useProjectGroupsStore = create<ProjectGroupsState>((set, get) => ({
  groups: null,
  selectedGroupId: null,
  unreadGroupIds: new Set<string>(),
  revision: 0,
  fetchGroups: async () => {
    // Capture the host so a slow response from the PREVIOUS host can
    // never land after a switch (projects-store idiom).
    const hostKey = activeHostKey(useConnectHostStore.getState().activeHost)
    try {
      const groups = await fetchProjectGroups()
      if (activeHostKey(useConnectHostStore.getState().activeHost) !== hostKey) return
      set({ groups })
      // Drop a selection whose group vanished (delete on another client).
      const sel = get().selectedGroupId
      if (sel && !groups.some((g) => g.id === sel)) set({ selectedGroupId: null })
      // §4.4 reconciliation — advisory; failures leave the set as-is.
      const unread = await fetchUnreadGroupIds(groups, getLastSeen)
      if (activeHostKey(useConnectHostStore.getState().activeHost) !== hostKey) return
      set({ unreadGroupIds: new Set(unread) })
    } catch (err) {
      console.warn('[project-groups] list fetch failed:', err)
      // Leave `groups` as-is when already loaded; first load surfaces
      // the empty-ish state rather than spinning forever.
      if (get().groups === null) set({ groups: [] })
    }
  },
  selectGroup: (groupId) => {
    set({ selectedGroupId: groupId })
    if (groupId) get().markGroupSeen(groupId)
  },
  markGroupSeen: (groupId) => {
    stampLastSeen(groupId)
    set((s) => {
      if (!s.unreadGroupIds.has(groupId)) return {}
      const next = new Set(s.unreadGroupIds)
      next.delete(groupId)
      return { unreadGroupIds: next }
    })
  },
}))

// Host switch: everything here is keyed by the previous host's group ids
// — reset. The revision bump makes an open Projects page refetch against
// the new host.
onActiveHostChange(() => {
  useProjectGroupsStore.setState((s) => ({
    groups: null,
    selectedGroupId: null,
    unreadGroupIds: new Set<string>(),
    revision: s.revision + 1,
  }))
})

// ── Event wiring ──────────────────────────────────────────────────────────

let eventsInitialized = false
let refetchTimer: ReturnType<typeof setTimeout> | null = null

/** Bump `revision` immediately (open views refetch on it) and schedule
 *  the list refetch on a trailing 300ms window — each new event resets
 *  the timer, so a burst fires ONE fetch. */
function bumpAndCoalesceRefetch(): void {
  useProjectGroupsStore.setState((s) => ({ revision: s.revision + 1 }))
  if (refetchTimer !== null) clearTimeout(refetchTimer)
  refetchTimer = setTimeout(() => {
    refetchTimer = null
    void useProjectGroupsStore.getState().fetchGroups()
  }, 300)
}

/** Event payload contract — frozen in project_group_routes.rs. */
interface MessageCreatedPayload {
  groupId: string
  groupName: string
  messageId: string
  author: string
}

/** Wire the project-group event listeners ONCE per window (idempotent —
 *  the top-bar switcher mounts/unmounts across layout switches but the
 *  subscription must survive them; the initFeedbackEvents idiom). */
export function initProjectGroupEvents(): void {
  if (eventsInitialized) return
  eventsInitialized = true

  // Boot reconciliation: the Projects-tab badge (§4.4) must be right
  // before the page is ever opened — one list fetch + the unread probe.
  void useProjectGroupsStore.getState().fetchGroups()

  void import('@tauri-apps/api/event').then(({ listen }) => {
    // listen() rejects outside Tauri (vitest) — warn, don't blow up.
    const warn = (err: unknown): void =>
      console.warn('[project-groups] listen() unavailable:', err)
    // Structural events → coalesced nav refetch + revision.
    listen('project-group:groups-changed', () => bumpAndCoalesceRefetch()).catch(warn)
    listen('project-group:members-changed', () => bumpAndCoalesceRefetch()).catch(warn)
    listen('project-group:poc-changed', () => bumpAndCoalesceRefetch()).catch(warn)
    // Chat messages → unread bookkeeping (badge) + revision (the P6
    // drawer refetches on it). NOT a list refetch — a message changes no
    // list metadata.
    listen<MessageCreatedPayload>('project-group:message-created', (event) => {
      const { groupId } = event.payload
      const store = useProjectGroupsStore.getState()
      const viewingIt =
        usePageViewStore.getState().page === 'projects' && store.selectedGroupId === groupId
      if (viewingIt) {
        // On screen right now — it's seen the moment it lands.
        store.markGroupSeen(groupId)
      } else {
        useProjectGroupsStore.setState((s) => {
          if (s.unreadGroupIds.has(groupId)) return {}
          const next = new Set(s.unreadGroupIds)
          next.add(groupId)
          return { unreadGroupIds: next }
        })
      }
      useProjectGroupsStore.setState((s) => ({ revision: s.revision + 1 }))
    }).catch(warn)
  })
}
