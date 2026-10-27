// @vitest-environment jsdom
// Mark all as read (thread-read-all-v1, 0.46.5; Rosson 2026-10-26: "a
// message that says unread but I can never read it"):
//   - on the window's own server: the agent's unread drops at once (its
//     waiting cards stay, other agents untouched), `thread/read-all
//     {projectId}` goes to that server, and the store re-reads the
//     daemon's truth afterwards, also when the POST fails (the count comes
//     back and the call throws);
//   - on another server: the POST goes through THAT server's scope and the
//     window's store is never touched;
//   - the menu row shows only with unread to clear, on a server that
//     reports the key, through a scope that may send it (never view-only);
//   - the room write allowlist carries `thread/read-all`.

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/stores/session-events', () => ({
  onReadStateChanged: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
}))
vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => 'local',
  onActiveHostChange: vi.fn(() => () => undefined),
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

const cli = vi.hoisted(() => ({
  calls: [] as Array<{ method: string; host: string; path: string; body?: unknown }>,
  summary: null as unknown,
  postFails: false,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, path: string) => {
    cli.calls.push({ method: 'GET', host: scope.hostKey, path })
    if (path !== 'thread/unread') throw new Error(`unexpected GET ${path}`)
    return cli.summary
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, path: string, body: unknown) => {
    cli.calls.push({ method: 'POST', host: scope.hostKey, path, body })
    if (cli.postFails) throw new Error('read-all refused')
    return { ok: true, changed: true, projectId: 'p1', marked: ['c1', 'c-old'], unread: 0, waiting: 1, muted: false, total: 2, waitingTotal: 1 }
  }),
}))

import {
  __resetUnreadForTests,
  markAgentAllRead,
  markAllReadSupported,
  threadSumForProject,
  useUnreadStore,
} from '@/stores/unread'
import { markAllReadMenuItems, MARK_ALL_READ_MENU_ID } from '@/lib/mark-all-read-menu'
import { ROOM_WRITE_ROUTES } from '@/kessel/room-writes'
import { noteServerVersion, primaryScope, viewOnlyScope, type ServerScope } from '@/kessel/server-scope'

function row(over: Record<string, unknown>): Record<string, unknown> {
  return { kind: 'thread', id: 'c1', projectId: 'p1', seq: 3, lastReadSeq: 3, lastSeq: 5, unread: 2, waiting: 0, muted: false, ...over }
}

/** Before: p1 has 2 + 3 unread over two Threads (one muted) and a waiting
 *  card; p2 has 1. */
const BEFORE = {
  ok: true,
  total: 4,
  waitingTotal: 1,
  byConversation: [
    row({}),
    row({ id: 'c-old', unread: 3, muted: true, lastReadSeq: 1, seq: 1, lastSeq: 4 }),
    row({ id: 'c2', unread: 0, waiting: 1, lastReadSeq: 9, seq: 9, lastSeq: 10 }),
    row({ id: 'c3', projectId: 'p2', unread: 1, lastReadSeq: 0, seq: 0, lastSeq: 1 }),
  ],
  muted: [{ kind: 'thread', id: 'c-old' }],
}

/** The daemon after mark-all-read on p1: only the card and p2 remain. */
const AFTER = {
  ok: true,
  total: 2,
  waitingTotal: 1,
  byConversation: [BEFORE.byConversation[2], BEFORE.byConversation[3]],
  muted: [],
}

