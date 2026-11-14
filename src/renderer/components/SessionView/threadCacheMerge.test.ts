// Ported from K2 Companion's `src/zen/threadCacheMerge.test.ts` (MIT,
// Alakazam Labs), retyped to the desktop's OverlayThreadItem. The project
// chat case stays on the phone.
import { describe, expect, it } from 'vitest'
import type { OverlayDoc, OverlayThreadItem } from './overlayThread'
import { cacheSlice, mergeDelta, mergeServerPage, newestSeq, removeIds, shareUnchanged } from './threadCacheMerge'

function msg(seq: number, over: Partial<OverlayThreadItem> = {}, doc: Partial<OverlayDoc> = {}): OverlayThreadItem {
  const id = over.id ?? `m${seq}`
  return {
    collection: 'thread',
    seq,
    id,
    doc: { id, kind: 'text', from: 'agent', to: null, body: `text ${seq}`, via: 'thread', choice: null, secret: null, created_at: seq, ...doc },
    ...over,
  }
}
const ids = (t: OverlayThreadItem[]) => t.map((m) => m.id)
const range = (a: number, b: number) => Array.from({ length: b - a + 1 }, (_, i) => msg(a + i))
const card = (answer: string | null) => ({ prompt: 'Ok?', options: [{ label: 'Yes' }, { label: 'No' }], allow_custom: false, status: answer ? 'answered' : 'pending', answer })

describe('mergeServerPage', () => {
  it('the server copy wins for overlapping messages (an answered card)', () => {
    const cached = [msg(1), msg(2), msg(3, {}, { kind: 'choice', choice: card(null) })]
    const answered = msg(3, {}, { kind: 'choice', choice: card('Yes') })
    const out = mergeServerPage(cached, [msg(1), msg(2), answered], { hasMore: false })
    expect(out[2].doc.choice?.answer).toBe('Yes')
    // Unchanged messages keep their objects.
    expect(out[0]).toBe(cached[0])
    expect(out[1]).toBe(cached[1])
  })

  it('an unchanged page keeps the very same array (no re-render)', () => {
    const cached = range(1, 5)
    const page = range(1, 5).map((m) => ({ ...m }))
    expect(mergeServerPage(cached, page, { hasMore: false })).toBe(cached)
  })

  it('drops cached messages the page proves gone inside its window only', () => {
    // Cache: 1..10; the server's newest page covers 6..10 without 8, and
    // has older messages.
    const cached = range(1, 10)
    const page = [msg(6), msg(7), msg(9), msg(10)]
    const out = mergeServerPage(cached, page, { hasMore: true })
    expect(ids(out)).toEqual(['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm7', 'm9', 'm10'])
  })

  it('without more history, cached messages older than the page are gone too', () => {
    const out = mergeServerPage(range(1, 10), [msg(6), msg(7)], { hasMore: false })
    expect(ids(out)).toEqual(['m6', 'm7'])
  })

  it('new server messages are appended in seq order', () => {
    const out = mergeServerPage(range(1, 3), range(2, 6), { hasMore: true })
    expect(ids(out)).toEqual(['m1', 'm2', 'm3', 'm4', 'm5', 'm6'])
  })

  it('cached messages newer than the page are gone, unless a live frame brought them meanwhile', () => {
    const cached = [...range(1, 4), msg(5), msg(6)]
    const page = range(1, 4)
    expect(ids(mergeServerPage(cached, page, { hasMore: false }))).toEqual(['m1', 'm2', 'm3', 'm4'])
    const live = new Set(['m6'])
    expect(ids(mergeServerPage(cached, page, { hasMore: false, arrivedLive: live }))).toEqual(['m1', 'm2', 'm3', 'm4', 'm6'])
  })

  it('keeps this window’s unsent posts', () => {
    const pending = msg(99, { id: 'local-1', pending: { state: 'sending' } as OverlayThreadItem['pending'] })
    const failed = msg(98, { id: 'local-2', pending: { state: 'failed', error: 'offline' } as OverlayThreadItem['pending'] })
    const out = mergeServerPage([msg(1), pending, failed], [msg(1), msg(2)], { hasMore: false })
    expect(ids(out)).toEqual(['m1', 'm2', 'local-1', 'local-2'])
  })

  it('another conversation replaces the cached copy whole', () => {
    const out = mergeServerPage(range(10, 20), [msg(1), msg(2)], { hasMore: true, sameConversation: false })
    expect(ids(out)).toEqual(['m1', 'm2'])
  })

  it('an empty page empties the Thread', () => {
    expect(mergeServerPage(range(1, 3), [], { hasMore: false })).toEqual([])
  })

  it('a deleted message from the server replaces the cached body', () => {
    const cached = [msg(1)]
    const out = mergeServerPage(cached, [msg(1, {}, { body: null, deleted: true })], { hasMore: false })
    expect(out[0].doc.deleted).toBe(true)
    expect(out[0].doc.body).toBeNull()
  })
})

