// The catch-up (speed pass P3b): one POST per server brings cached Threads
// up to date in the view cache. Logic after K2 Companion's
// `src/zen/chatCatchup.test.ts`, on the desktop's cache.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async () => {
    throw new Error('no reads here')
  },
  daemonCliPost: async () => {
    throw new Error('posts go through deps.post here')
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayThreadItem } from './overlayThread'
import { catchUpServer, markThreadOpen, resetChatCatchupForTests, CATCHUP_MAX_ROUNDS } from './chatCatchup'
import { parseCatchupAnswer, type CatchupResult, type catchupBody } from './threadCatchup'
import { peekThreadView, readThreadView, resetThreadViewCacheForTests, saveThreadView } from './threadViewCache'

const HOST = 'local'
const scope = { hostKey: HOST, isRemote: false, serverSupports: () => true } as unknown as ServerScope

function row(seq: number, over: Partial<OverlayThreadItem> = {}, body = `m${seq}`): OverlayThreadItem {
  const id = over.id ?? `t${seq}`
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'k2', body }, ...over }
}
const rows = (a: number, b: number) => Array.from({ length: b - a + 1 }, (_, i) => row(a + i))
const ids = (items: readonly OverlayThreadItem[] | undefined) => (items ?? []).map((it) => it.id)

type Body = ReturnType<typeof catchupBody>

/** A fake daemon: answers each body with `answer(ask, round)`. */
function fakePost(answer: (ask: Body['threads'][number], round: number) => unknown) {
  const bodies: Body[] = []
  const post = vi.fn(async (_s: ServerScope, body: Body): Promise<CatchupResult> => {
    bodies.push(body)
    const round = bodies.length - 1
    return { ok: true, data: parseCatchupAnswer({ threads: body.threads.map((a) => answer(a, round)) }, body) }
  })
  return { post, bodies }
}

beforeEach(async () => {
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  await resetThreadViewCacheForTests()
  resetChatCatchupForTests()
})

