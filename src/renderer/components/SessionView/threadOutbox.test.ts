// @vitest-environment jsdom
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
// research-remote-thread-load-v1 Fix 3: the "sending" bubble. Enter shows
// the message at once as pending (keyed by a clientMsgId); the server's
// answer or its socket frame replaces it, whichever comes first, never
// both; a failure on the way stays as "Not sent" and Retry sends the SAME
// id. The daemon is faked at the `daemon-cli` / socket boundary.
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

import { postThreadCompose, retryThreadPost, discardThreadPost, type OverlayThreadItem } from './overlayThread'
import { setOverlayReconnectBaseForTests, useOverlayThread } from './useOverlayThread'
import {
  configureOutboxForTests,
  kickOutbox,
  newClientMsgId,
  reconcileOutbox,
  resetOutboxForTests,
  wasPendingBubble,
  type OutboxEntry,
} from './threadOutbox'
import { clearPendingSendsForTests, readPendingSends, resetThreadViewCacheForTests } from './threadViewCache'
import { noteServerVersion, primaryScope, __resetServerScopesForTests } from '@/kessel/server-scope'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import { DaemonHttpError } from '@/lib/daemon-http-error'

/** What `CliTimeoutError` looks like to a caller (daemon-cli is mocked here). */
class CliTimeoutError extends Error {
  constructor() {
    super('no answer from the server in 30 s (thread/post)')
    this.name = 'CliTimeoutError'
  }
}

interface Row {
  collection: 'thread'
  seq: number
  id: string
  doc: Record<string, unknown>
}

const server = { conv: 'conv-1', rows: [] as Row[], seq: 0 }

function write(id: string, body: string, extra: Record<string, unknown> = {}): Row {
  server.seq += 1
  const row: Row = { collection: 'thread', seq: server.seq, id, doc: { id, kind: 'text', from: 'rosson', body, via: 'compose', ...extra } }
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
  frame(row: Row): void {
    this.onmessage?.({ data: JSON.stringify({ collection: row.collection, seq: row.seq, id: row.id, doc: row.doc }) })
  }
  close(): void {
    this.closed = true
    this.readyState = 3
  }
}

/** The answer a `thread/post` gets (what overlay_routes.rs sends). */
function answerFor(row: Row, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { ok: true, id: row.id, seq: row.seq, from: 'rosson', body: row.doc.body, kind: 'text', via: 'compose', conversation_id: server.conv, ...(row.doc.clientMsgId ? { clientMsgId: row.doc.clientMsgId } : {}), ...extra }
}

/** POST bodies sent to `thread/post`. */
function posts(): Array<Record<string, unknown>> {
  return daemonCliPost.mock.calls.filter((c) => c[0] === 'thread/post').map((c) => c[1] as Record<string, unknown>)
}

/** The daemon's thread/post with post-idempotency-v1: a repeat of a
 *  clientMsgId answers the stored post with duplicate:true, never a 2nd row. */
function serverPost(body: Record<string, unknown>): Record<string, unknown> {
  const cid = typeof body.clientMsgId === 'string' ? body.clientMsgId : null
  const prior = cid ? server.rows.find((r) => r.doc.clientMsgId === cid) : undefined
  if (prior) return answerFor(prior, { duplicate: true })
  const row = write(`own-${server.seq + 1}`, String(body.text), cid ? { clientMsgId: cid } : {})
  return answerFor(row)
}

beforeEach(async () => {
  server.conv = 'conv-1'
  server.rows = []
  server.seq = 0
  FakeSocket.all = []
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetThreadViewCacheForTests({ keepDisk: true })
  // No timers or window listeners of the outbox's own: each test drives
  // the reconnect / focus / restart it is about.
  configureOutboxForTests({ autoTimers: false })
  resetOutboxForTests()
  resetGridDialQueueForTests()
  __resetServerScopesForTests()
  vi.stubGlobal('WebSocket', FakeSocket)
  daemonCliGet.mockImplementation(async (route: string, p: Record<string, unknown>) => {
    if (route === 'thread/activity') return { ok: true, turn: null }
    if (route !== 'thread') throw new Error(`unexpected GET ${route}`)
    if (p.since_seq !== undefined) {
      return { conversation_id: server.conv, has_more: false, items: server.rows.filter((r) => r.seq > Number(p.since_seq)) }
    }
    return { conversation_id: server.conv, has_more: false, items: server.rows.slice(-Number(p.limit)) }
  })
})

