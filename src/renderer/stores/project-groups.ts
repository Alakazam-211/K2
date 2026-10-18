// Projects V1 P4 — nav/list state + live-update wiring for project
// GROUPS (prd-projects-v1 §6.1), mirroring stores/feedback.ts:
//
// The daemon fires `project-group:*` HookEvents (project_group_routes.rs)
// and — remote live-update fix — mirrors every one of them onto the
// HOST-AWARE `/cli/sessions/events` bus as an app-level
// `project_groups_changed` refetch signal (`reason` = the hook name
// minus its `project-group:` prefix). This store subscribes via
// `onProjectGroupsChanged` (stores/session-events.ts), which — unlike
// the old Tauri `listen('project-group:*')` wiring riding the
// loopback-only `/events` WS — also fires when the app is connected to
// a REMOTE host over K2 Connect. The three STRUCTURAL reasons
// (groups-changed / members-changed / poc-changed) coalesce-refetch the
// group list on a trailing 300ms window (N rapid events → ONE fetch —
// the FeedbackItemView idiom); every reason also bumps `revision`
// immediately so the open Projects page refetches its selected group's
// `show` view.
//
// Unread (thread-read-v1, 0.46.1): the Projects-tab badge and the nav dots
// are the DAEMON's per-person read cursors (`stores/unread.ts`), not a
// per-client localStorage cursor any more. Selecting a group / opening its
// chat posts `/cli/project-group/read` (`markGroupSeen`); the open chat
// panel marks new messages read while it is on screen
// (`useAutoMarkRead`). Every device of the person sees the same dots.

import { create } from 'zustand'
import { onDaemonConnected } from '@/lib/daemon-reconnect'
import { activeHostKey, onActiveHostChange, useConnectHostStore } from '@/stores/connect-host'
import { onAppHello, onProjectGroupsChanged } from '@/stores/session-events'
import {
  fetchProjectGroups,
  fetchProjectGroupShow,
  type ProjectGroup,
} from '@/components/Projects/projects-api'
import { initUnreadEvents, unreadKey, useUnreadStore } from '@/stores/unread'
import {
  buildTagsByWorkspaceId,
  type ProjectNavTag,
} from '@/lib/nav-project-tags'
import { dropCachedGroupIcon } from '@/components/Projects/group-icon-cache'
import {
  CHAT_PANEL_DEFAULT_WIDTH,
  clampChatPanelWidth,
} from '@/components/Projects/project-chat'
import { primaryScope } from '@/kessel/server-scope'

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
  /** When set, open/focus an `htmlDoc` resource pane instead of the member terminal. */
  filePath?: string
}

interface ProjectGroupsState {
  /** null = not fetched yet (the page shows a loading state). */
  groups: ProjectGroup[] | null
  /** workspace registry id → project-group chips for the main nav. */
  tagsByWorkspaceId: Record<string, ProjectNavTag[]>
  /** The nav's selected project (drives the main area + member drawer). */
  selectedGroupId: string | null
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
  /** Select a project in the nav; selecting marks it read (§4.4) —
   *  unless the chat panel is closed (its messages stay unread). */
  selectGroup: (groupId: string | null) => void
  /** Read the group's chat up to its newest message: POST
   *  `/cli/project-group/read` when the daemon says it has unread. */
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
  /** Open or focus a resource file pane on the mounted project dashboard. */
  requestFileDocPane: (workspaceId: string, filePath: string) => void
  clearPaneRequest: () => void
}

export const useProjectGroupsStore = create<ProjectGroupsState>((set, get) => ({
  groups: null,
  tagsByWorkspaceId: {},
  selectedGroupId: null,
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
      const groups = await fetchProjectGroups(primaryScope())
      if (activeHostKey(useConnectHostStore.getState().activeHost) !== hostKey) return
      set({ groups })
      const membersByGroupId: Record<string, string[]> = {}
      const listCarriesMembers = groups.every((g) => Array.isArray(g.memberWorkspaceIds))
      if (listCarriesMembers) {
        for (const g of groups) {
          membersByGroupId[g.id] = g.memberWorkspaceIds ?? []
        }
      } else {
        await Promise.all(
          groups.map(async (g) => {
            try {
              const show = await fetchProjectGroupShow(primaryScope(), g.id)
              if (!Array.isArray(show.members)) return
              membersByGroupId[g.id] = show.members.map((m) => m.workspaceId)
            } catch {
              // One group's show failure must not wipe others' chips.
            }
          }),
        )
      }
      if (activeHostKey(useConnectHostStore.getState().activeHost) !== hostKey) return
      set({ tagsByWorkspaceId: buildTagsByWorkspaceId(groups, membersByGroupId) })
      // Drop a selection whose group vanished (delete on another client).
      const sel = get().selectedGroupId
      if (sel && !groups.some((g) => g.id === sel)) set({ selectedGroupId: null })
    } catch (err) {
      console.warn('[project-groups] list fetch failed:', err)
      // Leave `groups` as-is. First load stays `null` (loading) so
      // onDaemonConnected / onAppHello can retry; a successful empty
      // list is the only path that writes `[]`.
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
    // Absent from the unread store = nothing unread: no request.
    const entry = useUnreadStore.getState().byKey[unreadKey('project', groupId)]
    if (!entry || entry.unread <= 0) return
    void useUnreadStore.getState().markRead('project', groupId)
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
  requestFileDocPane: (workspaceId, filePath) => {
    set((s) => ({
      paneRequest: {
        workspaceId,
        filePath,
        nonce: (s.paneRequest?.nonce ?? 0) + 1,
      },
    }))
  },
  clearPaneRequest: () => set({ paneRequest: null }),
}))

