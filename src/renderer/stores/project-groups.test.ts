// Project-groups store — event wiring for the P4 nav/badge path.
//
// The daemon fires project-group:groups-changed / members-changed /
// poc-changed / message-created on its /events WS and the daemon_events
// bridge re-emits them on the Tauri bus. This suite proves the store's
// `initProjectGroupEvents` wiring (the feedback.test.ts idiom):
//   - all four events are subscribed;
//   - the three STRUCTURAL events bump `revision` immediately and
//     coalesce their list refetch on a trailing 300ms window — a burst
//     fires ONE fetch;
//   - message-created marks the group unread (the Projects-tab badge)
//     UNLESS the user is viewing that group on the Projects page, in
//     which case it's seen on arrival — and it never refetches the list;
//   - selectGroup marks the group seen (clears its unread bit);
//   - P6 (§6.4): seen semantics gate on the chat drawer being VISIBLE —
//     a collapsed drawer accrues unread (its dot) even for the viewed
//     group, and expanding it marks the selected group seen.
//
// vitest env is node — the Tauri event bus + the api module + the
// connect-host store are mocked at the module boundary.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store imports) ──────────────────

const bus = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, fn: (event: { payload: unknown }) => void) => {
    bus.listeners.set(name, fn)
    return () => undefined
  }),
}))

const api = vi.hoisted(() => ({
  fetchProjectGroups: vi.fn(async () => [] as unknown[]),
  fetchUnreadGroupIds: vi.fn(async () => [] as string[]),
}))
vi.mock('@/components/Projects/projects-api', () => ({
  fetchProjectGroups: api.fetchProjectGroups,
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

function fire(name: string, payload: unknown): void {
  const fn = bus.listeners.get(name)
  if (!fn) throw new Error(`no listener registered for ${name}`)
  fn({ payload })
}

describe('project-groups store event wiring', () => {
  beforeEach(async () => {
    initProjectGroupEvents()
    await flush()
    api.fetchProjectGroups.mockClear()
    api.fetchUnreadGroupIds.mockClear()
    usePageViewStore.setState({ page: 'agents' })
    useProjectGroupsStore.setState({
      groups: [],
      selectedGroupId: null,
      unreadGroupIds: new Set<string>(),
      chatCollapsed: false,
    })
  })

  it('subscribes to all four project-group events', async () => {
    await vi.waitFor(() => {
      for (const name of [
        'project-group:groups-changed',
        'project-group:members-changed',
        'project-group:poc-changed',
        'project-group:message-created',
      ]) {
        expect(bus.listeners.has(name), `listener for ${name}`).toBe(true)
      }
    })
  })

  it('a structural-event burst bumps revision per event but fetches ONCE (trailing 300ms)', async () => {
    vi.useFakeTimers()
    try {
      const before = useProjectGroupsStore.getState().revision
      fire('project-group:groups-changed', { groupId: 'g1' })
      fire('project-group:members-changed', { groupId: 'g1' })
      fire('project-group:poc-changed', { groupId: 'g1', pocWorkspaceId: 'w1' })
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

  it('each new structural event resets the coalesce window', async () => {
    vi.useFakeTimers()
    try {
      fire('project-group:groups-changed', { groupId: 'g1' })
      await vi.advanceTimersByTimeAsync(200)
      fire('project-group:groups-changed', { groupId: 'g2' })
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

  it('message-created marks the group unread and bumps revision without a list fetch', async () => {
    vi.useFakeTimers()
    try {
      const before = useProjectGroupsStore.getState().revision
      fire('project-group:message-created', {
        groupId: 'g1',
        groupName: 'release',
        messageId: 'm1',
        author: 'scout',
      })
      const s = useProjectGroupsStore.getState()
      expect(s.revision).toBe(before + 1)
      expect(s.unreadGroupIds.has('g1')).toBe(true)
      await vi.advanceTimersByTimeAsync(1000)
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })

  it('message-created for the group being VIEWED is seen on arrival (no unread)', () => {
    usePageViewStore.setState({ page: 'projects' })
    useProjectGroupsStore.setState({ selectedGroupId: 'g1' })
    fire('project-group:message-created', {
      groupId: 'g1',
      groupName: 'release',
      messageId: 'm2',
      author: 'scout',
    })
    expect(useProjectGroupsStore.getState().unreadGroupIds.has('g1')).toBe(false)
    // …but a DIFFERENT group still goes unread while the page is open.
    fire('project-group:message-created', {
      groupId: 'g2',
      groupName: 'other',
      messageId: 'm3',
      author: 'scout',
    })
    expect(useProjectGroupsStore.getState().unreadGroupIds.has('g2')).toBe(true)
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

  it('message-created for the viewed group goes UNREAD while the drawer is collapsed', () => {
    usePageViewStore.setState({ page: 'projects' })
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: true })
    fire('project-group:message-created', {
      groupId: 'g1',
      groupName: 'release',
      messageId: 'm4',
      author: 'scout',
    })
    // The collapsed drawer shows the unread dot — NOT seen on arrival.
    expect(useProjectGroupsStore.getState().unreadGroupIds.has('g1')).toBe(true)
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