afterEach(async () => {
  setOverlayReconnectBaseForTests(null)
  resetOutboxForTests()
  await clearPendingSendsForTests()
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  __resetServerScopesForTests()
  vi.unstubAllGlobals()
})

async function mountLive() {
  const view = renderHook(() => useOverlayThread({ scope: primaryScope(), addr: 'sales', conversationId: 'conv-1', enabled: true }))
  await waitFor(() => expect(FakeSocket.all.length).toBe(1))
  const sock = FakeSocket.all[0]
  act(() => sock.open())
  await waitFor(() => expect(view.result.current.loaded).toBe(true))
  return { view, sock }
}

/** A POST answer the test releases. */
function gatePost(): { release: (fn: () => unknown) => void } {
  let resolve!: (v: unknown) => void
  let reject!: (e: unknown) => void
  daemonCliPost.mockImplementationOnce(
    () =>
      new Promise((res, rej) => {
        resolve = res
        reject = rej
      }),
  )
  return {
    release: (fn) => {
      try {
        resolve(fn())
      } catch (e) {
        reject(e)
      }
    },
  }
}

const HEX32 = /^[0-9a-f]{32}$/

describe('the pending bubble', () => {
  it('shows at once with a 32-hex clientMsgId the post carries; the answer replaces it; the echo frame adds nothing', async () => {
    write('a1', 'hello', { via: 'thread', from: 'sales' })
    const { view, sock } = await mountLive()
    const gate = gatePost()
    let result!: Promise<unknown>
    act(() => {
      result = postThreadCompose(primaryScope(), 'sales', 'ship it', null, { pending: true })
    })
    await waitFor(() => expect(view.result.current.outbox).toHaveLength(1))
    const bubble = view.result.current.outbox[0]
    expect(bubble.pending).toMatchObject({ state: 'sending' })
    expect(bubble.doc.body).toBe('ship it')
    const cid = posts()[0].clientMsgId as string
    expect(cid).toMatch(HEX32)
    expect(bubble.pending?.clientMsgId).toBe(cid)
    expect(view.result.current.items.map((it) => it.id)).toEqual(['a1'])

    const row = write('own-2', 'ship it', { clientMsgId: cid })
    await act(async () => {
      gate.release(() => answerFor(row))
      await result
    })
    expect(view.result.current.outbox).toEqual([])
    expect(view.result.current.items.map((it) => it.id)).toEqual(['a1', 'own-2'])
    expect(wasPendingBubble('own-2')).toBe(true)
    act(() => sock.frame(row))
    expect(view.result.current.items.map((it) => it.id)).toEqual(['a1', 'own-2'])
    expect(view.result.current.outbox).toEqual([])
  })

  it('the socket frame can come first: it replaces the bubble at once, and the late answer adds nothing', async () => {
    const { view, sock } = await mountLive()
    const gate = gatePost()
    let result!: Promise<unknown>
    act(() => {
      result = postThreadCompose(primaryScope(), 'sales', 'frame first', null, { pending: true })
    })
    await waitFor(() => expect(view.result.current.outbox).toHaveLength(1))
    const cid = posts()[0].clientMsgId as string
    // The daemon publishes the frame at insert time, before its slow inject.
    const row = write('own-1', 'frame first', { clientMsgId: cid })
    act(() => sock.frame(row))
    await waitFor(() => expect(view.result.current.outbox).toEqual([]))
    expect(view.result.current.items.map((it) => it.id)).toEqual(['own-1'])
    await act(async () => {
      gate.release(() => answerFor(row))
      expect(await result).toMatchObject({ ok: true })
    })
    expect(view.result.current.items.map((it) => it.id)).toEqual(['own-1'])
    expect(view.result.current.outbox).toEqual([])
  })

  it('a failure on the way waits to go again; Retry sends the SAME id; duplicate:true counts as sent', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    let first: unknown
    await act(async () => {
      first = await postThreadCompose(primaryScope(), 'sales', 'flaky net', null, { pending: true })
    })
    expect(first).toMatchObject({ ok: false, kept: true })
    expect(view.result.current.outbox).toHaveLength(1)
    expect(view.result.current.outbox[0].pending).toMatchObject({ state: 'waiting', error: 'Load failed' })
    const cid = posts()[0].clientMsgId as string

    // The first try did land on the server; the retry is a repeat.
    const row = write('own-1', 'flaky net', { clientMsgId: cid })
    daemonCliPost.mockResolvedValueOnce(answerFor(row, { duplicate: true }))
    let retried: unknown
    await act(async () => {
      retried = await retryThreadPost(cid)
    })
    expect(retried).toMatchObject({ ok: true })
    expect(posts()).toHaveLength(2)
    expect(posts()[1]).toEqual(posts()[0])
    expect(posts()[1].clientMsgId).toBe(cid)
    expect(view.result.current.outbox).toEqual([])
    expect(view.result.current.items.map((it) => it.id)).toEqual(['own-1'])
  })

  it('a 409 client_msg_id_reused fails without re-keying: Retry sends the same id again; Discard drops it', async () => {
    const { view } = await mountLive()
    const reused = (): never => {
      throw new DaemonHttpError('this clientMsgId already names a post in another chat', 409, 'client_msg_id_reused')
    }
    daemonCliPost.mockImplementationOnce(async () => reused())
    let first: unknown
    await act(async () => {
      first = await postThreadCompose(primaryScope(), 'sales', 'twice', null, { pending: true })
    })
    expect(first).toMatchObject({ ok: false, kept: true, code: 'client_msg_id_reused' })
    expect(view.result.current.outbox[0].pending?.state).toBe('failed')
    const cid = posts()[0].clientMsgId as string
    daemonCliPost.mockImplementationOnce(async () => reused())
    await act(async () => {
      await retryThreadPost(cid)
    })
    expect(posts().map((b) => b.clientMsgId)).toEqual([cid, cid])
    expect(view.result.current.outbox[0].pending?.state).toBe('failed')
    act(() => discardThreadPost(cid))
    expect(view.result.current.outbox).toEqual([])
  })

  it('a refusal (4xx) takes the bubble away and returns the draft path (not kept)', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new DaemonHttpError('attachment refused', 400, 'usage'))
    let r: unknown
    await act(async () => {
      r = await postThreadCompose(primaryScope(), 'sales', 'no', null, { pending: true })
    })
    expect(r).toMatchObject({ ok: false })
    expect((r as { kept?: boolean }).kept).toBeUndefined()
    expect(view.result.current.outbox).toEqual([])
  })

  it('a refusal on a later try keeps it as "Not sent" (Retry / Delete), never drops it', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'later refused', null, { pending: true })
    })
    const cid = posts()[0].clientMsgId as string
    daemonCliPost.mockRejectedValueOnce(new DaemonHttpError('not allowed', 403, 'forbidden'))
    await act(async () => {
      await kickOutbox({ force: true })
    })
    expect(view.result.current.outbox.map((b) => b.pending)).toEqual([{ clientMsgId: cid, state: 'failed', error: 'not allowed' }])
    act(() => discardThreadPost(cid))
    expect(view.result.current.outbox).toEqual([])
    await waitFor(async () => expect(await readPendingSends()).toEqual([]))
  })

  it('a send without pending (Zen, a Garden widget) carries no id and shows no bubble', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockResolvedValueOnce(answerFor(write('own-1', 'from Zen')))
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'from Zen')
    })
    expect(posts()[0]).toEqual({ addr: 'sales', text: 'from Zen', via: 'compose' })
    expect(view.result.current.outbox).toEqual([])
  })
})

