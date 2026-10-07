// @vitest-environment jsdom
//
// Thread sync (Rosson 2026-10-04): "when other systems (like a garden
// view) add messages to a thread, that message doesn't appear in the
// thread in other places." Every Thread surface — the Agents page Thread
// pane, a Home room, a Zen Garden conversation — is a `useOverlayThread`.
// These tests drive that hook against a fake daemon (its Thread + its
// overlay socket) and pin that every view converges on the daemon's
// Thread, merged by id, whoever wrote the message and whatever happened
// to the socket.
import type { ServerScope } from '@/kessel/server-scope'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(daemonCliGet),
    daemonCliPost: primaryOnly(daemonCliPost),
  }
})

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: async (scope: ServerScope) => {
    const { expectPrimaryScope } = await import('@/test-utils/scope')
    expectPrimaryScope(scope)
    return { host: '127.0.0.1', port: 1, token: 't', secure: false }
  },
  daemonWsBase: () => 'ws://test',
}))

import { ingestOverlayThreadItem, postThreadCompose, type OverlayThreadItem } from './overlayThread'
import { setOverlayReconnectBaseForTests, useOverlayThread } from './useOverlayThread'
import { primaryScope } from '@/kessel/server-scope'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'

// ── A fake daemon: one Thread, its seq counter, and the overlay sockets ──

interface Row {
  collection: 'thread'
  seq: number
  id: string
  doc: Record<string, unknown>
}

const server = {
  conv: 'conv-1',
  rows: [] as Row[],
  seq: 0,
  /** GETs with `since_seq` (catch-ups) the hook made. */
  catchUps: [] as number[],
  /** The live Thread turn `GET thread/activity` answers (S7, TW9). */
  turn: null as Record<string, unknown> | null,
  /** `GET thread/activity` calls, by addr. */
  turnAsks: [] as string[],
  /** When set, `GET thread/activity` waits for it (a slow answer). */
  turnGate: null as Promise<void> | null,
}

function write(id: string, from: string, body: string, via = 'thread'): Row {
  server.seq += 1
  const row: Row = { collection: 'thread', seq: server.seq, id, doc: { id, kind: 'text', from, body, via } }
  server.rows.push(row)
  return row
}

