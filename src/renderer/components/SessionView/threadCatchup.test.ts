import { describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async () => {
    throw new Error('no reads here')
  },
  daemonCliPost: async () => {
    throw new Error('posts go through deps.post here')
  },
}))

import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { CATCHUP_MAX_THREADS, catchupBody, parseThreadEntry, postCatchup, type CatchupThreadAsk } from './threadCatchup'

function scope(features: string[] = []): ServerScope {
  return { hostKey: 'local', isRemote: false, serverSupports: (f: string) => features.includes(f) } as unknown as ServerScope
}

const row = (seq: number) => ({ collection: 'thread', seq, id: `t${seq}`, doc: { id: `t${seq}`, kind: 'text', from: 'k2', body: `m${seq}` } })

describe('catchupBody', () => {
  it('one ask per address (the first wins), bad cursors out, at most the cap', () => {
    const asks: CatchupThreadAsk[] = [
      { addr: 'a', afterSeq: 4 },
      { addr: 'a', newest: true },
      { addr: 'b', newest: true },
      { addr: 'c', afterSeq: -1 },
      { addr: 'd', afterSeq: 1.5 },
      { addr: '', newest: true },
    ]
    expect(catchupBody(asks)).toEqual({ threads: [{ addr: 'a', afterSeq: 4 }, { addr: 'b', newest: true }], limit: 50 })
    const many = Array.from({ length: CATCHUP_MAX_THREADS + 5 }, (_, i) => ({ addr: `x${i}`, newest: true as const }))
    expect(catchupBody(many).threads).toHaveLength(CATCHUP_MAX_THREADS)
  })
})

describe('parseThreadEntry', () => {
  const sent: CatchupThreadAsk = { addr: 'sales', afterSeq: 3 }
  it('a served delta: items, changed and removed as the hook keeps them', () => {
    const e = parseThreadEntry(
      { addr: 'sales', conversation_id: 'c1', after_seq: 3, has_more: false, items: [row(5), row(4)], changed: [row(2)], removed: [{ id: 't1', seq: 1 }, { nope: 1 }] },
      sent,
    )
    if (e.kind !== 'served') throw new Error(`not served: ${e.kind}`)
    expect(e.items.map((it) => it.seq)).toEqual([4, 5])
    expect(e.changed.map((it) => it.id)).toEqual(['t2'])
    expect(e.removed).toEqual([{ id: 't1' }])
    expect([e.conversationId, e.afterSeq, e.hasMore, e.movedFrom, e.resync]).toEqual(['c1', 3, false, null, false])
  })

  it('deferred, missing and error entries', () => {
    expect(parseThreadEntry({ deferred: true }, sent)).toEqual({ kind: 'deferred', addr: 'sales' })
    expect(parseThreadEntry(undefined, sent)).toEqual({ kind: 'deferred', addr: 'sales' })
    expect(parseThreadEntry({ status: 404, error: { code: 'not_found', hint: 'no such Thread' } }, sent)).toEqual({
      kind: 'error',
      addr: 'sales',
      error: { status: 404, code: 'not_found', hint: 'no such Thread' },
    })
  })

  it('a renamed chat answers at its new address with movedFrom', () => {
    const e = parseThreadEntry({ addr: 'sales/reviewer', movedFrom: 'sales', items: [] }, sent)
    expect(e).toMatchObject({ kind: 'served', addr: 'sales/reviewer', movedFrom: 'sales' })
  })
})

describe('postCatchup', () => {
  it('posts the body with cards=2 on a server with permission cards, and matches entries by position', async () => {
    const post = vi.fn(async () => ({ ok: true, has_more: false, threads: [{ items: [row(1)], conversation_id: 'c1' }, { deferred: true }] }))
    const body = catchupBody([{ addr: 'a', newest: true }, { addr: 'b', afterSeq: 2 }])
    const r = await postCatchup(scope(['permission-cards-v1']), body, { post })
    expect(post).toHaveBeenCalledWith(expect.anything(), 'thread/catchup', { threads: body.threads, limit: 50, cards: 2 })
    if (!r.ok) throw new Error(r.error.hint)
    expect(r.data.threads.map((t) => t.kind)).toEqual(['served', 'deferred'])
  })

  it('a 429 waits its retryAfterMs and tries again, twice at most; other errors come back as failures', async () => {
    const busy = new DaemonHttpError('busy', 429, 'rate_limited', null, { error: 'rate_limited', retryAfterMs: 250 })
    const sleep = vi.fn(async () => {})
    const post = vi.fn().mockRejectedValueOnce(busy).mockResolvedValueOnce({ threads: [] })
    const ok = await postCatchup(scope(), catchupBody([]), { post, sleep })
    expect(ok.ok).toBe(true)
    expect(sleep).toHaveBeenCalledWith(250)

    const always = vi.fn().mockRejectedValue(busy)
    const r = await postCatchup(scope(), catchupBody([]), { post: always, sleep })
    expect(always).toHaveBeenCalledTimes(3)
    expect(r).toEqual({ ok: false, error: { status: 429, code: 'rate_limited', hint: 'busy' } })

    const down = vi.fn().mockRejectedValue(new Error('tunnel down'))
    expect(await postCatchup(scope(), catchupBody([]), { post: down, sleep })).toEqual({
      ok: false,
      error: { status: null, code: 'network', hint: 'tunnel down' },
    })
  })
})
