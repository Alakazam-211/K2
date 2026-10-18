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
//   - message-created bumps `messageRevision` ONLY (0.46.1): no list
//     refetch and no structural revision, so open settings/page views
//     don't refetch on chat lines; unread is the DAEMON's
//     (thread-read-v1): the store posts nothing on it (the open chat
//     panel marks read, the daemon's `project_read` frame moves the
//     badge);
//   - 0.46.1 frames name their project (`groupId`): only that project's
//     `selectGroupRevision` moves; a frame naming none moves every
//     project's (older daemons);
//   - selectGroup marks the group read: POST /cli/project-group/read,
//     only when the daemon's unread store says it has unread;
//   - P6 (§6.4): read semantics gate on the chat drawer being VISIBLE —
//     a collapsed drawer keeps the unread (its dot) even for the viewed
//     group, and expanding it marks the selected group read.
//
// vitest env is node — the session-events registry + the api module +
// the connect-host store are mocked at the module boundary.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store imports) ──────────────────

// session-events registry: record the store's handler so tests can fire
// reasons through it (the daemon-broadcasts.test.ts idiom).
const ev = vi.hoisted(() => ({
  handlers: [] as Array<(reason: string, groupId?: string) => void>,
  hello: [] as Array<() => void>,
  read: [] as Array<(e: Record<string, unknown>) => void>,
}))
vi.mock('@/stores/session-events', async () => {
  // Home M1: every subscription takes the server scope first; these
  // mocks fail loudly unless it is the primary scope.
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    onProjectGroupsChanged: vi.fn((scope: unknown, fn: (reason: string, groupId?: string) => void) => {
      expectPrimaryScope(scope)
      ev.handlers.push(fn)
      return () => void (ev.handlers = ev.handlers.filter((f) => f !== fn))
    }),
    onAppHello: vi.fn((scope: unknown, fn: () => void) => {
      expectPrimaryScope(scope)
      ev.hello.push(fn)
      return () => void (ev.hello = ev.hello.filter((f) => f !== fn))
    }),
    // thread-read-v1: the unread store's live frames.
    onReadStateChanged: vi.fn((scope: unknown, fn: (e: Record<string, unknown>) => void) => {
      expectPrimaryScope(scope)
      ev.read.push(fn)
      return () => undefined
    }),
  }
})

// thread-read-v1: the unread store (real) talks to the daemon here.
const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string) => cli.get(path)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
  }
})

const reconnect = vi.hoisted(() => ({
  connected: [] as Array<() => void>,
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn((fn: () => void) => {
    reconnect.connected.push(fn)
  }),
}))

const api = vi.hoisted(() => ({
  fetchProjectGroups: vi.fn(async () => [] as unknown[]),
  fetchProjectGroupShow: vi.fn(async (_id: string) => ({ members: [] as { workspaceId: string }[] })),
}))
vi.mock('@/components/Projects/projects-api', async () => {
  // Home M1: the wrappers take the scope first; `primaryOnly` checks it is
  // the primary scope and forwards the old arguments.
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    fetchProjectGroups: primaryOnly((...a: []) => api.fetchProjectGroups(...a)),
    fetchProjectGroupShow: primaryOnly((id: string) => api.fetchProjectGroupShow(id)),
  }
})

// connect-host: stable local host; the host-change bus is inert here.
vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => 'local',
  onActiveHostChange: vi.fn(() => () => undefined),
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

import { useProjectGroupsStore, initProjectGroupEvents, selectGroupRevision } from '@/stores/project-groups'
import { dropCachedGroupIcon, getCachedGroupIcon, setCachedGroupIcon } from '@/components/Projects/group-icon-cache'
import { usePageViewStore } from '@/stores/page-view'
import { useUnreadStore, type UnreadEntry } from '@/stores/unread'

/** The daemon's unread state for project groups (the store's shape). */
function seedUnread(...groupIds: string[]): void {
  const byKey: Record<string, UnreadEntry> = {}
  for (const id of groupIds) {
    byKey[`project:${id}`] = {
      kind: 'project',
      id,
      projectId: null,
      lastReadSeq: 1,
      lastSeq: 4,
      unread: 3,
      waiting: 0,
      muted: false,
      mutedUntil: null,
    }
  }
  useUnreadStore.setState({ byKey, total: 3 * groupIds.length, waitingTotal: 0, loaded: true })
}

/** POST /cli/project-group/read calls, by group. */
function readPosts(): string[] {
  return cli.post.mock.calls
    .filter(([path]) => path === 'project-group/read')
    .map(([, body]) => (body as { group: string }).group)
}

function unreadIds(): string[] {
  return Object.values(useUnreadStore.getState().byKey)
    .filter((e) => e.kind === 'project' && e.unread > 0)
    .map((e) => e.id)
    .sort()
}