describe('catchUpServer', () => {
  it('a cached Thread asks from its newest server seq (a pushed row never counts) and merges the delta into the cache', async () => {
    const card = row(2, {}, 'Ship?')
    card.doc = { ...card.doc, kind: 'choice', choice: { prompt: 'Ship?', options: [{ label: 'Ship' }], allow_custom: false, status: 'pending', answer: null } }
    // Cached to 3; a push brought 5 (4 came with no frame).
    saveThreadView(HOST, 'sales', 'c1', [row(1), card, row(3), row(5, { provisional: true })])
    const answered = { ...card, doc: { ...card.doc, choice: { ...card.doc.choice!, status: 'answered', answer: 'Ship' } } }
    const { post, bodies } = fakePost((ask) => ({
      addr: ask.addr,
      conversation_id: 'c1',
      after_seq: 3,
      has_more: false,
      items: [row(4), row(5)],
      changed: [answered],
      removed: [{ id: 't1', seq: 1 }],
    }))
    const r = await catchUpServer(scope, ['sales'], { post })
    expect(bodies[0].threads).toEqual([{ addr: 'sales', afterSeq: 3 }])
    expect(r.saved).toEqual(['sales'])
    const v = peekThreadView(HOST, 'sales')
    expect(ids(v?.items)).toEqual(['t2', 't3', 't4', 't5'])
    expect(v?.items[0].doc.choice?.status).toBe('answered')
    // The server's copy confirmed the pushed row: it may be a cursor now.
    expect(v?.items.every((it) => !it.provisional)).toBe(true)
  })

  it('an uncached target asks the newest page and saves it', async () => {
    const { post, bodies } = fakePost((ask) => ({ addr: ask.addr, conversation_id: 'c9', has_more: true, items: rows(48, 50) }))
    await catchUpServer(scope, ['home-row'], { post })
    expect(bodies[0].threads).toEqual([{ addr: 'home-row', newest: true }])
    const v = await readThreadView(HOST, 'home-row')
    expect(ids(v?.items)).toEqual(['t48', 't49', 't50'])
    expect([v?.conversationId, v?.hasMore]).toEqual(['c9', true])
  })

  it('a Thread open in this window is left to its view', async () => {
    saveThreadView(HOST, 'sales', 'c1', rows(1, 3))
    const close = markThreadOpen(HOST, 'sales')
    const { post } = fakePost(() => ({}))
    const r = await catchUpServer(scope, ['sales'], { post })
    expect(post).not.toHaveBeenCalled()
    expect(r.asked).toBe(0)
    close()
    await catchUpServer(scope, ['sales'], { post })
    expect(post).toHaveBeenCalledTimes(1)
  })

  it('deferred entries and long deltas ask again; past the rounds what came lands', async () => {
    saveThreadView(HOST, 'a', 'c1', rows(1, 2))
    saveThreadView(HOST, 'b', 'c2', rows(1, 2))
    const { post, bodies } = fakePost((ask, round) => {
      if (ask.addr === 'b' && round === 0) return { deferred: true }
      if (ask.addr === 'b') return { conversation_id: 'c2', after_seq: 2, has_more: false, items: [row(3)], changed: [], removed: [] }
      // 'a' always has more: two new rows a round.
      const after = 'afterSeq' in ask ? ask.afterSeq : 0
      return { conversation_id: 'c1', after_seq: after, has_more: true, items: [row(after + 1), row(after + 2)], changed: [], removed: [] }
    })
    const r = await catchUpServer(scope, ['a', 'b'], { post })
    expect(bodies).toHaveLength(CATCHUP_MAX_ROUNDS)
    expect(bodies.map((b) => b.threads.find((t) => t.addr === 'a'))).toEqual([
      { addr: 'a', afterSeq: 2 },
      { addr: 'a', afterSeq: 4 },
      { addr: 'a', afterSeq: 6 },
      { addr: 'a', afterSeq: 8 },
    ])
    expect(ids(peekThreadView(HOST, 'b')?.items)).toEqual(['t1', 't2', 't3'])
    // Still behind after the last round: what came lands (contiguous to
    // seq 10); the next catch-up goes on from there.
    expect(r.dropped).toEqual([])
    expect(ids(peekThreadView(HOST, 'a')?.items)).toEqual(ids(rows(1, 10)))
  })

  it('a moved, renamed, reset or gone Thread loses its copy', async () => {
    for (const a of ['moved', 'renamed', 'resync', 'gone', 'other']) saveThreadView(HOST, a, 'c1', rows(1, 2))
    const { post } = fakePost((ask) => {
      if (ask.addr === 'moved') return { conversation_id: 'c-new', after_seq: 2, items: [], changed: [], removed: [] }
      if (ask.addr === 'renamed') return { addr: 'renamed/2', movedFrom: 'renamed', conversation_id: 'c1', items: [] }
      if (ask.addr === 'resync') return { conversation_id: 'c1', resync: true, items: [] }
      if (ask.addr === 'gone') return { status: 404, error: { code: 'not_found', hint: 'gone' } }
      return { status: 500, error: { code: 'store', hint: 'try later' } }
    })
    const r = await catchUpServer(scope, ['moved', 'renamed', 'resync', 'gone', 'other'], { post })
    expect(r.dropped.sort()).toEqual(['gone', 'moved', 'renamed', 'resync'])
    // A server error keeps the copy (the next catch-up tries again).
    expect(peekThreadView(HOST, 'other')).not.toBeNull()
  })

  it('a failed call changes nothing', async () => {
    saveThreadView(HOST, 'sales', 'c1', rows(1, 2))
    const post = vi.fn(async (): Promise<CatchupResult> => ({ ok: false, error: { status: null, code: 'network', hint: 'down' } }))
    const r = await catchUpServer(scope, ['sales'], { post })
    expect(r).toEqual({ asked: 1, saved: [], dropped: [] })
    expect(ids(peekThreadView(HOST, 'sales')?.items)).toEqual(['t1', 't2'])
  })
})
