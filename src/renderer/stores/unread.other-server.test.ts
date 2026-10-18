// @vitest-environment jsdom
// thread-read-v1 on ANOTHER server (0.46.1 polish item 11): a Home remote
// room's Thread and a Zen conversation whose agent lives on another server
// are marked read on THAT server, for this window's login there:
//   - `thread/read {conversationId, seq}` goes through that server's scope,
//     never the window's, and the window's unread store is untouched;
//   - the same or a lower seq never posts twice; a newer one posts again;
//   - a view-only room, or a server without `thread-read-v1`, is never asked;
//   - a failed post is forgotten, so the next look tries again;
//   - useAutoMarkRead with such a scope posts only while visible and the
//     window is focused, and a built-in Zen conversation on another server
//     reads through the same path while shown and ready;
//   - the room write allowlist carries `thread/read` (a remote room may send it).

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/stores/session-events', () => ({
  onReadStateChanged: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
}))

const cli = vi.hoisted(() => ({
  calls: [] as Array<{ host: string; path: string; body: unknown }>,
  fail: false,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => {
    throw new Error('no GET expected')
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, path: string, body: { conversationId: string; seq: number }) => {
    cli.calls.push({ host: scope.hostKey, path, body })
    if (cli.fail) throw new Error('B is down')
    return {
      ok: true,
      changed: true,
      kind: 'thread',
      id: body.conversationId,
      seq: body.seq,
      lastReadSeq: body.seq,
      lastSeq: body.seq,
      unread: 0,
      waiting: 0,
      muted: false,
      total: 0,
      waitingTotal: 0,
    }
  }),
}))

import { act } from 'react'
import { renderHook } from '@testing-library/react'
import { __resetUnreadForTests, markReadOnServer, useAutoMarkRead, useUnreadStore } from '@/stores/unread'
import { ROOM_WRITE_ROUTES } from '@/kessel/room-writes'
import { useZenThreadAutoRead } from '@/components/Zen/widgets/ZenThreadReadControls'
import type { ZenThreadView } from '@/lib/zen/zen-data'
import type { ServerScope } from '@/kessel/server-scope'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