async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0))
}

function fire(reason: string, groupId?: string): void {
  if (ev.handlers.length === 0) throw new Error('no project-groups handler registered')
  for (const fn of [...ev.handlers]) fn(reason, groupId)
}

describe('project-groups store event wiring', () => {
  beforeEach(async () => {
    initProjectGroupEvents()
    await flush()
    api.fetchProjectGroups.mockReset()
    api.fetchProjectGroups.mockResolvedValue([])
    api.fetchProjectGroupShow.mockReset()
    api.fetchProjectGroupShow.mockResolvedValue({ members: [] })
    cli.get.mockReset()
    cli.get.mockResolvedValue({ ok: true, total: 0, waitingTotal: 0, byConversation: [], muted: [] })
    cli.post.mockReset()
    // The daemon's answer to a read: nothing unread any more.
    cli.post.mockImplementation(async (_path: string, body?: unknown) => ({
      ok: true,
      changed: true,
      kind: 'project',
      id: (body as { group: string }).group,
      seq: 4,
      lastReadSeq: 4,
      lastSeq: 4,
      unread: 0,
      waiting: 0,
      muted: false,
      total: 0,
      waitingTotal: 0,
    }))
    useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
    usePageViewStore.setState({ page: 'agents' })
    useProjectGroupsStore.setState({
      groups: [],
      tagsByWorkspaceId: {},
      selectedGroupId: null,
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

  it('message-created bumps messageRevision ONLY: no list fetch, no structural revision, no read', async () => {
    vi.useFakeTimers()
    try {
      // Even while VIEWING the group: the open panel marks read from the
      // message's seq (useAutoMarkRead); the store's signal handler doesn't.
      usePageViewStore.setState({ page: 'projects' })
      useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: false })
      seedUnread('g1')
      const s0 = useProjectGroupsStore.getState()
      const g1Rev = selectGroupRevision('g1')(s0)
      fire('message-created', 'g1')
      fire('message-created') // an older daemon's frame names no project
      const s1 = useProjectGroupsStore.getState()
      expect(s1.messageRevision).toBe(s0.messageRevision + 2)
      expect(s1.revision).toBe(s0.revision)
      expect(selectGroupRevision('g1')(s1)).toBe(g1Rev)
      await vi.advanceTimersByTimeAsync(1000)
      await vi.advanceTimersByTimeAsync(0) // settle any fetch chain
      expect(api.fetchProjectGroups).not.toHaveBeenCalled()
      expect(readPosts()).toEqual([])
      expect(unreadIds()).toEqual(['g1'])
    } finally {
      vi.useRealTimers()
    }
  })

  it("a structural frame naming a project moves only that project's revision", () => {
    const s0 = useProjectGroupsStore.getState()
    fire('members-changed', 'g1')
    const s1 = useProjectGroupsStore.getState()
    expect(s1.revision).toBe(s0.revision + 1)
    expect(selectGroupRevision('g1')(s1)).toBe(selectGroupRevision('g1')(s0) + 1)
    expect(selectGroupRevision('g2')(s1)).toBe(selectGroupRevision('g2')(s0))
    expect(s1.messageRevision).toBe(s0.messageRevision)
  })

  it("a structural frame naming NO project (older daemon) moves every project's revision", () => {
    const s0 = useProjectGroupsStore.getState()
    fire('poc-changed')
    const s1 = useProjectGroupsStore.getState()
    expect(selectGroupRevision('g1')(s1)).toBe(selectGroupRevision('g1')(s0) + 1)
    expect(selectGroupRevision('g2')(s1)).toBe(selectGroupRevision('g2')(s0) + 1)
  })

  it('groups-changed keeps cached icons when rows carry iconKey, drops that project only otherwise', () => {
    setCachedGroupIcon('local', 'g1', { found: true, dataUrl: 'data:image/png;base64,a', iconKey: 'k' })
    setCachedGroupIcon('local', 'g2', { found: true, dataUrl: 'data:image/png;base64,b', iconKey: 'k2' })
    // 0.46.1 daemon: the list carries iconKey — the cache validates by key.
    useProjectGroupsStore.setState({
      groups: [
        { id: 'g1', name: 'A', pocWorkspaceId: null, pinned: false, color: null, sortOrder: 0, createdAt: 1, updatedAt: 1, memberCount: 0, iconKey: 'k' },
        { id: 'g2', name: 'B', pocWorkspaceId: null, pinned: false, color: null, sortOrder: 0, createdAt: 1, updatedAt: 1, memberCount: 0, iconKey: 'k2' },
      ],
    })
    fire('groups-changed', 'g1')
    expect(getCachedGroupIcon('local', 'g1')).toBeTruthy()
    // Older daemon: no iconKey on the rows — drop the named project only.
    useProjectGroupsStore.setState({
      groups: [
        { id: 'g1', name: 'A', pocWorkspaceId: null, pinned: false, color: null, sortOrder: 0, createdAt: 1, updatedAt: 1, memberCount: 0 },
        { id: 'g2', name: 'B', pocWorkspaceId: null, pinned: false, color: null, sortOrder: 0, createdAt: 1, updatedAt: 1, memberCount: 0 },
      ],
    })
    fire('groups-changed', 'g1')
    expect(getCachedGroupIcon('local', 'g1')).toBeUndefined()
    expect(getCachedGroupIcon('local', 'g2')).toBeTruthy()
    dropCachedGroupIcon()
  })

  it('patchGroup patches one loaded row in place and ignores unknown ids', () => {
    useProjectGroupsStore.setState({
      groups: [
        { id: 'g1', name: 'A', pocWorkspaceId: null, pinned: false, color: null, sortOrder: 0, createdAt: 1, updatedAt: 1, memberCount: 0 },
      ],
    })
    useProjectGroupsStore.getState().patchGroup('g1', { name: 'A2', pinned: true })
    expect(useProjectGroupsStore.getState().groups?.[0]).toMatchObject({ id: 'g1', name: 'A2', pinned: true })
    const before = useProjectGroupsStore.getState().groups
    useProjectGroupsStore.getState().patchGroup('nope', { name: 'X' })
    expect(useProjectGroupsStore.getState().groups).toBe(before)
  })

  it('the Projects badge follows the daemon: a project_read frame clears the group', () => {
    seedUnread('g1', 'g2')
    if (ev.read.length === 0) throw new Error('no read-state handler registered')
    for (const fn of ev.read) {
      fn({ kind: 'project_read', id: 'g1', seq: 4, lastReadSeq: 4, lastSeq: 4, unread: 0, waiting: 0, muted: false, total: 3, waitingTotal: 0 })
    }
    expect(unreadIds()).toEqual(['g2'])
    expect(useUnreadStore.getState().total).toBe(3)
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

  it('selectGroup marks the group read: POST /cli/project-group/read (only it)', async () => {
    seedUnread('g1', 'g2')
    useProjectGroupsStore.getState().selectGroup('g1')
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g1')
    // Optimistic: the dot goes at once.
    expect(unreadIds()).toEqual(['g2'])
    await flush()
    expect(cli.post).toHaveBeenCalledWith('project-group/read', { group: 'g1' })
    expect(readPosts()).toEqual(['g1'])
    expect(unreadIds()).toEqual(['g2'])
  })

  it('selectGroup on a group with nothing unread posts nothing', async () => {
    seedUnread('g2')
    useProjectGroupsStore.getState().selectGroup('g1')
    await flush()
    expect(readPosts()).toEqual([])
  })

  // ── P6 (§6.4) — seen semantics gate on the chat drawer being visible ──

  it('selectGroup does NOT mark read while the drawer is collapsed', async () => {
    seedUnread('g1')
    useProjectGroupsStore.setState({ chatCollapsed: true })
    useProjectGroupsStore.getState().selectGroup('g1')
    await flush()
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g1')
    expect(readPosts()).toEqual([])
    expect(unreadIds()).toEqual(['g1'])
  })

  it('expanding the drawer marks the selected group read (and only it)', async () => {
    seedUnread('g1', 'g2')
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: true })
    useProjectGroupsStore.getState().setChatCollapsed(false)
    await flush()
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(false)
    expect(readPosts()).toEqual(['g1'])
    expect(unreadIds()).toEqual(['g2'])
  })

  it('collapsing the drawer never posts a read', async () => {
    seedUnread('g1', 'g2')
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: false })
    useProjectGroupsStore.getState().setChatCollapsed(true)
    await flush()
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(true)
    expect(readPosts()).toEqual([])
    expect(unreadIds()).toEqual(['g1', 'g2'])
  })

  // ── §6.7.1 — the Projects-nav collapse toggle ─────────────────────────

  it('setNavCollapsed flips the per-client flag and never posts a read', async () => {
    seedUnread('g1')
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', navCollapsed: false, chatCollapsed: true })
    useProjectGroupsStore.getState().setNavCollapsed(true)
    expect(useProjectGroupsStore.getState().navCollapsed).toBe(true)
    useProjectGroupsStore.getState().setNavCollapsed(false)
    expect(useProjectGroupsStore.getState().navCollapsed).toBe(false)
    await flush()
    // Unlike the chat toggle, the nav has NO read semantics.
    expect(readPosts()).toEqual([])
    expect(unreadIds()).toEqual(['g1'])
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

  // ── Nav chips: list membership + isolated show failure ───────────────

  it('skips show when every list row carries memberWorkspaceIds', async () => {
    api.fetchProjectGroups.mockResolvedValue([
      { id: 'g1', name: 'Alpha', color: '#111111', memberWorkspaceIds: ['w1', 'w2'] },
      { id: 'g2', name: 'Beta', color: null, memberWorkspaceIds: [] },
    ])
    await useProjectGroupsStore.getState().fetchGroups()
    expect(api.fetchProjectGroupShow).not.toHaveBeenCalled()
    expect(useProjectGroupsStore.getState().tagsByWorkspaceId).toEqual({
      w1: [{ id: 'g1', name: 'Alpha', color: '#111111' }],
      w2: [{ id: 'g1', name: 'Alpha', color: '#111111' }],
    })
  })

  it('falls back to show when any list row lacks memberWorkspaceIds', async () => {
    api.fetchProjectGroups.mockResolvedValue([
      { id: 'g1', name: 'Alpha', color: null, memberWorkspaceIds: ['w1'] },
      { id: 'g2', name: 'Beta', color: null },
    ])
    api.fetchProjectGroupShow.mockImplementation(async (id: string) => {
      if (id === 'g1') return { members: [{ workspaceId: 'w1' }] }
      return { members: [{ workspaceId: 'w2' }] }
    })
    await useProjectGroupsStore.getState().fetchGroups()
    expect(api.fetchProjectGroupShow).toHaveBeenCalled()
    const tags = useProjectGroupsStore.getState().tagsByWorkspaceId
    expect(tags.w2?.map((t) => t.id)).toEqual(['g2'])
  })

  it('one group show failure does not zero other groups nav tags', async () => {
    api.fetchProjectGroups.mockResolvedValue([
      { id: 'g1', name: 'Alpha', color: null },
      { id: 'g2', name: 'Beta', color: null },
    ])
    api.fetchProjectGroupShow.mockImplementation(async (id: string) => {
      if (id === 'g1') throw new Error('show failed')
      return { members: [{ workspaceId: 'w2' }] }
    })
    await useProjectGroupsStore.getState().fetchGroups()
    const tags = useProjectGroupsStore.getState().tagsByWorkspaceId
    expect(tags.w2).toEqual([{ id: 'g2', name: 'Beta', color: null }])
    expect(tags.w1).toBeUndefined()
  })

  it('non-array show.members does not wipe other groups', async () => {
    api.fetchProjectGroups.mockResolvedValue([
      { id: 'g1', name: 'Alpha', color: null },
      { id: 'g2', name: 'Beta', color: null },
    ])
    api.fetchProjectGroupShow.mockImplementation(async (id: string) => {
      if (id === 'g1') return { members: null as unknown as { workspaceId: string }[] }
      return { members: [{ workspaceId: 'w2' }] }
    })
    await useProjectGroupsStore.getState().fetchGroups()
    expect(useProjectGroupsStore.getState().tagsByWorkspaceId.w2?.[0]?.id).toBe('g2')
  })

  it('first list failure stays null and hello/connect retry the fetch', async () => {
    api.fetchProjectGroups.mockRejectedValueOnce(new Error('daemon down'))
    useProjectGroupsStore.setState({ groups: null, tagsByWorkspaceId: {} })
    await useProjectGroupsStore.getState().fetchGroups()
    expect(useProjectGroupsStore.getState().groups).toBeNull()

    api.fetchProjectGroups.mockClear()
    api.fetchProjectGroups.mockResolvedValue([
      { id: 'g1', name: 'Alpha', color: null, memberWorkspaceIds: ['w1'] },
    ])
    // [0] is this store's (registered at import); [1] is the unread
    // store's (thread-read-v1, wired by initProjectGroupEvents).
    expect(ev.hello).toHaveLength(2)
    expect(reconnect.connected).toHaveLength(2)
    ev.hello[0]!()
    await flush()
    expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
    expect(useProjectGroupsStore.getState().tagsByWorkspaceId.w1?.[0]?.id).toBe('g1')

    useProjectGroupsStore.setState({ groups: null, tagsByWorkspaceId: {} })
    api.fetchProjectGroups.mockClear()
    reconnect.connected[0]!()
    await flush()
    expect(api.fetchProjectGroups).toHaveBeenCalledTimes(1)
  })

  it('hello does not refetch once chips have landed', async () => {
    useProjectGroupsStore.setState({
      groups: [{ id: 'g1', name: 'Alpha', color: null } as never],
      tagsByWorkspaceId: { w1: [{ id: 'g1', name: 'Alpha', color: null }] },
    })
    api.fetchProjectGroups.mockClear()
    ev.hello[0]!()
    reconnect.connected[0]!()
    await flush()
    expect(api.fetchProjectGroups).not.toHaveBeenCalled()
  })
})
