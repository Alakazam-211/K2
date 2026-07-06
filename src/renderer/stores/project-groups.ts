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
import { dropCachedGroupIcon } from '@/components/Projects/group-icon-cache'
import {
  CHAT_PANEL_DEFAULT_WIDTH,
  clampChatPanelWidth,
} from '@/components/Projects/project-chat'

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

// ── P6 (§6.4) — per-client chat-drawer collapse state ─────────────────────
// A plain client preference (not host-keyed, not canonical): whether the
// Dashboard tab's chat drawer is collapsed. It gates SEEN semantics —
// messages are only "seen on arrival" while the drawer is actually
// visible (page open + group selected + drawer expanded); a collapsed
// drawer accrues the unread dot instead.

const CHAT_COLLAPSED_KEY = 'k2:project-groups:chat-collapsed'

function readChatCollapsed(): boolean {
  try {
    return localStorage.getItem(CHAT_COLLAPSED_KEY) === '1'
  } catch {
    return false
  }
}

// ── §6.7.3 polish — per-client chat-panel width ────────────────────────────
// Same idiom as chatCollapsed: a plain client preference (not host-keyed,
// not canonical). Read through the clamp so a corrupt value can never
// wedge the panel; the panel drags live and persists on release.

const CHAT_WIDTH_KEY = 'k2:project-groups:chat-width'

function readChatWidth(): number {
  try {
    return clampChatPanelWidth(parseInt(localStorage.getItem(CHAT_WIDTH_KEY) ?? '', 10))
  } catch {
    return CHAT_PANEL_DEFAULT_WIDTH
  }
}

// ── §6.7.1 — per-client Projects-nav collapse state ────────────────────────
// Same idiom as chatCollapsed: a plain client preference (not host-keyed,
// not canonical) — whether the Projects page hides its left nav. The
// Agents page's sidebar-collapse counterpart, persisted per client.

const NAV_COLLAPSED_KEY = 'k2:project-groups:nav-collapsed'

function readNavCollapsed(): boolean {
  try {
    return localStorage.getItem(NAV_COLLAPSED_KEY) === '1'
  } catch {
    return false
  }
}

/** P5 (§6.2) — a member-row click asking the dashboard to open/focus
 *  that member's canonical pane. The nonce distinguishes repeat clicks
 *  on the same member; the dashboard CONSUMES the request (clears it)
 *  so a later remount never replays a stale one. */
export interface MemberPaneRequest {
  workspaceId: string
  nonce: number
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
  /** Pending open-member-pane click (P5 §6.2) — see MemberPaneRequest. */
  paneRequest: MemberPaneRequest | null
  /** P6 (§6.4) — the project chat panel (§6.7.3: a right-hand side
   *  panel, toggled from the Projects top bar), per-client persisted.
   *  While true, arriving messages go UNREAD even for the viewed group
   *  (the closed panel's toggle shows the dot); opening marks seen. */
  chatCollapsed: boolean
  /** §6.7.3 polish — the chat panel's width in px (clamped, per-client
   *  persisted; the chatCollapsed idiom). */
  chatWidth: number
  /** §6.7.1 — the Projects page's left nav, per-client persisted. */
  navCollapsed: boolean
  /** §6.7.4 — the last-clicked/focused terminal pane per DASHBOARD
   *  (workspaceId, keyed by dashboard id), so Esc can focus it. Plain
   *  session state — never persisted. */
  lastFocusedPaneByDashboard: Record<string, string>
  /** ⌘1…⌘9 — workspaceId → pane number on the CURRENTLY MOUNTED
   *  dashboard (terminal panes among the first 9, reading order),
   *  published by ProjectDashboard so the member drawer/rail rows can
   *  badge them. Session-only; {} while no dashboard is mounted. */
  dashPaneNumbers: Record<string, number>
  fetchGroups: () => Promise<void>
  /** Select a project in the nav; selecting marks it seen (§4.4) —
   *  unless the chat panel is closed (its messages stay unseen). */
  selectGroup: (groupId: string | null) => void
  markGroupSeen: (groupId: string) => void
  /** Close/open the chat panel (persisted per client); opening marks
   *  the selected group seen — its messages are now on screen. */
  setChatCollapsed: (collapsed: boolean) => void
  /** Resize the chat panel (clamped; persisted per client). */
  setChatWidth: (width: number) => void
  /** Collapse/expand the Projects left nav (persisted per client). */
  setNavCollapsed: (collapsed: boolean) => void
  /** §6.7.4 — note the last-used terminal pane of a dashboard. */
  notePaneFocus: (dashboardId: string, workspaceId: string) => void
  /** ⌘1…⌘9 — the mounted dashboard publishes its terminal pane
   *  numbers ({} on unmount). No-ops when the map is unchanged. */
  setDashPaneNumbers: (numbers: Record<string, number>) => void
  /** Member-row click → the dashboard opens/focuses that pane. */
  requestMemberPane: (workspaceId: string) => void
  clearPaneRequest: () => void
}

