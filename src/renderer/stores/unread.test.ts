// thread-read-v1 (0.46.1) — the unread store renders the daemon's truth:
//   - seeded from GET /cli/thread/unread (entries + totals);
//   - a `thread_read` / `project_read` frame replaces one entry and the
//     totals; unread 0 + waiting 0 + not muted removes it;
//   - markRead is optimistic, posts the right route and body, keeps one
//     request in flight per conversation and sends a later seq after;
//   - setMuted posts /cli/thread/mute and applies the reply;
//   - a host switch resets and refetches; hello refetches;
//   - a server without `thread-read-v1` is never asked.
//
// The daemon-cli layer, the session-events bus and the host store are
// mocked at the module boundary (the project-groups.test.ts idiom).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const ev = vi.hoisted(() => ({
  read: [] as Array<(e: Record<string, unknown>) => void>,
  hello: [] as Array<() => void>,
}))
vi.mock('@/stores/session-events', async () => {
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    onReadStateChanged: vi.fn((scope: unknown, fn: (e: Record<string, unknown>) => void) => {
      expectPrimaryScope(scope)
      ev.read.push(fn)
      return () => undefined
    }),
    onAppHello: vi.fn((scope: unknown, fn: () => void) => {
      expectPrimaryScope(scope)
      ev.hello.push(fn)
      return () => undefined
    }),
  }
})

const reconnect = vi.hoisted(() => ({ connected: [] as Array<() => void> }))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn((fn: () => void) => void reconnect.connected.push(fn)),
}))

const host = vi.hoisted(() => ({
  key: 'local',
  changes: [] as Array<() => void>,
}))
vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => host.key,
  onActiveHostChange: vi.fn((fn: () => void) => {
    host.changes.push(fn)
    return () => undefined
  }),
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string, q?: unknown) => cli.get(path, q)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
  }
})

import {
  __resetUnreadForTests,
  initUnreadEvents,
  parseUnreadEntry,
  selectProjectBadgeCount,
  threadSumForProject,
  unreadCountText,
  useUnreadStore,
} from '@/stores/unread'
import { noteServerVersion } from '@/kessel/server-scope'

function state(over: Record<string, unknown>): Record<string, unknown> {
  return {
    kind: 'thread',
    id: 'c1',
    projectId: 'p1',
    seq: 3,
    lastReadSeq: 3,
    lastSeq: 5,
    unread: 2,
    waiting: 0,
    muted: false,
    ...over,
  }
}

const SUMMARY = {
  ok: true,
  person: 'owner',
  total: 5,
  waitingTotal: 1,
  byConversation: [
    state({}),
    state({ kind: 'thread', id: 'c2', projectId: 'p1', unread: 0, waiting: 1, lastReadSeq: 9, seq: 9, lastSeq: 10 }),
    state({ kind: 'project', id: 'g1', projectId: undefined, unread: 2, lastSeq: 7, lastReadSeq: 5, seq: 5 }),
    state({ kind: 'project', id: 'g2', projectId: undefined, unread: 4, muted: true }),
  ],
  muted: [{ kind: 'project', id: 'g2' }],
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

function frame(e: Record<string, unknown>): void {
  if (ev.read.length === 0) throw new Error('no read-state handler registered')
  for (const fn of ev.read) fn(e)
}

beforeEach(async () => {
  __resetUnreadForTests()
  noteServerVersion('local', '0.46.1', ['thread-read-v1'])
  host.key = 'local'
  host.changes = []
  ev.read = []
  ev.hello = []
  reconnect.connected = []
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async (path: string) => {
    if (path !== 'thread/unread') throw new Error(`unexpected GET ${path}`)
    return SUMMARY
  })
  initUnreadEvents()
  await flush()
})

afterEach(() => {
  noteServerVersion('local', '0.46.1', ['thread-read-v1'])
})