/** Mount a view of `sales` and open its socket (the n-th one made). */
async function mountView(n: number) {
  const view = renderHook(() => useOverlayThread({ scope: primaryScope(), addr: 'sales', conversationId: 'conv-1', enabled: true }))
  await waitFor(() => expect(FakeSocket.all.length).toBe(n + 1))
  act(() => FakeSocket.all[n].open())
  await waitFor(() => expect(view.result.current.loaded).toBe(true))
  return view
}

function rowsWith(body: string): Row[] {
  return server.rows.filter((r) => r.doc.body === body)
}

describe('sending survives sleep, catch-up, time outs and restarts (thread-send-resilience)', () => {
  it('a send while the server is out of reach waits in the outbox; the socket reconnecting posts it once', async () => {
    setOverlayReconnectBaseForTests(1)
    const { view, sock } = await mountLive()
    // Woken from sleep: the pooled connection is dead.
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    let first: unknown
    await act(async () => {
      first = await postThreadCompose(primaryScope(), 'sales', 'typed before the lid closed', null, { pending: true })
    })
    expect(first).toMatchObject({ ok: false, kept: true })
    expect(view.result.current.outbox.map((b) => b.pending?.state)).toEqual(['waiting'])
    const cid = posts()[0].clientMsgId as string
    expect(rowsWith('typed before the lid closed')).toEqual([])
    // On disk, so a reload could not lose it either.
    await waitFor(async () => expect((await readPendingSends()).map((r) => r.clientMsgId)).toEqual([cid]))

    // The server is back: the Thread's socket drops and reconnects.
    daemonCliPost.mockImplementation(async (route: string, body: Record<string, unknown>) => {
      if (route !== 'thread/post') throw new Error(`unexpected POST ${route}`)
      return serverPost(body)
    })
    act(() => {
      sock.close()
      sock.onclose?.(new Event('close'))
    })
    await waitFor(() => expect(FakeSocket.all.length).toBe(2))
    act(() => FakeSocket.all[1].open())
    await waitFor(() => expect(view.result.current.outbox).toEqual([]))
    expect(posts().map((b) => b.clientMsgId)).toEqual([cid, cid])
    expect(rowsWith('typed before the lid closed')).toHaveLength(1)
    expect(view.result.current.items.map((it) => it.doc.body)).toEqual(['typed before the lid closed'])
    await waitFor(async () => expect(await readPendingSends()).toEqual([]))
  })

  it('a catch-up (new agent messages after wake) while a send is pending never drops it; it stays last', async () => {
    const { view, sock } = await mountLive()
    const gate = gatePost()
    let result!: Promise<unknown>
    act(() => {
      result = postThreadCompose(primaryScope(), 'sales', 'my words', null, { pending: true })
    })
    await waitFor(() => expect(view.result.current.outbox).toHaveLength(1))
    const cid = posts()[0].clientMsgId as string
    // The agent's replies from while the lid was closed come in.
    const r1 = write('a1', 'while you were away', { via: 'thread', from: 'sales' })
    const r2 = write('a2', 'and another', { via: 'thread', from: 'sales' })
    act(() => sock.frame(r1))
    act(() => sock.frame(r2))
    await waitFor(() => expect(view.result.current.items.map((it) => it.id)).toEqual(['a1', 'a2']))
    expect(view.result.current.outbox.map((b) => [b.pending?.clientMsgId, b.pending?.state, b.doc.body])).toEqual([
      [cid, 'sending', 'my words'],
    ])
    // The Thread view remounting (a room's failure gate, a tab switch) keeps it too.
    view.unmount()
    const again = await mountView(1)
    expect(again.result.current.items.map((it) => it.id)).toEqual(['a1', 'a2'])
    expect(again.result.current.outbox.map((b) => b.pending?.clientMsgId)).toEqual([cid])
    const row = write('own-3', 'my words', { clientMsgId: cid })
    await act(async () => {
      gate.release(() => answerFor(row))
      await result
    })
    expect(again.result.current.outbox).toEqual([])
    expect(again.result.current.items.map((it) => it.id)).toEqual(['a1', 'a2', 'own-3'])
  })

  it('a post that times out after the server stored it: the retry on focus gets the stored post, no duplicate', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockImplementationOnce(async (_route: string, body: Record<string, unknown>) => {
      serverPost(body) // stored...
      throw new CliTimeoutError() // ...but the answer never came
    })
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'stored but unanswered', null, { pending: true })
    })
    const cid = posts()[0].clientMsgId as string
    expect(rowsWith('stored but unanswered')).toHaveLength(1)
    expect(view.result.current.outbox.map((b) => b.pending?.state)).toEqual(['waiting'])
    const answers: Array<Record<string, unknown>> = []
    daemonCliPost.mockImplementation(async (_route: string, body: Record<string, unknown>) => {
      const a = serverPost(body)
      answers.push(a)
      return a
    })
    // The app gets focus again.
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await waitFor(() => expect(view.result.current.outbox).toEqual([]))
    expect(posts().map((b) => b.clientMsgId)).toEqual([cid, cid])
    expect(answers.map((a) => a.duplicate)).toEqual([true])
    expect(rowsWith('stored but unanswered')).toHaveLength(1)
    await waitFor(() => expect(view.result.current.items.filter((it) => it.doc.body === 'stored but unanswered')).toHaveLength(1))
  })

  it('two sends while out of reach go oldest first, each once', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValue(new TypeError('Load failed'))
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'first', null, { pending: true })
    })
    let second: unknown
    await act(async () => {
      second = await postThreadCompose(primaryScope(), 'sales', 'second', null, { pending: true })
    })
    // The second waits behind the first (it is not tried on its own).
    expect(second).toMatchObject({ ok: false, kept: true })
    expect(view.result.current.outbox.map((b) => [b.doc.body, b.pending?.state])).toEqual([
      ['first', 'waiting'],
      ['second', 'waiting'],
    ])
    daemonCliPost.mockReset()
    daemonCliPost.mockImplementation(async (_route: string, body: Record<string, unknown>) => serverPost(body))
    await act(async () => {
      await kickOutbox({ force: true })
    })
    await waitFor(() => expect(view.result.current.outbox).toEqual([]))
    expect(server.rows.map((r) => r.doc.body)).toEqual(['first', 'second'])
  })

  it('an unsent post survives an app restart and goes once when its Thread opens', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'across a restart', null, { pending: true })
    })
    const cid = posts()[0].clientMsgId as string
    await waitFor(async () => expect((await readPendingSends()).map((r) => [r.clientMsgId, r.text, r.state])).toEqual([[cid, 'across a restart', 'waiting']]))
    view.unmount()

    // Restart: this window's memory is gone; the disk stays.
    resetOutboxForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    daemonCliPost.mockImplementation(async (_route: string, body: Record<string, unknown>) => serverPost(body))
    const after = await mountView(1)
    await waitFor(() => expect(after.result.current.outbox).toEqual([]))
    expect(posts().map((b) => b.clientMsgId)).toEqual([cid, cid])
    expect(rowsWith('across a restart')).toHaveLength(1)
    await waitFor(() => expect(after.result.current.items.map((it) => it.doc.body)).toEqual(['across a restart']))
    await waitFor(async () => expect(await readPendingSends()).toEqual([]))
  })

  it('a send made before the restart shows from disk as pending until it goes', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    await act(async () => {
      await postThreadCompose(primaryScope(), 'sales', 'shown from disk', null, { pending: true })
    })
    await waitFor(async () => expect(await readPendingSends()).toHaveLength(1))
    view.unmount()
    resetOutboxForTests()
    await resetThreadViewCacheForTests({ keepDisk: true })
    // Still out of reach after the restart.
    daemonCliPost.mockRejectedValue(new TypeError('Load failed'))
    const after = await mountView(1)
    await waitFor(() => expect(after.result.current.outbox.map((b) => [b.doc.body, b.pending?.state])).toEqual([['shown from disk', 'waiting']]))
    expect(rowsWith('shown from disk')).toEqual([])
  })
})