// ── Event wiring ──────────────────────────────────────────────────────────

let eventsInitialized = false
let refetchTimer: ReturnType<typeof setTimeout> | null = null

/** True when chips never landed, were wiped on a host switch, or a first
 *  list fetch failed (groups stays `null`). Successful empty lists still
 *  retry — a cheap GET, and the only way to recover from `[]` that was
 *  never a real empty roster. */
function groupsNeedRefetch(): boolean {
  const s = useProjectGroupsStore.getState()
  return s.groups === null || Object.keys(s.tagsByWorkspaceId).length === 0
}

function refetchGroupsIfNeeded(): void {
  if (!groupsNeedRefetch()) return
  void useProjectGroupsStore.getState().fetchGroups()
}

// Host switch: everything here is keyed by the previous host's group ids
// — reset, then refetch against the new host (projects-store idiom).
// `initProjectGroupEvents` is module-idempotent, so the App remount after
// a switch does NOT refetch on its own. `chatCollapsed`/`chatWidth`/
// `navCollapsed` deliberately survive — they're per-client UI prefs.
onActiveHostChange(() => {
  if (refetchTimer !== null) {
    clearTimeout(refetchTimer)
    refetchTimer = null
  }
  useProjectGroupsStore.setState((s) => ({
    groups: null,
    tagsByWorkspaceId: {},
    selectedGroupId: null,
    revision: s.revision + 1,
    paneRequest: null,
    lastFocusedPaneByDashboard: {},
    dashPaneNumbers: {},
  }))
  void useProjectGroupsStore.getState().fetchGroups()
})

// Boot/reconnect: the first list GET can race the daemon coming up, and
// a host switch can land before Hello. Retry only when groups never
// landed or were wiped — do not fan out another N-show on every hello
// once chips are painted.
onDaemonConnected(refetchGroupsIfNeeded)
onAppHello(primaryScope(), refetchGroupsIfNeeded)

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

/** Wire the project-group event listeners ONCE per window (idempotent —
 *  the top-bar switcher mounts/unmounts across layout switches but the
 *  subscription must survive them; the initFeedbackEvents idiom).
 *
 *  Remote live-update fix: registers on the HOST-AWARE session-events bus
 *  (`onProjectGroupsChanged`, app-level `project_groups_changed` frames)
 *  instead of the old Tauri `listen('project-group:*')` wiring — the
 *  legacy bus is loopback-only, so a client connected to a remote host
 *  over K2 Connect never saw member/PoC/chat changes without navigating
 *  away and back. The registration is deliberately never torn down
 *  (module-lifetime), exactly like the old listeners. */
export function initProjectGroupEvents(): void {
  if (eventsInitialized) return
  eventsInitialized = true

  // Boot: the nav chips, and the daemon's unread summary (the Projects-tab
  // badge must be right before the page is ever opened).
  void useProjectGroupsStore.getState().fetchGroups()
  initUnreadEvents()

  onProjectGroupsChanged(primaryScope(), (reason) => {
    switch (reason) {
      // Structural — coalesced nav refetch + revision. groups-changed
      // also covers set-icon/set-color (§6.7.7) — drop the cached icons
      // FIRST so the revision bump makes mounted avatars refetch a fresh
      // upload. (The lean signal carries no groupId, so drop ALL cached
      // icons — see group-icon-cache; coarser than the old payload-keyed
      // drop but the next mount refetches each avatar anyway.)
      case 'groups-changed':
        dropCachedGroupIcon()
        bumpAndCoalesceRefetch()
        break
      case 'members-changed':
      case 'poc-changed':
        bumpAndCoalesceRefetch()
        break
      // P5 (§6.3) — a layout save (this client's or another's) → revision
      // bump ONLY, so the open page refetches its `show` view and the
      // dashboard's freshness logic sees the new dashboard revision. NO
      // list refetch (layout changes no list metadata) and explicitly NO
      // live rearrange — an open dashboard only marks itself stale;
      // the latest layout applies on project open/switch.
      case 'layout-changed':
        useProjectGroupsStore.setState((s) => ({ revision: s.revision + 1 }))
        break
      // Chat messages → revision (the open chat panel refetches on it).
      // Unread is the daemon's: its `project_read` frame moves the badge,
      // and the open panel marks the new message read while it is on
      // screen (ProjectChatPanel → useAutoMarkRead).
      case 'message-created':
        bumpAndCoalesceRefetch()
        break
      // Forward compat — an unknown reason from a newer daemon is still
      // a "something changed" signal; do the safe full refresh.
      default:
        bumpAndCoalesceRefetch()
    }
  })
}