/** Another server's scope: a usable remote room unless told otherwise. */
function scopeB(over: Partial<{ features: string[]; viewOnly: boolean; remoteRoom: boolean }> = {}): ServerScope {
  const features = over.features ?? ['thread-read-v1']
  return {
    hostKey: 'host-b',
    connectionKey: 'host-b',
    isRemote: true,
    label: 'B',
    connectHost: () => null,
    isWindowHost: () => false,
    creds: async () => {
      throw new Error('no creds in this test')
    },
    httpBase: async () => 'http://b.test',
    wsBase: async () => 'ws://b.test',
    serverSupports: (f: string) => features.includes(f),
    viewOnly: over.viewOnly ?? false,
    remoteRoom: over.remoteRoom ?? true,
  } as unknown as ServerScope
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

let focused = true

beforeEach(() => {
  __resetUnreadForTests()
  cli.calls = []
  cli.fail = false
  focused = true
  vi.spyOn(document, 'hasFocus').mockImplementation(() => focused)
})

describe('markReadOnServer', () => {
  it('posts thread/read to that server; the same or a lower seq never posts again; a newer one does', async () => {
    const b = scopeB()
    await markReadOnServer(b, 'thread', 'conv-b', 7)
    expect(cli.calls).toEqual([{ host: 'host-b', path: 'thread/read', body: { conversationId: 'conv-b', seq: 7 } }])
    await markReadOnServer(b, 'thread', 'conv-b', 7)
    await markReadOnServer(b, 'thread', 'conv-b', 4)
    expect(cli.calls).toHaveLength(1)
    await markReadOnServer(b, 'thread', 'conv-b', 9)
    expect(cli.calls.map((c) => c.body)).toEqual([
      { conversationId: 'conv-b', seq: 7 },
      { conversationId: 'conv-b', seq: 9 },
    ])
    // The window's own store never learns another server's Threads.
    expect(useUnreadStore.getState().byKey).toEqual({})
  })

  it('a view-only room and a server without thread-read-v1 are never asked', async () => {
    await markReadOnServer(scopeB({ viewOnly: true, remoteRoom: false }), 'thread', 'conv-b', 3)
    await markReadOnServer(scopeB({ features: [] }), 'thread', 'conv-b', 3)
    expect(cli.calls).toEqual([])
  })

  it('a failed post is forgotten: the next look asks again', async () => {
    const b = scopeB()
    cli.fail = true
    await markReadOnServer(b, 'thread', 'conv-b', 5)
    cli.fail = false
    await markReadOnServer(b, 'thread', 'conv-b', 5)
    expect(cli.calls.map((c) => c.body)).toEqual([
      { conversationId: 'conv-b', seq: 5 },
      { conversationId: 'conv-b', seq: 5 },
    ])
  })

  it('only Threads, and only a real seq', async () => {
    await markReadOnServer(scopeB(), 'project', 'g1', 5)
    await markReadOnServer(scopeB(), 'thread', 'conv-b', 0)
    expect(cli.calls).toEqual([])
  })
})

describe('useAutoMarkRead on another server', () => {
  it('visible + focused posts to the room’s server; not focused posts nothing until focus', async () => {
    const b = scopeB()
    focused = false
    const { rerender } = renderHook((p: { seq: number; visible: boolean }) =>
      useAutoMarkRead({ kind: 'thread', id: 'conv-b', newestSeq: p.seq, visible: p.visible, scope: b }),
    { initialProps: { seq: 4, visible: true } })
    await flush()
    expect(cli.calls).toEqual([])
    focused = true
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await flush()
    expect(cli.calls).toEqual([{ host: 'host-b', path: 'thread/read', body: { conversationId: 'conv-b', seq: 4 } }])
    // Hidden: a newer message is not read.
    rerender({ seq: 6, visible: false })
    await flush()
    expect(cli.calls).toHaveLength(1)
    rerender({ seq: 6, visible: true })
    await flush()
    expect(cli.calls.map((c) => c.body)).toEqual([
      { conversationId: 'conv-b', seq: 4 },
      { conversationId: 'conv-b', seq: 6 },
    ])
  })
})

describe('a Zen conversation on another server', () => {
  function view(over: Partial<ZenThreadView>): ZenThreadView {
    return {
      address: 'k2::b.test/ops',
      phase: 'ready',
      note: null,
      items: [
        { seq: 3, id: 'i3', doc: { id: 'i3', kind: 'text', from: 'ops', to: 'owner', created_at: 1, body: 'hi' } },
        { seq: 8, id: 'i8', doc: { id: 'i8', kind: 'text', from: 'ops', to: 'owner', created_at: 2, body: 'news' } },
      ],
      loaded: true,
      hasMore: false,
      loadingOlder: false,
      error: null,
      conversationId: 'conv-b',
      windowServer: false,
      scope: scopeB(),
      ...over,
    } as unknown as ZenThreadView
  }

  it('reads its Thread on that server while shown; hidden or not ready posts nothing', async () => {
    const { rerender } = renderHook((p: { v: ZenThreadView; shown: boolean }) => useZenThreadAutoRead(p.v, p.shown), {
      initialProps: { v: view({ phase: 'opening' as ZenThreadView['phase'] }), shown: true },
    })
    await flush()
    expect(cli.calls).toEqual([])
    rerender({ v: view({}), shown: false })
    await flush()
    expect(cli.calls).toEqual([])
    rerender({ v: view({}), shown: true })
    await flush()
    expect(cli.calls).toEqual([{ host: 'host-b', path: 'thread/read', body: { conversationId: 'conv-b', seq: 8 } }])
  })
})

describe('the room write allowlist', () => {
  it('lets a remote room send thread/read to its own server', () => {
    expect(ROOM_WRITE_ROUTES.has('thread/read')).toBe(true)
  })
})
