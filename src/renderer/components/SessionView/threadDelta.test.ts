import { describe, expect, it } from 'vitest'
import type { OverlayThreadItem } from './overlayThread'
import { applyThreadDelta, parseThreadDelta } from './threadDelta'

function row(seq: number, id = `m${seq}`, body = `m${seq}`): OverlayThreadItem {
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'sales', to: null, body, via: 'thread', choice: null, secret: null } }
}

describe('parseThreadDelta', () => {
  it('reads items, changed and removed from a thread-delta-v1 answer', () => {
    const d = parseThreadDelta({
      ok: true,
      conversation_id: 'conv-1',
      after_seq: 4,
      has_more: false,
      items: [row(6), row(5)],
      changed: [row(2, 'm2', 'edited')],
      removed: [{ id: 'm3', seq: 3 }, { id: 7 }],
    })
    if (!d) throw new Error('not read as a delta')
    expect(d.conversation_id).toBe('conv-1')
    expect(d.items.map((it) => it.seq)).toEqual([5, 6])
    expect(d.changed.map((it) => it.id)).toEqual(['m2'])
    expect(d.removed).toEqual([{ id: 'm3', seq: 3 }])
    expect(d.hasMore).toBe(false)
  })

  it('is null for an older daemon’s newest page (no after_seq echo, no changed)', () => {
    expect(parseThreadDelta({ conversation_id: 'c', has_more: false, items: [row(1)] })).toBeNull()
    expect(parseThreadDelta({ conversation_id: 'c', after_seq: 1, items: [] })).toBeNull()
    expect(parseThreadDelta(null)).toBeNull()
  })
})

describe('applyThreadDelta', () => {
  it('merges new rows and changed rows by id, drops removed ones', () => {
    const current = [row(1), row(2), row(3)]
    const next = applyThreadDelta(
      current,
      { items: [row(4)], changed: [row(2, 'm2', 'now a tombstone')], removed: [{ id: 'm3' }] },
      false,
    )
    expect(next.map((it) => it.id)).toEqual(['m1', 'm2', 'm4'])
    expect(next[1].doc.body).toBe('now a tombstone')
  })

  it('a repeat of what the list already holds changes nothing (same array)', () => {
    const current = [row(1), row(2)]
    expect(applyThreadDelta(current, { items: [row(2)], changed: [row(1)], removed: [] }, false)).toBe(current)
  })

  it('a changed row below the loaded window, while older pages remain, stays out', () => {
    const current = [row(10), row(11)]
    const next = applyThreadDelta(current, { items: [], changed: [row(3)], removed: [] }, true)
    expect(next).toBe(current)
    // With the whole Thread loaded there is no window below: it joins.
    expect(applyThreadDelta(current, { items: [], changed: [row(3)], removed: [] }, false).map((it) => it.seq)).toEqual([3, 10, 11])
  })
})