class FakeSocket {
  static all: FakeSocket[] = []
  url: string
  readyState = 0
  onmessage: ((ev: { data: string }) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onopen: ((ev: Event) => void) | null = null
  onclose: ((ev: Event) => void) | null = null
  closed = false
  constructor(url: string) {
    this.url = url
    FakeSocket.all.push(this)
  }
  open(): void {
    this.readyState = 1
    this.onopen?.(new Event('open'))
  }
  /** The daemon pushes a frame (what overlay_ws.rs sends). */
  frame(row: Row): void {
    this.onmessage?.({ data: JSON.stringify({ collection: row.collection, seq: row.seq, id: row.id, doc: row.doc }) })
  }
  /** The daemon pushes an ephemeral activity frame (§7.6). */
  activity(body: Record<string, unknown>): void {
    this.onmessage?.({ data: JSON.stringify({ collection: 'activity', seq: 0, id: body.turnId, activity: body }) })
  }
  /** The connection drops (daemon restart, network flap, edge idle cut). */
  drop(): void {
    this.readyState = 3
    this.onclose?.(new Event('close'))
  }
  close(): void {
    this.closed = true
    this.readyState = 3
  }
}

function live(): FakeSocket[] {
  return FakeSocket.all.filter((s) => !s.closed && s.readyState !== 3)
}

/** Push a row to every live socket (every window subscribed). */
function broadcast(row: Row): void {
  for (const s of live()) act(() => s.frame(row))
}

function ids(items: OverlayThreadItem[]): string[] {
  return items.map((it) => it.id)
}

beforeEach(() => {
  server.conv = 'conv-1'
  server.rows = []
  server.seq = 0
  server.catchUps = []
  server.turn = null
  server.turnAsks = []
  server.turnGate = null
  FakeSocket.all = []
  resetGridDialQueueForTests()
  vi.stubGlobal('WebSocket', FakeSocket)
  daemonCliGet.mockImplementation(async (route: string, p: Record<string, unknown>) => {
    if (route === 'thread/activity') {
      server.turnAsks.push(String(p.addr))
      const answer = server.turn
      if (server.turnGate) await server.turnGate
      return { ok: true, addr: p.addr, conversation_id: server.conv, turn: answer }
    }
    if (route !== 'thread') throw new Error(`unexpected GET ${route}`)
    if (p.since_seq !== undefined) {
      const since = Number(p.since_seq)
      server.catchUps.push(since)
      return { conversation_id: server.conv, has_more: false, items: server.rows.filter((r) => r.seq > since) }
    }
    const limit = Number(p.limit)
    return { conversation_id: server.conv, has_more: server.rows.length > limit, items: server.rows.slice(-limit) }
  })
  daemonCliPost.mockImplementation(async (route: string, body: Record<string, string>) => {
    if (route !== 'thread/post') throw new Error(`unexpected POST ${route}`)
    const row = write(`own-${server.seq + 1}`, 'rosson', body.text, 'compose')
    return { ok: true, id: row.id, seq: row.seq, from: 'rosson', body: body.text, kind: 'text', via: 'compose', conversation_id: server.conv }
  })
})

afterEach(() => {
  setOverlayReconnectBaseForTests(null)
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  vi.unstubAllGlobals()
})

function mountView(conversationId: string | null = null) {
  return renderHook(() => useOverlayThread({ scope: primaryScope(), addr: 'sales', conversationId, enabled: true }))
}

/** Mount, let the socket open, and wait for its gap catch-up. */
async function mountLive(conversationId: string | null = null) {
  const made = FakeSocket.all.length
  const view = mountView(conversationId)
  await waitFor(() => expect(FakeSocket.all.length).toBe(made + 1))
  const sock = FakeSocket.all[made]
  const before = server.catchUps.length
  act(() => sock.open())
  await waitFor(() => expect(server.catchUps.length).toBe(before + 1))
  return { view, sock }
}

describe('Thread sync: every view converges on the daemon’s Thread', () => {
  it('a message from another place (a Garden, the agent, another person) appears live, once', async () => {
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    expect(ids(view.result.current.items)).toEqual(['a1'])

    // Another surface posts; the daemon pushes the frame to this view.
    const fromGarden = write('g2', 'rosson', 'sent from the Garden', 'compose')
    broadcast(fromGarden)
    const fromAgent = write('a3', 'sales', 'on it')
    broadcast(fromAgent)
    const fromPat = write('p4', 'pat', 'me too', 'compose')
    broadcast(fromPat)
    // A duplicate delivery (a reconnect replay) never doubles a row.
    broadcast(fromGarden)

    expect(ids(view.result.current.items)).toEqual(['a1', 'g2', 'a3', 'p4'])
  })

  it('this window’s own send and its socket echo are one row', async () => {
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    let sent: OverlayThreadItem | null = null
    await act(async () => {
      const r = await postThreadCompose(primaryScope(), 'sales', 'ship it')
      if (!r.ok) throw new Error(r.error)
      sent = r.item
    })
    if (!sent) throw new Error('the post returned no row')
    expect(ids(view.result.current.items)).toEqual(['a1', (sent as OverlayThreadItem).id])
    broadcast(server.rows[server.rows.length - 1])
    expect(ids(view.result.current.items)).toEqual(['a1', (sent as OverlayThreadItem).id])
  })

  it('another sender’s message still lands when this window’s later send came back first', async () => {
    // Bug: the hook dropped any new frame whose seq was <= the newest seq
    // it had seen, and its own send bumped that mark. The agent's reply
    // (seq 2) racing the human's send (seq 3) vanished from this view.
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    const agentReply = write('a2', 'sales', 'reply in flight')
    await act(async () => {
      const r = await postThreadCompose(primaryScope(), 'sales', 'my next message')
      if (!r.ok) throw new Error(r.error)
    })
    broadcast(agentReply)
    expect(ids(view.result.current.items)).toEqual(['a1', 'a2', 'own-3'])
  })

  it('a message written between the snapshot and the socket opening is caught up', async () => {
    write('a1', 'sales', 'hello')
    const view = mountView('conv-1')
    await waitFor(() => expect(live().length).toBe(1))
    // Written after the snapshot, before this view subscribed: no frame
    // will ever come for it.
    write('g2', 'rosson', 'sent from the Garden', 'compose')
    act(() => live()[0].open())
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['a1', 'g2']))
    expect(server.catchUps).toEqual([1])
  })