describe('mergeDelta / removeIds', () => {
  it('appends new messages and replaces changed older ones by id', () => {
    const cached = range(1, 3)
    const changed = msg(2, {}, { body: 'edited' })
    const out = mergeDelta(cached, [changed, msg(4), msg(5)])
    expect(ids(out)).toEqual(['m1', 'm2', 'm3', 'm4', 'm5'])
    expect(out[1].doc.body).toBe('edited')
    expect(out[0]).toBe(cached[0])
  })

  it('an empty delta changes nothing', () => {
    const cached = range(1, 3)
    expect(mergeDelta(cached, [])).toBe(cached)
  })

  it('removeIds drops a delta’s removed rows, never an unsent post', () => {
    const pending = msg(9, { id: 'local-1', pending: { state: 'sending' } as OverlayThreadItem['pending'] })
    const cached = [...range(1, 3), pending]
    expect(ids(removeIds(cached, ['m2', 'local-1']))).toEqual(['m1', 'm3', 'local-1'])
    expect(removeIds(cached, [])).toBe(cached)
    expect(removeIds(cached, ['nope'])).toBe(cached)
  })
})

describe('cacheSlice / newestSeq / shareUnchanged', () => {
  it('keeps the newest N settled messages and says when it cut', () => {
    const thread = [...range(1, 120), msg(121, { id: 'p', pending: { state: 'sending' } as OverlayThreadItem['pending'] })]
    const s = cacheSlice(thread, 100)
    expect(s.truncated).toBe(true)
    expect(s.messages).toHaveLength(100)
    expect(s.messages[0].id).toBe('m21')
    expect(s.messages.some((m) => m.pending)).toBe(false)
    expect(cacheSlice(range(1, 5), 100).truncated).toBe(false)
  })

  it('newestSeq skips unsent rows', () => {
    expect(newestSeq([msg(1), msg(7), msg(9, { pending: { state: 'sending' } as OverlayThreadItem['pending'] })])).toBe(7)
    expect(newestSeq([])).toBeNull()
  })

  it('shareUnchanged returns prev when equal', () => {
    const a = range(1, 3)
    expect(shareUnchanged(a, a.map((m) => ({ ...m })))).toBe(a)
  })
})

describe('messages from a push (provisional)', () => {
  it('never move the delta cursor: a gap before one is still fetched', () => {
    // Cached to 10; a push brought 12 (11 came with no frame).
    const held = [msg(9), msg(10), msg(12, { provisional: true })]
    expect(newestSeq(held)).toBe(10)
    // The delta after 10 brings 11 and the confirmed 12.
    const merged = mergeDelta(held, [msg(11), msg(12)])
    expect(ids(merged)).toEqual(['m9', 'm10', 'm11', 'm12'])
    expect(merged[3].provisional).toBeUndefined()
    expect(newestSeq(merged)).toBe(12)
  })

  it('the server’s copy confirms a pushed one, even with the same doc', () => {
    const held = [msg(1), msg(2, { provisional: true })]
    const merged = mergeDelta(held, [msg(2)])
    expect(merged[1]).not.toBe(held[1])
    expect(merged[1].provisional).toBeUndefined()
  })

  it('a newest page confirms it inside its window', () => {
    const held = [msg(1), msg(2, { provisional: true })]
    const merged = mergeServerPage(held, [msg(1), msg(2)], { hasMore: false })
    expect(merged[1].provisional).toBeUndefined()
    expect(newestSeq(merged)).toBe(2)
  })
})