describe('a server without post-idempotency-v1', () => {
  it('the id rides anyway (an older server ignores it); the bubble is matched by text (never an older identical post); never drawn twice', async () => {
    // This computer's daemon read as an older one: no such feature.
    noteServerVersion('local', null, ['daemon-activity'])
    write('old-1', 'same words')
    const { view, sock } = await mountLive()
    const gate = gatePost()
    let result!: Promise<unknown>
    act(() => {
      result = postThreadCompose(primaryScope(), 'sales', 'same words', null, { pending: true })
    })
    await waitFor(() => expect(view.result.current.outbox).toHaveLength(1))
    expect(posts()[0].clientMsgId).toMatch(HEX32)
    // The older post with the same words does not answer it.
    expect(view.result.current.items.map((it) => it.id)).toEqual(['old-1'])
    const row = write('own-2', 'same words', { created_at: Math.floor(Date.now() / 1000) })
    act(() => sock.frame(row))
    await waitFor(() => expect(view.result.current.outbox).toEqual([]))
    expect(view.result.current.items.map((it) => it.id)).toEqual(['old-1', 'own-2'])
    await act(async () => {
      gate.release(() => answerFor(row))
      await result
    })
    expect(view.result.current.items.map((it) => it.id)).toEqual(['old-1', 'own-2'])
    expect(view.result.current.outbox).toEqual([])
  })
})