export const useProjectGroupsStore = create<ProjectGroupsState>((set, get) => ({
  groups: null,
  selectedGroupId: null,
  unreadGroupIds: new Set<string>(),
  revision: 0,
  paneRequest: null,
  chatCollapsed: readChatCollapsed(),
  chatWidth: readChatWidth(),
  navCollapsed: readNavCollapsed(),
  lastFocusedPaneByDashboard: {},
  dashPaneNumbers: {},
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
    // Switching projects drops any pending pane request — it addressed
    // the previous project's dashboard.
    set({ selectedGroupId: groupId, paneRequest: null })
    // A collapsed drawer means the messages are NOT on screen — the
    // unread dot/badge must survive selection (§6.4).
    if (groupId && !get().chatCollapsed) get().markGroupSeen(groupId)
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
  setChatCollapsed: (collapsed) => {
    try {
      localStorage.setItem(CHAT_COLLAPSED_KEY, collapsed ? '1' : '0')
    } catch {
      /* ignore — collapse state degrades to per-session */
    }
    set({ chatCollapsed: collapsed })
    if (!collapsed) {
      const sel = get().selectedGroupId
      if (sel) get().markGroupSeen(sel)
    }
  },
  setChatWidth: (width) => {
    const clamped = clampChatPanelWidth(width)
    try {
      localStorage.setItem(CHAT_WIDTH_KEY, String(clamped))
    } catch {
      /* ignore — width degrades to per-session */
    }
    set({ chatWidth: clamped })
  },
  setNavCollapsed: (collapsed) => {
    try {
      localStorage.setItem(NAV_COLLAPSED_KEY, collapsed ? '1' : '0')
    } catch {
      /* ignore — collapse state degrades to per-session */
    }
    set({ navCollapsed: collapsed })
  },
  notePaneFocus: (dashboardId, workspaceId) => {
    set((s) =>
      s.lastFocusedPaneByDashboard[dashboardId] === workspaceId
        ? {}
        : {
            lastFocusedPaneByDashboard: {
              ...s.lastFocusedPaneByDashboard,
              [dashboardId]: workspaceId,
            },
          },
    )
  },
  setDashPaneNumbers: (numbers) => {
    set((s) => {
      const prev = s.dashPaneNumbers
      const keys = Object.keys(numbers)
      if (
        keys.length === Object.keys(prev).length &&
        keys.every((k) => prev[k] === numbers[k])
      ) {
        return {}
      }
      return { dashPaneNumbers: numbers }
    })
  },
  requestMemberPane: (workspaceId) => {
    set((s) => ({ paneRequest: { workspaceId, nonce: (s.paneRequest?.nonce ?? 0) + 1 } }))
  },
  clearPaneRequest: () => set({ paneRequest: null }),
}))

// Host switch: everything here is keyed by the previous host's group ids
// — reset. The revision bump makes an open Projects page refetch against
// the new host. `chatCollapsed`/`chatWidth`/`navCollapsed` deliberately
// survive — they're plain per-client UI preferences, not host data.
onActiveHostChange(() => {
  useProjectGroupsStore.setState((s) => ({
    groups: null,
    selectedGroupId: null,
    unreadGroupIds: new Set<string>(),
    revision: s.revision + 1,
    paneRequest: null,
    lastFocusedPaneByDashboard: {},
    dashPaneNumbers: {},
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
    // Structural events → coalesced nav refetch + revision. groups-changed
    // also carries set-icon/set-color (§6.7.7) — drop the group's cached
    // icon FIRST so the revision bump makes mounted avatars refetch a
    // fresh upload (no payload groupId → drop all; see group-icon-cache).
    listen<{ groupId?: string }>('project-group:groups-changed', (event) => {
      dropCachedGroupIcon(event.payload?.groupId)
      bumpAndCoalesceRefetch()
    }).catch(warn)
    listen('project-group:members-changed', () => bumpAndCoalesceRefetch()).catch(warn)
    listen('project-group:poc-changed', () => bumpAndCoalesceRefetch()).catch(warn)
    // P5 (§6.3) — a layout save (this client's or another's) → revision
    // bump ONLY, so the open page refetches its `show` view and the
    // dashboard's freshness logic sees the new dashboard revision. NO
    // list refetch (layout changes no list metadata) and explicitly NO
    // live rearrange — an open dashboard only marks itself stale;
    // the latest layout applies on project open/switch.
    listen('project-group:layout-changed', () => {
      useProjectGroupsStore.setState((s) => ({ revision: s.revision + 1 }))
    }).catch(warn)
    // Chat messages → unread bookkeeping (badge) + revision (the P6
    // drawer refetches on it). NOT a list refetch — a message changes no
    // list metadata.
    listen<MessageCreatedPayload>('project-group:message-created', (event) => {
      const { groupId } = event.payload
      const store = useProjectGroupsStore.getState()
      // "Viewing" = the messages are actually on screen: Projects page
      // open, this group selected, AND the chat drawer expanded (§6.4 —
      // a collapsed drawer accrues the unread dot instead).
      const viewingIt =
        usePageViewStore.getState().page === 'projects' &&
        store.selectedGroupId === groupId &&
        !store.chatCollapsed
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
