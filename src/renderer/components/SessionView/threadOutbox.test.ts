// @vitest-environment jsdom
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
import { useOverlayThread } from './useOverlayThread'
import {
  newClientMsgId,
  reconcileOutbox,
  resetOutboxForTests,
  wasPendingBubble,
  type OutboxEntry,
} from './threadOutbox'
import { noteServerVersion, primaryScope, __resetServerScopesForTests } from '@/kessel/server-scope'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import { DaemonHttpError } from '@/lib/daemon-http-error'

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

beforeEach(() => {
  server.conv = 'conv-1'
  server.rows = []
  server.seq = 0
  FakeSocket.all = []
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

afterEach(() => {
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

  it('a failure on the way stays as "Not sent"; Retry sends the SAME id; duplicate:true counts as sent', async () => {
    const { view } = await mountLive()
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    let first: unknown
    await act(async () => {
      first = await postThreadCompose(primaryScope(), 'sales', 'flaky net', null, { pending: true })
    })
    expect(first).toMatchObject({ ok: false, kept: true })
    expect(view.result.current.outbox).toHaveLength(1)
    expect(view.result.current.outbox[0].pending).toMatchObject({ state: 'failed', error: 'Load failed' })
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

describe('a server without post-idempotency-v1', () => {
  it('sends no id; the bubble still shows and is matched by text (never an older identical post); never drawn twice', async () => {
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
    expect(posts()[0]).not.toHaveProperty('clientMsgId')
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
    key: 'primary|sales',
    scope: primaryScope(),
    addr: 'sales',
    text: 'hi',
    body: {},
    keyed: true,
    createdAtSec: 1_000,
    state: 'sending',
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

  it('makes 32 lowercase hex ids', () => {
    const ids = new Set(Array.from({ length: 50 }, () => newClientMsgId()))
    expect(ids.size).toBe(50)
    for (const id of ids) expect(id).toMatch(HEX32)
  })
})