describe('reconcileOutbox', () => {
  const entry = (over: Partial<OutboxEntry>): OutboxEntry => ({
    clientMsgId: newClientMsgId(),
    key: 'local|sales',
    hostKey: 'local',
    scope: primaryScope(),
    addr: 'sales',
    text: 'hi',
    body: {},
    keyed: true,
    createdAtSec: 1_000,
    state: 'sending',
    attempts: 0,
    nextAttemptAtMs: 0,
    login: '',
    ...over,
  })
  const item = (id: string, doc: Record<string, unknown>): OverlayThreadItem => ({
    collection: 'thread',
    seq: 1,
    id,
    doc: { id, kind: 'text', from: 'rosson', via: 'compose', body: 'hi', ...doc },
  })

  it('a keyed entry matches only its own id; each server item answers one entry', () => {
    const a = entry({ createdAtSec: 1_000 })
    const b = entry({ createdAtSec: 1_001 })
    const { matched, pending } = reconcileOutbox([a, b], [item('x', { clientMsgId: b.clientMsgId }), item('y', { body: 'hi' })])
    expect(matched).toEqual([{ entry: b, id: 'x' }])
    expect(pending.map((p) => p.pending?.clientMsgId)).toEqual([a.clientMsgId])
  })

  it('unkeyed: same words, a person post, around the send time, not in the baseline; one item for one entry', () => {
    const a = entry({ keyed: false, createdAtSec: 1_000 })
    const b = entry({ keyed: false, createdAtSec: 1_002 })
    const baseline = new Map([[a.clientMsgId, new Set(['old'])], [b.clientMsgId, new Set(['old'])]])
    const r = reconcileOutbox([a, b], [item('old', {}), item('new', { created_at: 1_001 })], baseline)
    expect(r.matched).toEqual([{ entry: a, id: 'new' }])
    expect(r.pending).toHaveLength(1)
    // An agent's line with the same words, or one far from the send time, is not it.
    expect(reconcileOutbox([a], [item('ag', { via: 'thread' })]).matched).toEqual([])
    expect(reconcileOutbox([a], [item('late', { created_at: 5_000 })]).matched).toEqual([])
  })

  it('unkeyed with a send-time seq: only an item newer than the Thread was at send time answers it', () => {
    const a = entry({ keyed: false, createdAtSec: 1_000, afterSeq: 5 })
    const at = (seq: number, id: string): OverlayThreadItem => ({ ...item(id, { created_at: 1_000 }), seq })
    expect(reconcileOutbox([a], [at(5, 'old')]).matched).toEqual([])
    expect(reconcileOutbox([a], [at(5, 'old'), at(6, 'new')]).matched).toEqual([{ entry: a, id: 'new' }])
  })

  it('makes 32 lowercase hex ids', () => {
    const ids = new Set(Array.from({ length: 50 }, () => newClientMsgId()))
    expect(ids.size).toBe(50)
    for (const id of ids) expect(id).toMatch(HEX32)
  })
})