describe('seeding from GET /cli/thread/unread', () => {
  it('holds every conversation keyed kind:id, with the totals', () => {
    const s = useUnreadStore.getState()
    expect(cli.get).toHaveBeenCalledWith('thread/unread', undefined)
    expect(s.loaded).toBe(true)
    expect(s.total).toBe(5)
    expect(s.waitingTotal).toBe(1)
    expect(Object.keys(s.byKey).sort()).toEqual(['project:g1', 'project:g2', 'thread:c1', 'thread:c2'])
    expect(s.byKey['thread:c1']).toEqual({
      kind: 'thread',
      id: 'c1',
      projectId: 'p1',
      lastReadSeq: 3,
      lastSeq: 5,
      unread: 2,
      waiting: 0,
      muted: false,
      mutedUntil: null,
    })
    expect(s.byKey['project:g2'].muted).toBe(true)
  })

  it('the Projects badge counts unmuted unread groups; a Thread sum is per workspace', () => {
    const s = useUnreadStore.getState()
    expect(selectProjectBadgeCount(s)).toBe(1)
    expect(threadSumForProject(s, 'p1')).toEqual({ unread: 2, waiting: 1, muted: false })
    expect(threadSumForProject(s, 'nope')).toBeNull()
  })

  it('a server without thread-read-v1 is never asked', async () => {
    __resetUnreadForTests()
    noteServerVersion('local', '0.46.0', [])
    cli.get.mockClear()
    initUnreadEvents()
    await flush()
    expect(cli.get).not.toHaveBeenCalled()
    await useUnreadStore.getState().markRead('thread', 'c1', 5)
    expect(cli.post).not.toHaveBeenCalled()
    expect(useUnreadStore.getState().loaded).toBe(false)
  })

  it('a body that is not a summary throws inside and keeps what is shown', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    cli.get.mockResolvedValueOnce({ ok: true })
    await useUnreadStore.getState().refresh()
    expect(warn).toHaveBeenCalledWith('[unread] summary fetch failed:', expect.any(Error))
    expect(useUnreadStore.getState().byKey['thread:c1'].unread).toBe(2)
    warn.mockRestore()
  })
})

describe('live frames', () => {
  it('a thread_read frame replaces the entry and the totals', () => {
    frame({ ...state({ unread: 7, lastSeq: 12 }), kind: 'thread_read', total: 10, waitingTotal: 1 })
    const s = useUnreadStore.getState()
    expect(s.byKey['thread:c1'].unread).toBe(7)
    expect(s.byKey['thread:c1'].lastSeq).toBe(12)
    expect(s.total).toBe(10)
  })

  it('a frame with nothing unread, nothing waiting and no mute removes the entry', () => {
    frame({
      ...state({ unread: 0, waiting: 0, lastReadSeq: 5, seq: 5 }),
      kind: 'thread_read',
      total: 3,
      waitingTotal: 1,
    })
    const s = useUnreadStore.getState()
    expect(s.byKey['thread:c1']).toBeUndefined()
    expect(s.total).toBe(3)
  })

  it('a project_read frame for a new group adds it (kind from the frame tag)', () => {
    frame({ kind: 'project_read', id: 'g9', seq: 0, lastReadSeq: 0, lastSeq: 1, unread: 1, waiting: 0, muted: false, total: 6, waitingTotal: 1 })
    const s = useUnreadStore.getState()
    expect(s.byKey['project:g9']).toMatchObject({ kind: 'project', id: 'g9', unread: 1, projectId: null })
    expect(selectProjectBadgeCount(s)).toBe(2)
  })
})

describe('markRead', () => {
  it('is optimistic, posts thread/read with the seq, then applies the reply', async () => {
    let answer: (v: unknown) => void = () => {}
    cli.post.mockImplementationOnce(() => new Promise((r) => (answer = r)))
    const done = useUnreadStore.getState().markRead('thread', 'c1', 5)
    // Optimistic: unread 0 now; the waiting-free entry is gone; total drops.
    expect(useUnreadStore.getState().byKey['thread:c1']).toBeUndefined()
    expect(useUnreadStore.getState().total).toBe(3)
    expect(cli.post).toHaveBeenCalledWith('thread/read', { conversationId: 'c1', seq: 5 })
    answer({ ok: true, changed: true, ...state({ unread: 0, lastReadSeq: 5, seq: 5 }), total: 3, waitingTotal: 1 })
    await done
    expect(useUnreadStore.getState().byKey['thread:c1']).toBeUndefined()
    expect(useUnreadStore.getState().total).toBe(3)
  })

  it('a project chat posts project-group/read {group, seq}', async () => {
    cli.post.mockResolvedValueOnce({
      ok: true,
      changed: true,
      ...state({ kind: 'project', id: 'g1', projectId: undefined, unread: 0, lastReadSeq: 7, seq: 7 }),
      total: 3,
      waitingTotal: 1,
    })
    await useUnreadStore.getState().markRead('project', 'g1', 7)
    expect(cli.post).toHaveBeenCalledWith('project-group/read', { group: 'g1', seq: 7 })
    expect(useUnreadStore.getState().byKey['project:g1']).toBeUndefined()
  })

  it('one request in flight per conversation; a later seq is sent once the first answers', async () => {
    const answers: Array<(v: unknown) => void> = []
    cli.post.mockImplementation(() => new Promise((r) => answers.push(r)))
    const first = useUnreadStore.getState().markRead('thread', 'c1', 4)
    void useUnreadStore.getState().markRead('thread', 'c1', 5)
    void useUnreadStore.getState().markRead('thread', 'c1', 5)
    expect(cli.post).toHaveBeenCalledTimes(1)
    answers[0]({ ok: true, changed: true, ...state({ unread: 1, lastReadSeq: 4, seq: 4 }), total: 4, waitingTotal: 1 })
    await first
    await flush()
    expect(cli.post).toHaveBeenCalledTimes(2)
    expect(cli.post).toHaveBeenLastCalledWith('thread/read', { conversationId: 'c1', seq: 5 })
    answers[1]({ ok: true, changed: true, ...state({ unread: 0, lastReadSeq: 5, seq: 5 }), total: 3, waitingTotal: 1 })
    await flush()
    expect(cli.post).toHaveBeenCalledTimes(2)
  })

  it('a waiting card survives reading (only an answer clears it)', async () => {
    cli.post.mockResolvedValueOnce({
      ok: true,
      changed: true,
      ...state({ id: 'c2', unread: 0, waiting: 1, lastReadSeq: 10, seq: 10, lastSeq: 10 }),
      total: 5,
      waitingTotal: 1,
    })
    await useUnreadStore.getState().markRead('thread', 'c2', 10)
    expect(useUnreadStore.getState().byKey['thread:c2']).toMatchObject({ waiting: 1, unread: 0, lastReadSeq: 10 })
  })

  it('a failed post refetches the summary', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    cli.post.mockRejectedValueOnce(new Error('boom'))
    cli.get.mockClear()
    await useUnreadStore.getState().markRead('thread', 'c1', 5)
    await flush()
    expect(cli.get).toHaveBeenCalledWith('thread/unread', undefined)
    // The refetch put the daemon's state back.
    expect(useUnreadStore.getState().byKey['thread:c1'].unread).toBe(2)
    warn.mockRestore()
  })
})

