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
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'

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
import { listThreadViews, resetThreadViewCacheForTests } from './threadViewCache'

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
  /** GETs with `since_seq` (full catch-ups) the hook made. */
  catchUps: [] as number[],
  /** GETs with `after_seq` (delta catch-ups, thread-delta-v1) the hook made. */
  deltas: [] as number[],
  /** In-place rewrites (a card answered, a message deleted): the doc id
   *  and the head seq when it changed (the daemon's `thread_changes`). */
  changes: [] as Array<{ id: string; mark: number }>,
  /** Rows that left this Thread (a fork split), with their seq and mark. */
  removedRows: [] as Array<{ id: string; seq: number; mark: number }>,
  /** The most rows one delta answer holds (the route's `limit`). */
  deltaLimit: null as number | null,
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

/** Rewrite a row's doc in place, as the daemon's `put_card_doc` does (the
 *  change is marked with the head seq now). */
function rewrite(id: string, patch: (doc: Record<string, unknown>) => Record<string, unknown>): Row {
  const row = server.rows.find((r) => r.id === id)
  if (!row) throw new Error(`no row ${id}`)
  row.doc = patch(row.doc)
  server.changes.push({ id, mark: server.seq })
  return row
}

/** Every sync GET (full or delta) the hook made. */
function syncs(): number {
  return server.catchUps.length + server.deltas.length
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
  /** The daemon moved this Thread to another key (Codex/Hermes adoption,
   *  overlay_ws.rs `publish_moved`). */
  moved(from: string, to: string, seq: number): void {
    this.onmessage?.({ data: JSON.stringify({ collection: 'moved', seq, id: to, from, to }) })
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

beforeEach(async () => {
  // Every test starts with no Thread copy in this window (the view cache
  // holds local Threads too).
  await resetThreadViewCacheForTests()
  server.conv = 'conv-1'
  server.rows = []
  server.seq = 0
  server.catchUps = []
  server.deltas = []
  server.changes = []
  server.removedRows = []
  server.deltaLimit = null
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
    if (p.after_seq !== undefined) {
      // thread-delta-v1 (store.rs `read_thread_delta`): rows after N, plus
      // rows at or below N changed in place since (marked at or after N),
      // plus rows that left.
      if (p.since_seq !== undefined || p.before_seq !== undefined) throw new Error('after_seq combined with since/before')
      const after = Number(p.after_seq)
      server.deltas.push(after)
      const newer = server.rows.filter((r) => r.seq > after)
      const limit = server.deltaLimit ?? Number(p.limit)
      const page = limit > 0 ? newer.slice(0, limit) : newer
      const changedIds = new Set(server.changes.filter((c) => c.mark >= after).map((c) => c.id))
      return {
        ok: true,
        collection: 'thread',
        conversation_id: server.conv,
        after_seq: after,
        has_more: page.length < newer.length,
        items: page,
        changed: server.rows.filter((r) => r.seq <= after && changedIds.has(r.id)),
        removed: server.removedRows.filter((r) => r.mark >= after && r.seq <= after).map((r) => ({ id: r.id, seq: r.seq })),
      }
    }
    if (p.since_seq !== undefined) {
      const since = Number(p.since_seq)
      server.catchUps.push(since)
      return { conversation_id: server.conv, has_more: false, items: server.rows.filter((r) => r.seq > since) }
    }
    const limit = Number(p.limit)
    if (p.before_seq !== undefined) {
      const older = server.rows.filter((r) => r.seq < Number(p.before_seq))
      return { conversation_id: server.conv, has_more: older.length > limit, items: older.slice(-limit) }
    }
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
  const before = syncs()
  act(() => sock.open())
  await waitFor(() => expect(syncs()).toBe(before + 1))
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
    // The delta since the snapshot's newest row (it also carries a card
    // changed between the snapshot and the subscribe).
    expect(server.deltas).toEqual([1])
    expect(server.catchUps).toEqual([])
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
    rewrite('card', (doc) => ({
      ...doc,
      choice: { ...(doc.choice as Record<string, unknown>), status: 'answered', answer: 'Ship' },
    }))

    await waitFor(() => expect(live().length).toBe(1))
    expect(live()[0]).not.toBe(sock)
    act(() => live()[0].open())
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['card', 'g2']))
    expect(view.result.current.items[0].doc.choice?.status).toBe('answered')
    // The re-sync asked for the delta since the last read (seq 1): the new
    // row and the card's change, not the whole loaded window.
    expect(server.deltas[server.deltas.length - 1]).toBe(1)
    expect(server.catchUps).toEqual([])
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

// Rosson 2026-10-07: "if you send a message in the Thread for Codex, the
// response from Codex doesn't land in the Thread. You have to refresh, and
// then the first message you sent disappears". Codex mints its own id, so
// its Thread starts under the pane key; on adoption the daemon moves the
// Thread (every row, renumbered) to Codex's id and sends `moved` on both
// keys' sockets. The view follows in place, with no refresh.
describe('Thread sync: a Thread that moves on adoption (Codex/Hermes)', () => {
  it('follows the move: keeps the first message, re-subscribes on the new key, shows the reply', async () => {
    server.conv = 'pane-1'
    write('u1', 'rosson', 'hello codex', 'compose')
    const { view, sock } = await mountLive(null)
    expect(view.result.current.conversationId).toBe('pane-1')
    expect(ids(view.result.current.items)).toEqual(['u1'])

    // Adoption: the daemon moved u1 under codex-1; the agent's reply raced
    // in right after (seq 2 there).
    server.conv = 'codex-1'
    write('a2', 'sales', 'hi from codex')
    const catchUpsBefore = server.catchUps.length
    act(() => sock.moved('pane-1', 'codex-1', 1))

    // Nothing on screen went away while the socket moves.
    expect(ids(view.result.current.items)).toEqual(['u1'])
    expect(view.result.current.loaded).toBe(true)
    expect(view.result.current.conversationId).toBe('codex-1')
    expect(sock.onmessage).toBeNull()
    await waitFor(() => expect(live().some((s) => s.url.includes('conversation=codex-1'))).toBe(true))
    const next = live().find((s) => s.url.includes('conversation=codex-1'))
    if (!next) throw new Error('no socket on the new key')
    act(() => next.open())

    // The whole Thread is re-read (renumbered rows) and merged by id.
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['u1', 'a2']))
    expect(server.catchUps.slice(catchUpsBefore)).toEqual([0])
    expect(view.result.current.items.map((it) => it.seq)).toEqual([1, 2])
    expect(sock.readyState).toBe(3)

    // Later frames arrive on the new socket; the old one is out of the loop.
    act(() => sock.frame(write('stale', 'sales', 'never shown')))
    act(() => next.frame(write('a3', 'sales', 'done')))
    expect(ids(view.result.current.items)).toEqual(['u1', 'a2', 'a3'])
    // The in-window compose ingest now matches the new key too.
    act(() => {
      ingestOverlayThreadItem({
        collection: 'thread',
        seq: 9,
        id: 'own-9',
        conversation_id: 'codex-1',
        doc: { id: 'own-9', kind: 'text', from: 'rosson', body: 'thanks', via: 'compose' },
      })
    })
    expect(ids(view.result.current.items)).toEqual(['u1', 'a2', 'a3', 'own-9'])
  })

  it('a view already on the new key re-reads the renumbered Thread on the same socket', async () => {
    server.conv = 'codex-1'
    write('a1', 'sales', 'early reply')
    const { view, sock } = await mountLive('codex-1')
    expect(ids(view.result.current.items)).toEqual(['a1'])

    // The moved first message sorts before the reply; seqs were renumbered.
    server.rows = [
      { collection: 'thread', seq: 1, id: 'u0', doc: { id: 'u0', kind: 'text', from: 'rosson', body: 'hello codex', via: 'compose' } },
      { collection: 'thread', seq: 2, id: 'a1', doc: { id: 'a1', kind: 'text', from: 'sales', body: 'early reply', via: 'thread' } },
    ]
    server.seq = 2
    const sockets = FakeSocket.all.length
    act(() => sock.moved('pane-1', 'codex-1', 2))
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['u0', 'a1']))
    expect(view.result.current.items.map((it) => it.seq)).toEqual([1, 2])
    expect(FakeSocket.all.length).toBe(sockets)
    expect(live()).toContain(sock)
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

  // 0.46.5 (review B1): the daemon dropped frames on a lagging socket,
  // the end among them, and the bar stuck on "working". Now it sends an
  // `activity_reset` with the turn as it is now (or null).
  it('an activity_reset replaces a stuck turn: null clears it, a body replaces it', async () => {
    write('a1', 'rosson', 'run the tests', 'compose')
    const { view, sock } = await mountLive('conv-1')
    await waitFor(() => expect(server.turnAsks).toHaveLength(1))
    act(() => sock.activity(body({ rev: 4, state: 'monitoring', phase: 'children', line: null })))
    expect(view.result.current.turn?.state).toBe('monitoring')
    // The end frame was lost; the reset says none is running.
    act(() => sock.onmessage?.({ data: JSON.stringify({ collection: 'activity_reset', seq: 0, id: null, activity: null }) }))
    expect(view.result.current.turn).toBeNull()
    // A reset with a live turn puts it back as it is now.
    const now = body({ turnId: 'turn-2', rev: 1, phase: 'thinking', line: null })
    act(() => sock.onmessage?.({ data: JSON.stringify({ collection: 'activity_reset', seq: 0, id: 'turn-2', activity: now }) }))
    expect([view.result.current.turn?.turnId, view.result.current.turn?.phase]).toEqual(['turn-2', 'thinking'])
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

describe('Thread sync: a renamed chat keeps its Thread (prd-thread-survives-tab-rename S4)', () => {
  /** A view whose `addr` prop the test changes, like a rename does. */
  async function mountRenamable(addr: string) {
    const made = FakeSocket.all.length
    const loadedSeen: boolean[] = []
    const view = renderHook(
      ({ a }: { a: string }) => {
        const r = useOverlayThread({ scope: primaryScope(), addr: a, conversationId: 'conv-1', enabled: true })
        loadedSeen.push(r.loaded)
        return r
      },
      { initialProps: { a: addr } },
    )
    await waitFor(() => expect(FakeSocket.all.length).toBe(made + 1))
    const sock = FakeSocket.all[made]
    const before = syncs()
    act(() => sock.open())
    await waitFor(() => expect(syncs()).toBe(before + 1))
    return { view, sock, loadedSeen }
  }

  it('test 17: a new address for the same conversation keeps items and socket, no loaded flash', async () => {
    write('a1', 'sales/1', 'before the rename')
    const { view, sock, loadedSeen } = await mountRenamable('sales/1')
    expect(ids(view.result.current.items)).toEqual(['a1'])
    const sockets = FakeSocket.all.length
    const syncsBefore = syncs()
    const flashFrom = loadedSeen.length

    view.rerender({ a: 'sales/reviewer' })
    // The swap re-reads on the NEW address, on the same socket.
    await waitFor(() => expect(syncs()).toBe(syncsBefore + 1))
    const lastGet = daemonCliGet.mock.calls.filter((c) => c[0] === 'thread').pop()
    expect((lastGet?.[1] as Record<string, unknown>).addr).toBe('sales/reviewer')
    expect(FakeSocket.all.length).toBe(sockets)
    expect(sock.closed).toBe(false)
    expect(ids(view.result.current.items)).toEqual(['a1'])
    expect(loadedSeen.slice(flashFrom).every(Boolean)).toBe(true)

    // A reply still lands live on the same socket.
    broadcast(write('a2', 'sales/reviewer', 'after the rename'))
    expect(ids(view.result.current.items)).toEqual(['a1', 'a2'])
  })

  it('an address frame names the new address and tells the views holding the old one', async () => {
    const { subscribeThreadAddress } = await import('@/lib/thread-address-bus')
    const seen: Array<{ address: string; previous: string }> = []
    const off = subscribeThreadAddress((c) => seen.push({ address: c.address, previous: c.previous }))
    try {
      write('a1', 'sales/1', 'hello')
      const { view, sock } = await mountRenamable('sales/1')
      act(() => {
        sock.onmessage?.({
          data: JSON.stringify({ collection: 'address', seq: 0, id: 'conv-1', address: 'sales/reviewer', previous: 'sales/1' }),
        })
      })
      expect(seen).toEqual([{ address: 'sales/reviewer', previous: 'sales/1' }])
      expect(view.result.current.currentAddr).toBe('sales/reviewer')
      expect(view.result.current.pastAddresses).toContain('sales/1')
      expect(ids(view.result.current.items)).toEqual(['a1'])
    } finally {
      off()
    }
  })

  it('test 16: a Thread post that 404s asks for one re-lookup and shows why', async () => {
    const { subscribeThreadAddress } = await import('@/lib/thread-address-bus')
    const seen: string[] = []
    const off = subscribeThreadAddress((c) => seen.push(c.previous))
    try {
      daemonCliPost.mockImplementationOnce(async () => {
        throw new Error(JSON.stringify({ ok: false, error: { code: 'not_found', hint: "unknown overlay addr 'sales/1'" } }))
      })
      const r = await postThreadCompose(primaryScope(), 'sales/1', 'and the lint')
      expect(r.ok).toBe(false)
      if (r.ok) throw new Error('unreachable')
      expect(r.error).toMatch(/address changed/)
      expect(seen).toEqual(['sales/1'])
    } finally {
      off()
    }
  })
})

// Speed pass P3: a focus or reconnect re-sync asks only for what changed
// since this view's last server read (`after_seq`, thread-delta-v1), not
// the whole loaded window again (`since_seq` = oldest − 1, `limit=0`).
describe('Thread sync: re-sync by delta (speed pass P3)', () => {
  const realNow = Date.now.bind(Date)
  let skew = 0
  beforeEach(() => {
    // Views the earlier tests left mounted would answer this block's focus
    // events too, and their reads would land on this fake daemon.
    cleanup()
    skew = 0
    vi.spyOn(Date, 'now').mockImplementation(() => realNow() + skew)
  })
  afterEach(() => {
    cleanup()
    vi.mocked(Date.now).mockRestore()
  })

  /** Window focus past the re-sync throttle (10 s). */
  function focusLater(): void {
    skew += 11_000
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
  }

  /** The last `thread` GET's params and the answer it got. */
  async function lastThreadRead(): Promise<{ params: Record<string, unknown>; answer: Record<string, unknown> }> {
    const calls = daemonCliGet.mock.calls
    const results = daemonCliGet.mock.results
    for (let i = calls.length - 1; i >= 0; i--) {
      if (calls[i][0] !== 'thread') continue
      return { params: calls[i][1] as Record<string, unknown>, answer: (await results[i].value) as Record<string, unknown> }
    }
    throw new Error('no thread read')
  }

  function card(id: string): Row {
    server.seq += 1
    const row: Row = {
      collection: 'thread',
      seq: server.seq,
      id,
      doc: { id, kind: 'choice', from: 'sales', choice: { prompt: 'Ship?', options: [{ label: 'Ship' }], allow_custom: false, status: 'pending' } },
    }
    server.rows.push(row)
    return row
  }

  it('with 300 messages loaded and one new, a focus re-sync asks after_seq and gets one message', async () => {
    for (let i = 1; i <= 300; i++) write(`m${i}`, 'sales', `message ${i}`)
    const { view } = await mountLive('conv-1')
    await act(async () => {
      await view.result.current.loadAll()
    })
    expect(view.result.current.items).toHaveLength(300)
    // A new message whose frame never came (a half-open socket after sleep).
    write('m301', 'rosson', 'while asleep', 'compose')
    const before = server.deltas.length
    focusLater()
    await waitFor(() => expect(view.result.current.items).toHaveLength(301))
    expect(server.deltas.slice(before)).toEqual([300])
    const { params, answer } = await lastThreadRead()
    expect(params.after_seq).toBe(300)
    expect(params.since_seq).toBeUndefined()
    expect(params.limit).not.toBe(0)
    expect(answer.items).toHaveLength(1)
    expect(server.catchUps).toEqual([])
    const all = ids(view.result.current.items)
    expect(new Set(all).size).toBe(301)
    expect(all[300]).toBe('m301')
  })

  it('a card answered elsewhere arrives through the delta on focus, with no new rows', async () => {
    card('c1')
    write('a2', 'sales', 'after the card')
    const { view } = await mountLive('conv-1')
    expect(view.result.current.items[0].doc.choice?.status).toBe('pending')
    rewrite('c1', (doc) => ({ ...doc, choice: { ...(doc.choice as Record<string, unknown>), status: 'answered', answer: 'Ship' } }))
    focusLater()
    await waitFor(() => expect(view.result.current.items[0].doc.choice?.status).toBe('answered'))
    expect(view.result.current.items[0].doc.choice?.answer).toBe('Ship')
    expect(ids(view.result.current.items)).toEqual(['c1', 'a2'])
    const { answer } = await lastThreadRead()
    expect(answer.items).toEqual([])
    expect((answer.changed as unknown[]).length).toBe(1)
  })

  it('a message deleted elsewhere shows as its tombstone after a reconnect', async () => {
    setOverlayReconnectBaseForTests(5)
    write('p1', 'rosson', 'my password is hunter2', 'compose')
    write('a2', 'sales', 'noted')
    const { view, sock } = await mountLive('conv-1')
    act(() => sock.drop())
    rewrite('p1', (doc) => ({ id: doc.id, kind: 'text', from: doc.from, via: doc.via, body: null, deleted: true, deletedAt: 1_700_000_000 }))
    await waitFor(() => expect(live().length).toBe(1))
    act(() => live()[0].open())
    await waitFor(() => expect(view.result.current.items[0].doc.deleted).toBe(true))
    expect(view.result.current.items[0].doc.body ?? null).toBeNull()
    expect(ids(view.result.current.items)).toEqual(['p1', 'a2'])
    expect(server.catchUps).toEqual([])
  })

  it('a row that left the Thread (a fork split) leaves the list', async () => {
    write('a1', 'sales', 'stays')
    write('f2', 'sales', 'moves to its own chat')
    const { view } = await mountLive('conv-1')
    server.rows = server.rows.filter((r) => r.id !== 'f2')
    server.removedRows.push({ id: 'f2', seq: 2, mark: server.seq })
    focusLater()
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['a1']))
  })

  it('the cursor is the last server read, never a newer row a frame brought (SP-8)', async () => {
    card('c1')
    const { view, sock } = await mountLive('conv-1')
    // A frame brings seq 2; the card's change frame (marked at 2) is lost;
    // a frame brings seq 3.
    act(() => sock.frame(write('a2', 'sales', 'one')))
    rewrite('c1', (doc) => ({ ...doc, choice: { ...(doc.choice as Record<string, unknown>), status: 'answered', answer: 'Ship' } }))
    act(() => sock.frame(write('a3', 'sales', 'two')))
    expect(view.result.current.items[0].doc.choice?.status).toBe('pending')
    focusLater()
    await waitFor(() => expect(view.result.current.items[0].doc.choice?.status).toBe('answered'))
    expect(server.deltas[server.deltas.length - 1]).toBe(1)
    // The delta repeated a2 and a3: still one row each.
    expect(ids(view.result.current.items)).toEqual(['c1', 'a2', 'a3'])
    // The next re-sync starts from the head that read returned.
    focusLater()
    await waitFor(() => expect(server.deltas[server.deltas.length - 1]).toBe(3))
  })

  it('a gap past one delta page falls back to the full re-read, with no duplicates', async () => {
    setOverlayReconnectBaseForTests(5)
    write('a1', 'sales', 'hello')
    write('a2', 'sales', 'again')
    const { view, sock } = await mountLive('conv-1')
    act(() => sock.drop())
    server.deltaLimit = 2
    write('g3', 'rosson', 'one', 'compose')
    write('g4', 'rosson', 'two', 'compose')
    write('g5', 'rosson', 'three', 'compose')
    await waitFor(() => expect(live().length).toBe(1))
    act(() => live()[0].open())
    await waitFor(() => expect(ids(view.result.current.items)).toEqual(['a1', 'a2', 'g3', 'g4', 'g5']))
    expect(server.deltas).toEqual([2, 2])
    // The full read of the loaded window (oldest − 1).
    expect(server.catchUps).toEqual([0])
    // The cursor moved to the head: the next re-sync is a delta again.
    focusLater()
    await waitFor(() => expect(server.deltas[server.deltas.length - 1]).toBe(5))
  })

  it('a server without thread-delta-v1 re-reads the loaded window, as before', async () => {
    const supports = vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => f !== 'thread-delta-v1')
    try {
      write('a1', 'sales', 'hello')
      const { view } = await mountLive('conv-1')
      write('a2', 'sales', 'more')
      focusLater()
      await waitFor(() => expect(ids(view.result.current.items)).toEqual(['a1', 'a2']))
      expect(server.deltas).toEqual([])
      expect(server.catchUps).toEqual([1, 0])
    } finally {
      supports.mockRestore()
    }
  })

  it('an answer that is not a delta (an older daemon ignoring after_seq) falls back to the full read', async () => {
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    write('a2', 'sales', 'more')
    const base = daemonCliGet.getMockImplementation()
    if (!base) throw new Error('no fake daemon')
    daemonCliGet.mockImplementation(async (route: string, p: Record<string, unknown>) => {
      if (route === 'thread' && p.after_seq !== undefined) {
        server.deltas.push(Number(p.after_seq))
        return { conversation_id: server.conv, has_more: false, items: server.rows.slice(-25) }
      }
      return base(route, p)
    })
    focusLater()
    await waitFor(() => expect(server.catchUps).toEqual([0]))
    expect(ids(view.result.current.items)).toEqual(['a1', 'a2'])
  })

  it('a refused delta (400) falls back to the full read', async () => {
    const { DaemonHttpError } = await import('@/lib/daemon-http-error')
    write('a1', 'sales', 'hello')
    const { view } = await mountLive('conv-1')
    write('a2', 'sales', 'more')
    const base = daemonCliGet.getMockImplementation()
    if (!base) throw new Error('no fake daemon')
    daemonCliGet.mockImplementation(async (route: string, p: Record<string, unknown>) => {
      if (route === 'thread' && p.after_seq !== undefined) {
        throw new DaemonHttpError("invalid after_seq", 400, 'usage')
      }
      return base(route, p)
    })
    focusLater()
    await waitFor(() => expect(server.catchUps).toEqual([0]))
    expect(ids(view.result.current.items)).toEqual(['a1', 'a2'])
  })

  it('a pushed row is provisional: the saved copy’s cursor stays the last server read until a read confirms it (P3b)', async () => {
    write('a1', 'sales', 'hello')
    const { view, sock } = await mountLive('conv-1')
    act(() => sock.frame(write('a2', 'sales', 'pushed')))
    expect(view.result.current.items.map((it) => [it.id, it.provisional ?? false])).toEqual([
      ['a1', false],
      ['a2', true],
    ])
    await waitFor(async () => expect((await listThreadViews('local')).map((m) => m.cursor)).toEqual([1]))
    // A re-sync's delta carries a2: the server's copy confirms it.
    focusLater()
    await waitFor(() => expect(view.result.current.items[1].provisional).toBeUndefined())
    await waitFor(async () => expect((await listThreadViews('local')).map((m) => m.cursor)).toEqual([2]))
  })
})