  it('a dropped socket reconnects and catches up what it missed, including a card answered elsewhere', async () => {
    setOverlayReconnectBaseForTests(5)
    server.rows.push({
      collection: 'thread',
      seq: 1,
      id: 'card',
      doc: { id: 'card', kind: 'choice', from: 'sales', choice: { prompt: 'Ship?', options: [{ label: 'Ship' }], allow_custom: false, status: 'pending' } },
    })
    server.seq = 1
    const { view, sock } = await mountLive('conv-1')
    expect(view.result.current.items[0].doc.choice?.status).toBe('pending')

    act(() => sock.drop())
    // While this view is offline: a Garden message, and the card answered
    // in another window.
    write('g2', 'rosson', 'sent while you were away', 'compose')
    const card = server.rows[0].doc as { choice: { status: string; answer?: string } }
    card.choice.status = 'answered'
    card.choice.answer = 'Ship'

    await waitFor(() => expect(live().length).toBe(1))
    expect(live()[0]).not.toBe(sock)
    act(() => live()[0].open())
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['card', 'g2']))
    expect(view.result.current.items[0].doc.choice?.status).toBe('answered')
    // The re-sync covered the loaded window (oldest − 1), not only newer rows.
    expect(server.catchUps[server.catchUps.length - 1]).toBe(0)
  })

  it('a window coming back to the front reconnects at once when its socket is gone', async () => {
    setOverlayReconnectBaseForTests(60_000)
    write('a1', 'sales', 'hello')
    const { view, sock } = await mountLive('conv-1')
    act(() => sock.drop())
    write('g2', 'rosson', 'from the Garden', 'compose')
    // The backoff would wait a minute; focus does not.
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await waitFor(() => expect(live().length).toBe(1))
    act(() => live()[0].open())
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['a1', 'g2']))
  })

  it('when the address now points at another conversation (pinned Chat changed), the view moves to it', async () => {
    setOverlayReconnectBaseForTests(5)
    write('a1', 'sales', 'old chat')
    const { view, sock } = await mountLive(null)
    expect(view.result.current.conversationId).toBe('conv-1')
    // New pinned Chat: the address resolves elsewhere; the Garden posts there.
    server.conv = 'conv-2'
    server.rows = []
    write('n1', 'rosson', 'to the new chat', 'compose')
    act(() => sock.drop())
    await waitFor(() => expect(live().length).toBe(1))
    act(() => live()[0].open())
    await waitFor(() => expect(view.result.current.conversationId).toBe('conv-2'))
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['n1']))
    await waitFor(() => expect(live().some((s) => s.url.includes('conversation=conv-2'))).toBe(true))
    expect(FakeSocket.all.filter((s) => s.url.includes('conversation=conv-1')).every((s) => s.readyState === 3)).toBe(true)
  })

  it('two views of one Thread in one window (the Agents page and a Garden) stay identical', async () => {
    write('a1', 'sales', 'hello')
    const pane = await mountLive('conv-1')
    const garden = await mountLive(null)
    // The Garden sends: the pane shows it at once (in-window), then both
    // sockets echo it.
    await act(async () => {
      const r = await postThreadCompose(primaryScope(), 'sales', 'from the Garden')
      if (!r.ok) throw new Error(r.error)
    })
    broadcast(server.rows[server.rows.length - 1])
    // Someone else writes (another window, another user, an app guest).
    broadcast(write('x3', 'guest', 'from the app'))
    // The agent answers through `k2 thread` — no in-window ingest at all.
    broadcast(write('a4', 'sales', 'done'))
    const expected = ['a1', 'own-2', 'x3', 'a4']
    expect(ids(pane.view.result.current.items)).toEqual(expected)
    expect(ids(garden.view.result.current.items)).toEqual(expected)
  })

  it('an in-window send for another conversation never lands here', async () => {
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    act(() => {
      ingestOverlayThreadItem({
        collection: 'thread',
        seq: 9,
        id: 'elsewhere',
        conversation_id: 'conv-other',
        doc: { id: 'elsewhere', kind: 'text', from: 'rosson', body: 'other agent', via: 'compose' },
      })
    })
    expect(ids(view.result.current.items)).toEqual(['a1'])
  })
})