describe('setMuted', () => {
  it('posts thread/mute with kind + id and keeps the grey count out of the total', async () => {
    cli.post.mockResolvedValueOnce({
      ok: true,
      changed: true,
      ...state({ kind: 'project', id: 'g1', projectId: undefined, unread: 2, muted: true }),
      total: 3,
      waitingTotal: 1,
    })
    await useUnreadStore.getState().setMuted('project', 'g1', true)
    expect(cli.post).toHaveBeenCalledWith('thread/mute', { kind: 'project', id: 'g1', muted: true })
    const s = useUnreadStore.getState()
    expect(s.byKey['project:g1']).toMatchObject({ muted: true, unread: 2 })
    expect(s.total).toBe(3)
    expect(selectProjectBadgeCount(s)).toBe(0)
  })
})

describe('host and socket lifecycle', () => {
  it('a host switch resets and refetches against the new host', async () => {
    expect(host.changes).toHaveLength(1)
    host.key = 'remote-b'
    cli.get.mockReset()
    cli.get.mockResolvedValue({ ok: true, person: 'rosson', total: 0, waitingTotal: 0, byConversation: [], muted: [] })
    host.changes[0]()
    // Reset at once, before the new summary lands.
    expect(useUnreadStore.getState().byKey).toEqual({})
    expect(useUnreadStore.getState().loaded).toBe(false)
    await flush()
    expect(cli.get).toHaveBeenCalledTimes(1)
    expect(useUnreadStore.getState().loaded).toBe(true)
    expect(useUnreadStore.getState().total).toBe(0)
  })

  it('a slow summary from the previous host never lands after a switch', async () => {
    let answer: (v: unknown) => void = () => {}
    cli.get.mockImplementationOnce(() => new Promise((r) => (answer = r)))
    const pending = useUnreadStore.getState().refresh()
    host.key = 'remote-b'
    useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: false })
    answer(SUMMARY)
    await pending
    expect(useUnreadStore.getState().byKey).toEqual({})
  })

  it('hello and a daemon reconnect refetch (frames sent while down are lost)', async () => {
    cli.get.mockClear()
    expect(ev.hello).toHaveLength(1)
    expect(reconnect.connected).toHaveLength(1)
    ev.hello[0]()
    reconnect.connected[0]()
    await flush()
    expect(cli.get).toHaveBeenCalledTimes(2)
  })

  it('init is idempotent', () => {
    initUnreadEvents()
    initUnreadEvents()
    expect(ev.read).toHaveLength(1)
    expect(host.changes).toHaveLength(1)
  })
})

describe('pure helpers', () => {
  it('unreadCountText shows 99+ above 99 and nothing at 0', () => {
    expect(unreadCountText(0)).toBeNull()
    expect(unreadCountText(1)).toBe('1')
    expect(unreadCountText(99)).toBe('99')
    expect(unreadCountText(100)).toBe('99+')
  })

  it('parseUnreadEntry takes seq when lastReadSeq is absent and refuses a body without id', () => {
    expect(parseUnreadEntry({ kind: 'thread', id: 'x', seq: 4 })?.lastReadSeq).toBe(4)
    expect(parseUnreadEntry({ kind: 'thread' })).toBeNull()
    expect(parseUnreadEntry({ id: 'x' })).toBeNull()
    expect(parseUnreadEntry({ id: 'x' }, 'project')?.kind).toBe('project')
  })
})
