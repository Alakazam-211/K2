// Project-groups store — event wiring for the P4 nav/badge path.
//
// Remote live-update fix: the daemon mirrors every `project-group:*`
// HookEvent onto the HOST-AWARE /cli/sessions/events bus as an app-level
// `project_groups_changed` refetch signal, and the store subscribes via
// `onProjectGroupsChanged(reason)` (stores/session-events.ts) instead of
// the old loopback-only Tauri `listen('project-group:*')` wiring. This
// suite proves the store's `initProjectGroupEvents` wiring per reason
// (the feedback.test.ts idiom):
//   - one registration on the session-events registry;
//   - the three STRUCTURAL reasons bump `revision` immediately and
//     coalesce their list refetch on a trailing 300ms window — a burst
//     fires ONE fetch;
//   - layout-changed bumps `revision` ONLY (no list fetch — a layout
//     save changes no list metadata);
//   - message-created bumps `revision` and schedules the coalesced
//     refetch (its fetchUnreadGroupIds probe reconciles the unread
//     badge — the lean signal carries no groupId); while the user is
//     VIEWING a group's chat (Projects page + selected + drawer
//     expanded) the selected group is stamped seen on arrival;
//   - selectGroup marks the group seen (clears its unread bit);
//   - P6 (§6.4): seen semantics gate on the chat drawer being VISIBLE —
//     a collapsed drawer accrues unread (its dot) even for the viewed
//     group, and expanding it marks the selected group seen.
//
// vitest env is node — the session-events registry + the api module +
// the connect-host store are mocked at the module boundary.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store imports) ──────────────────

// session-events registry: record the store's handler so tests can fire
// reasons through it (the daemon-broadcasts.test.ts idiom).
const ev = vi.hoisted(() => ({
  handlers: [] as Array<(reason: string) => void>,
}))
vi.mock('@/stores/session-events', () => ({
  onProjectGroupsChanged: vi.fn((fn: (reason: string) => void) => {
    ev.handlers.push(fn)
    return () => void (ev.handlers = ev.handlers.filter((f) => f !== fn))
  }),
}))

const api = vi.hoisted(() => ({
  fetchProjectGroups: vi.fn(async () => [] as unknown[]),
  fetchProjectGroupShow: vi.fn(async () => ({ members: [] as { workspaceId: string }[] })),
  fetchUnreadGroupIds: vi.fn(async () => [] as string[]),
}))
vi.mock('@/components/Projects/projects-api', () => ({
  fetchProjectGroups: api.fetchProjectGroups,
  fetchProjectGroupShow: api.fetchProjectGroupShow,
  fetchUnreadGroupIds: api.fetchUnreadGroupIds,
}))

// connect-host: stable local host; the host-change bus is inert here.
vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => 'local',
  onActiveHostChange: vi.fn(() => () => undefined),
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

import { useProjectGroupsStore, initProjectGroupEvents } from '@/stores/project-groups'
import { usePageViewStore } from '@/stores/page-view'

async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0))
}

function fire(reason: string): void {
  if (ev.handlers.length === 0) throw new Error('no project-groups handler registered')
  for (const fn of [...ev.handlers]) fn(reason)
}