// prd-daemon-activity-and-thread-working-v1 S7 (TW9, TW11, T-S7b, T-S7c):
// the working strip's turn rides the same socket and never touches items.
describe('Thread sync: the working strip turn (S7)', () => {
  function body(over: Record<string, unknown> = {}): Record<string, unknown> {
    const now = Date.now()
    return {
      turnId: 'turn-1',
      state: 'working',
      phase: 'tool',
      phaseSince: now - 5_000,
      line: 'Running `cargo test`',
      startedAt: now - 60_000,
      subagents: 0,
      subagentsDone: 0,
      background: 0,
      tally: { read: 0, search: 0, cmd: 1, edit: 0 },
      end: null,
      serverNow: now,
      rev: 2,
      ...over,
    }
  }

  it('every socket open reads the live turn (since_seq never replays activity)', async () => {
    setOverlayReconnectBaseForTests(5)
    write('a1', 'rosson', 'run the tests', 'compose')
    server.turn = body()
    const { view, sock } = await mountLive('conv-1')
    await waitFor(() => expect(view.result.current.turn?.turnId).toBe('turn-1'))
    expect(server.turnAsks).toEqual(['sales'])
    expect(view.result.current.turnsReported).toBe(true)

    // Reconnect mid-turn: the turn moved on while the socket was down.
    act(() => sock.drop())
    server.turn = body({ phase: 'thinking', line: null, rev: 5 })
    await waitFor(() => expect(live().length).toBe(1))
    act(() => live()[0].open())
    await waitFor(() => expect(view.result.current.turn?.phase).toBe('thinking'))
    expect(server.turnAsks).toEqual(['sales', 'sales'])
  })

  it('activity frames move the turn and never the items (T-S7c); an end ends it', async () => {
    write('a1', 'rosson', 'run the tests', 'compose')
    const { view, sock } = await mountLive('conv-1')
    await waitFor(() => expect(server.turnAsks).toHaveLength(1))
    const items = view.result.current.items
    act(() => sock.activity(body({ rev: 1, phase: 'delivering', line: null })))
    expect(view.result.current.turn?.phase).toBe('delivering')
    act(() => sock.activity(body({ rev: 2 })))
    expect(view.result.current.turn?.line).toBe('Running `cargo test`')
    expect(view.result.current.items).toBe(items)

    const reply = write('a2', 'sales', 'all green')
    broadcast(reply)
    act(() => sock.activity(body({ rev: 3, state: 'idle', end: { reason: 'reply', detail: null, at: Date.now() } })))
    expect(view.result.current.turn?.end?.reason).toBe('reply')
    expect(ids(view.result.current.items)).toEqual(['a1', 'a2'])
  })

  it('a catch-up answer older than a frame that arrived meanwhile is dropped', async () => {
    write('a1', 'rosson', 'run the tests', 'compose')
    let release!: () => void
    server.turnGate = new Promise((r) => {
      release = r
    })
    server.turn = null
    const { view, sock } = await mountLive('conv-1')
    await waitFor(() => expect(server.turnAsks).toHaveLength(1))
    // The turn starts while the GET (answered "no turn") is in flight.
    act(() => sock.activity(body({ rev: 1, phase: 'delivering', line: null })))
    await act(async () => {
      release()
      await Promise.resolve()
    })
    await waitFor(() => expect(view.result.current.turn?.phase).toBe('delivering'))
  })

  it('T-S7b: a server without daemon-activity is never asked, and the turn stays null', async () => {
    const supports = vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => f !== 'daemon-activity')
    const warn = vi.spyOn(console, 'warn')
    const error = vi.spyOn(console, 'error')
    try {
      write('a1', 'sales', 'hello')
      const { view } = await mountLive('conv-1')
      expect(ids(view.result.current.items)).toEqual(['a1'])
      expect(server.turnAsks).toEqual([])
      expect(view.result.current.turn).toBeNull()
      expect(view.result.current.turnsReported).toBe(false)
      expect(warn).not.toHaveBeenCalled()
      expect(error).not.toHaveBeenCalled()
    } finally {
      supports.mockRestore()
      warn.mockRestore()
      error.mockRestore()
    }
  })
})