/** Another server's scope (a usable remote room by default). */
function scopeB(over: Partial<{ features: string[]; viewOnly: boolean }> = {}): ServerScope {
  const features = over.features ?? ['thread-read-v1', 'thread-read-all-v1']
  return {
    hostKey: 'host-b',
    connectionKey: 'host-b',
    isRemote: true,
    isPrimary: false,
    label: 'B',
    connectHost: () => null,
    isWindowHost: () => false,
    serverSupports: (f: string) => features.includes(f),
    viewOnly: over.viewOnly ?? false,
    remoteRoom: true,
  } as unknown as ServerScope
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

async function seed(): Promise<void> {
  cli.summary = BEFORE
  await useUnreadStore.getState().refresh()
  expect(useUnreadStore.getState().loaded).toBe(true)
  cli.calls = []
}

beforeEach(async () => {
  __resetUnreadForTests()
  noteServerVersion('local', '0.46.5', ['thread-read-v1', 'thread-read-all-v1'])
  cli.calls = []
  cli.postFails = false
  await seed()
})

describe('markAgentAllRead on the window’s own server', () => {
  it('drops the agent’s unread at once, keeps its waiting card and other agents, posts, then re-reads the truth', async () => {
    expect(threadSumForProject(useUnreadStore.getState(), 'p1')).toEqual({ unread: 5, waiting: 1, muted: false })
    cli.summary = AFTER
    const done = markAgentAllRead(primaryScope(), 'p1')
    // Optimistic, before the daemon answers.
    const s = useUnreadStore.getState()
    expect(threadSumForProject(s, 'p1')).toEqual({ unread: 0, waiting: 1, muted: false })
    expect(threadSumForProject(s, 'p2')).toEqual({ unread: 1, waiting: 0, muted: false })
    expect(s.total).toBe(2) // 4 − c1's 2 (c-old was muted: never in the total)
    expect(s.byKey['thread:c-old']).toMatchObject({ unread: 0, muted: true, lastReadSeq: 4 })
    await expect(done).resolves.toEqual({ marked: ['c1', 'c-old'], unread: 0, waiting: 1 })
    await flush()
    expect(cli.calls).toEqual([
      { method: 'POST', host: 'local', path: 'thread/read-all', body: { projectId: 'p1' } },
      { method: 'GET', host: 'local', path: 'thread/unread' },
    ])
    expect(threadSumForProject(useUnreadStore.getState(), 'p1')).toEqual({ unread: 0, waiting: 1, muted: false })
  })

  it('a refused POST throws and the daemon’s truth comes back (the count returns)', async () => {
    cli.postFails = true
    cli.summary = BEFORE
    await expect(markAgentAllRead(primaryScope(), 'p1')).rejects.toThrow('read-all refused')
    await flush()
    expect(cli.calls.map((c) => `${c.method} ${c.path}`)).toEqual(['POST thread/read-all', 'GET thread/unread'])
    expect(threadSumForProject(useUnreadStore.getState(), 'p1')).toEqual({ unread: 5, waiting: 1, muted: false })
  })
})

describe('markAgentAllRead on another server', () => {
  it('posts through that server’s scope and never touches this window’s store', async () => {
    const before = useUnreadStore.getState()
    await expect(markAgentAllRead(scopeB(), 'p-remote')).resolves.toMatchObject({ unread: 0 })
    expect(cli.calls).toEqual([{ method: 'POST', host: 'host-b', path: 'thread/read-all', body: { projectId: 'p-remote' } }])
    expect(useUnreadStore.getState().byKey).toEqual(before.byKey)
    expect(useUnreadStore.getState().total).toBe(before.total)
  })
})

describe('the menu row', () => {
  it('shows only with unread, on a server reporting the key, through a scope that may send it', () => {
    expect(markAllReadMenuItems(primaryScope(), 3).map((i) => i.id)).toEqual([MARK_ALL_READ_MENU_ID, 'separator-mark-all-read'])
    expect(markAllReadMenuItems(primaryScope(), 3, { separator: false })).toEqual([
      { id: MARK_ALL_READ_MENU_ID, label: 'Mark all as read' },
    ])
    expect(markAllReadMenuItems(primaryScope(), 0)).toEqual([])
    expect(markAllReadMenuItems(scopeB(), 1).map((i) => i.id)[0]).toBe(MARK_ALL_READ_MENU_ID)
    expect(markAllReadSupported(scopeB({ features: ['thread-read-v1'] }))).toBe(false)
    expect(markAllReadMenuItems(scopeB({ features: ['thread-read-v1'] }), 4)).toEqual([])
    expect(markAllReadSupported(viewOnlyScope(scopeB()))).toBe(false)
    noteServerVersion('local', '0.46.4', ['thread-read-v1'])
    expect(markAllReadMenuItems(primaryScope(), 3)).toEqual([])
  })

  it('a remote room may send thread/read-all', () => {
    expect(ROOM_WRITE_ROUTES.has('thread/read-all')).toBe(true)
  })
})