describe('project-groups store event wiring', () => {
  beforeEach(async () => {
    initProjectGroupEvents()
    await flush()
    api.fetchProjectGroups.mockClear()
    api.fetchUnreadGroupIds.mockClear()
    api.fetchUnreadGroupIds.mockResolvedValue([])
    usePageViewStore.setState({ page: 'agents' })
    useProjectGroupsStore.setState({
      groups: [],
      tagsByWorkspaceId: {},
      selectedGroupId: null,
      unreadGroupIds: new Set<string>(),
      chatCollapsed: false,
    })
  })

  it('registers ONE handler on the session-events registry', () => {
    // initProjectGroupEvents is idempotent — repeated beforeEach inits
    // must not stack registrations.
    expect(ev.handlers).toHaveLength(1)
  })

  it('a structural-reason burst bumps revision per event but fetches ONCE (trailing 300ms)', async () => {
    vi.useFakeTimers()
    try {
      const before = useProjectGroupsStore.getState().revision
      fire('groups-changed')
      fire('members-changed')
      fire('poc-changed')
      expect(useProjectGroupsStore.getState().revision).toBe(before + 3)
      // Inside the window — nothing fetched yet.
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(299)
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(1)
      expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('each new structural reason resets the coalesce window', async () => {
    vi.useFakeTimers()
    try {
      fire('groups-changed')
      await vi.advanceTimersByTimeAsync(200)
      fire('groups-changed')
      await vi.advanceTimersByTimeAsync(200)
      // 400ms after the FIRST event but only 200ms after the second —
      // the trailing window hasn't elapsed.
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(100)
      expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('layout-changed bumps revision WITHOUT any list fetch', async () => {
    vi.useFakeTimers()
    try {
      const before = useProjectGroupsStore.getState().revision
      fire('layout-changed')
      expect(useProjectGroupsStore.getState().revision).toBe(before + 1)
      await vi.advanceTimersByTimeAsync(1000)
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })

  it('message-created bumps revision and reconciles unread via the coalesced probe', async () => {
    vi.useFakeTimers()
    try {
      // The lean signal carries no groupId — which group went unread is
      // the fetchUnreadGroupIds probe's answer (messages newer than each
      // group's last-seen cursor).
      api.fetchUnreadGroupIds.mockResolvedValue(['g1'])
      const before = useProjectGroupsStore.getState().revision
      fire('message-created')
      expect(useProjectGroupsStore.getState().revision).toBe(before + 1)
      // Inside the window — nothing fetched yet.
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(300)
      await vi.advanceTimersByTimeAsync(0) // settle the fetch chain
      expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
      expect(useProjectGroupsStore.getState().unreadGroupIds.has('g1')).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('message-created while VIEWING a group stamps the selected group seen on arrival', async () => {
    vi.useFakeTimers()
    try {
      usePageViewStore.setState({ page: 'projects' })
      useProjectGroupsStore.setState({
        selectedGroupId: 'g1',
        unreadGroupIds: new Set(['g1']),
        chatCollapsed: false,
      })
      fire('message-created')
      // The viewed group's messages are on screen — seen the moment they
      // land (its unread bit clears immediately, before any reconcile).
      expect(useProjectGroupsStore.getState().unreadGroupIds.has('g1')).toBe(false)
      // Drain the coalesce timer so it can't leak into a later test.
      await vi.advanceTimersByTimeAsync(300)
    } finally {
      vi.useRealTimers()
    }
  })

  it('unknown reasons (forward compat) fall back to the coalesced refetch', async () => {
    vi.useFakeTimers()
    try {
      const before = useProjectGroupsStore.getState().revision
      fire('some-newer-daemon-reason')
      expect(useProjectGroupsStore.getState().revision).toBe(before + 1)
      await vi.advanceTimersByTimeAsync(300)
      expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('selectGroup marks the group seen (clears its unread bit)', () => {
    useProjectGroupsStore.setState({ unreadGroupIds: new Set(['g1', 'g2']) })
    useProjectGroupsStore.getState().selectGroup('g1')
    const s = useProjectGroupsStore.getState()
    expect(s.selectedGroupId).toBe('g1')
    expect(s.unreadGroupIds.has('g1')).toBe(false)
    expect(s.unreadGroupIds.has('g2')).toBe(true)
  })

  // ── P6 (§6.4) — seen semantics gate on the chat drawer being visible ──

  it('message-created for the viewed group is NOT stamped seen while the drawer is collapsed', async () => {
    vi.useFakeTimers()
    try {
      usePageViewStore.setState({ page: 'projects' })
      useProjectGroupsStore.setState({
        selectedGroupId: 'g1',
        unreadGroupIds: new Set(['g1']),
        chatCollapsed: true,
      })
      fire('message-created')
      // The collapsed drawer shows the unread dot — NOT seen on arrival.
      expect(useProjectGroupsStore.getState().unreadGroupIds.has('g1')).toBe(true)
      // Drain the coalesce timer so it can't leak into a later test.
      await vi.advanceTimersByTimeAsync(300)
    } finally {
      vi.useRealTimers()
    }
  })

  it('selectGroup does NOT mark seen while the drawer is collapsed', () => {
    useProjectGroupsStore.setState({
      unreadGroupIds: new Set(['g1']),
      chatCollapsed: true,
    })
    useProjectGroupsStore.getState().selectGroup('g1')
    const s = useProjectGroupsStore.getState()
    expect(s.selectedGroupId).toBe('g1')
    expect(s.unreadGroupIds.has('g1')).toBe(true)
  })

  it('expanding the drawer marks the selected group seen (and only it)', () => {
    useProjectGroupsStore.setState({
      selectedGroupId: 'g1',
      unreadGroupIds: new Set(['g1', 'g2']),
      chatCollapsed: true,
    })
    useProjectGroupsStore.getState().setChatCollapsed(false)
    const s = useProjectGroupsStore.getState()
    expect(s.chatCollapsed).toBe(false)
    expect(s.unreadGroupIds.has('g1')).toBe(false)
    expect(s.unreadGroupIds.has('g2')).toBe(true)
  })

  it('collapsing the drawer never touches unread state', () => {
    useProjectGroupsStore.setState({
      selectedGroupId: 'g1',
      unreadGroupIds: new Set(['g2']),
      chatCollapsed: false,
    })
    useProjectGroupsStore.getState().setChatCollapsed(true)
    const s = useProjectGroupsStore.getState()
    expect(s.chatCollapsed).toBe(true)
    expect(s.unreadGroupIds.has('g2')).toBe(true)
  })

  // ── §6.7.1 — the Projects-nav collapse toggle ─────────────────────────

  it('setNavCollapsed flips the per-client flag and never touches unread/seen state', () => {
    useProjectGroupsStore.setState({
      selectedGroupId: 'g1',
      unreadGroupIds: new Set(['g1']),
      navCollapsed: false,
    })
    useProjectGroupsStore.getState().setNavCollapsed(true)
    let s = useProjectGroupsStore.getState()
    expect(s.navCollapsed).toBe(true)
    // Unlike the chat toggle, the nav has NO seen semantics.
    expect(s.unreadGroupIds.has('g1')).toBe(true)
    useProjectGroupsStore.getState().setNavCollapsed(false)
    s = useProjectGroupsStore.getState()
    expect(s.navCollapsed).toBe(false)
    expect(s.unreadGroupIds.has('g1')).toBe(true)
  })

  // ── §6.7.4 — last-used pane tracking (Esc-to-pane) ────────────────────

  it('notePaneFocus tracks the last pane PER dashboard', () => {
    useProjectGroupsStore.setState({ lastFocusedPaneByDashboard: {} })
    const store = useProjectGroupsStore.getState()
    store.notePaneFocus('dash-1', 'w1')
    store.notePaneFocus('dash-2', 'w9')
    store.notePaneFocus('dash-1', 'w2') // later click wins
    expect(useProjectGroupsStore.getState().lastFocusedPaneByDashboard).toEqual({
      'dash-1': 'w2',
      'dash-2': 'w9',
    })
  })

  it('notePaneFocus is a state no-op when the pane is already the last one', () => {
    useProjectGroupsStore.setState({ lastFocusedPaneByDashboard: { 'dash-1': 'w1' } })
    const before = useProjectGroupsStore.getState().lastFocusedPaneByDashboard
    useProjectGroupsStore.getState().notePaneFocus('dash-1', 'w1')
    // Same object identity — no churn for subscribers.
    expect(useProjectGroupsStore.getState().lastFocusedPaneByDashboard).toBe(before)
  })
})
