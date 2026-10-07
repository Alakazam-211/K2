import { describe, expect, it } from 'vitest'
import {
  applyActivityCatchUp,
  applyActivityFrame,
  applyChatterFrame,
  applyOverlayFrame,
  coerceThreadTurn,
  isTurnLive,
  chatterItemsFromSnapshot,
  ingestOverlayThreadItem,
  isChatterSurfaceItem,
  isThreadSurfaceItem,
  mergeOlderOverlayItems,
  mergeThreadItems,
  overlayItemFromThreadPost,
  OVERLAY_PAGE_SIZE,
  overlaySeq,
  releaseOverlayWebSocket,
  subscribeOverlayThreadLive,
  threadItemsFromSnapshot,
  threadWindowFloor,
  type OverlayThreadItem,
} from './overlayThread'

const textDoc = {
  id: 'doc-1',
  kind: 'text',
  from: 'k2',
  body: 'hi',
}

const chatterDoc = {
  id: 'doc-c',
  kind: 'chatter',
  from: 'sales',
  to: 'sales/reviewer',
  body: 'ping',
  via: 'msg',
}

describe('overlay Thread list never includes chatter', () => {
  it('drops kind:chatter from a snapshot even if it were linked', () => {
    const snap = threadItemsFromSnapshot({
      conversation_id: 'conv-1',
      items: [
        { collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc },
        { collection: 'thread', seq: 2, id: 'doc-c', doc: chatterDoc },
        { collection: 'chatter', seq: 1, id: 'doc-c', doc: chatterDoc },
      ],
    })
    expect(snap.conversation_id).toBe('conv-1')
    expect(snap.has_more).toBe(false)
    expect(snap.items).toHaveLength(1)
    expect(snap.items[0].id).toBe('doc-1')
    expect(snap.items[0].doc.kind).toBe('text')
    expect(snap.items.some((i) => i.doc.kind === 'chatter')).toBe(false)
  })

  it('parses has_more from snapshot; missing defaults false', () => {
    expect(OVERLAY_PAGE_SIZE).toBe(25)
    const withFlag = threadItemsFromSnapshot({
      conversation_id: 'conv-1',
      has_more: true,
      items: [{ collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc }],
    })
    expect(withFlag.has_more).toBe(true)
    const missing = threadItemsFromSnapshot({
      conversation_id: 'conv-1',
      items: [{ collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc }],
    })
    expect(missing.has_more).toBe(false)
  })

  it('WS chatter frames do not appear on the Thread pane', () => {
    const start: OverlayThreadItem[] = [
      { collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc },
    ]
    const afterChatter = applyOverlayFrame(
      start,
      { collection: 'chatter', seq: 9, id: 'doc-c', doc: chatterDoc },
      1,
    )
    expect(afterChatter).toEqual(start)
    const afterThreadChatter = applyOverlayFrame(
      start,
      { collection: 'thread', seq: 2, id: 'doc-c', doc: chatterDoc },
      1,
    )
    expect(afterThreadChatter).toEqual(start)
    const afterText = applyOverlayFrame(
      start,
      { collection: 'thread', seq: 2, id: 'doc-2', doc: { id: 'doc-2', kind: 'text', from: 'k2', body: 'later' } },
      1,
    )
    expect(afterText).toHaveLength(2)
    expect(afterText[1].doc.body).toBe('later')
  })

  it('WS frame with an existing id replaces the doc in place (card answered)', () => {
    const start: OverlayThreadItem[] = [
      {
        collection: 'thread',
        seq: 2,
        id: 'choice-1',
        doc: {
          id: 'choice-1',
          kind: 'choice',
          from: 'k2',
          choice: {
            prompt: '?',
            options: [{ label: 'Go' }, { label: 'Stop' }],
            allow_custom: false,
            status: 'pending',
          },
        },
      },
    ]
    const after = applyOverlayFrame(
      start,
      {
        collection: 'thread',
        seq: 2,
        id: 'choice-1',
        doc: {
          id: 'choice-1',
          kind: 'choice',
          from: 'k2',
          choice: {
            prompt: '?',
            options: [{ label: 'Go' }, { label: 'Stop' }],
            allow_custom: false,
            status: 'answered',
            answer: 'Go',
          },
        },
      },
      2,
    )
    expect(after).toHaveLength(1)
    expect(after[0].doc.choice?.status).toBe('answered')
    expect(after[0].doc.choice?.answer).toBe('Go')
  })

  it('isThreadSurfaceItem rejects chatter', () => {
    expect(
      isThreadSurfaceItem({
        collection: 'thread',
        seq: 1,
        id: 'x',
        doc: chatterDoc,
      }),
    ).toBe(false)
  })
})

describe('overlay Chatter list never includes thread', () => {
  it('snapshot GET chatter items appear; thread rows are dropped', () => {
    const snap = chatterItemsFromSnapshot({
      conversation_id: 'conv-1',
      items: [
        { collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc },
        { collection: 'chatter', seq: 2, id: 'doc-c', doc: chatterDoc },
        { collection: 'chatter', seq: 1, id: 'doc-c2', doc: { ...chatterDoc, id: 'doc-c2', from: 'ops' } },
      ],
    })
    expect(snap.conversation_id).toBe('conv-1')
    expect(snap.has_more).toBe(false)
    expect(snap.items).toHaveLength(2)
    expect(snap.items.map((i) => i.id)).toEqual(['doc-c2', 'doc-c'])
    expect(snap.items.every((i) => i.collection === 'chatter')).toBe(true)
    expect(snap.items.some((i) => i.collection === 'thread')).toBe(false)
  })

  it('thread frames do not appear on the Chatter pane', () => {
    const start: OverlayThreadItem[] = [
      { collection: 'chatter', seq: 1, id: 'doc-c', doc: chatterDoc },
    ]
    const afterThread = applyChatterFrame(
      start,
      { collection: 'thread', seq: 9, id: 'doc-1', doc: textDoc },
      1,
    )
    expect(afterThread).toEqual(start)
  })

  it('chatter WS frames apply (append + upsert by id)', () => {
    const start: OverlayThreadItem[] = [
      { collection: 'chatter', seq: 1, id: 'doc-c', doc: chatterDoc },
    ]
    const afterNew = applyChatterFrame(
      start,
      {
        collection: 'chatter',
        seq: 2,
        id: 'doc-c3',
        doc: { id: 'doc-c3', kind: 'chatter', from: 'ops', to: 'sales', body: 'ack', via: 'talk' },
      },
      1,
    )
    expect(afterNew).toHaveLength(2)
    expect(afterNew[1].id).toBe('doc-c3')
    expect(afterNew[1].doc.body).toBe('ack')

    const afterUpsert = applyChatterFrame(
      afterNew,
      {
        collection: 'chatter',
        seq: 2,
        id: 'doc-c',
        doc: { ...chatterDoc, body: 'pong' },
      },
      1,
    )
    expect(afterUpsert).toHaveLength(2)
    expect(afterUpsert[0].doc.body).toBe('pong')
  })

  it('parses chatter has_more from snapshot', () => {
    const snap = chatterItemsFromSnapshot({
      conversation_id: 'conv-1',
      has_more: true,
      items: [{ collection: 'chatter', seq: 1, id: 'doc-c', doc: chatterDoc }],
    })
    expect(snap.has_more).toBe(true)
  })

  it('isChatterSurfaceItem accepts collection chatter', () => {
    expect(
      isChatterSurfaceItem({
        collection: 'chatter',
        seq: 1,
        id: 'x',
        doc: chatterDoc,
      }),
    ).toBe(true)
    expect(
      isChatterSurfaceItem({
        collection: 'thread',
        seq: 1,
        id: 'x',
        doc: textDoc,
      }),
    ).toBe(false)
  })
})

describe('mergeOlderOverlayItems', () => {
  it('prepends unique older items by id and keeps ascending seq', () => {
    const current: OverlayThreadItem[] = [
      { collection: 'thread', seq: 11, id: 't11', doc: { ...textDoc, id: 't11' } },
      { collection: 'thread', seq: 12, id: 't12', doc: { ...textDoc, id: 't12' } },
    ]
    const older: OverlayThreadItem[] = [
      { collection: 'thread', seq: 10, id: 't10', doc: { ...textDoc, id: 't10' } },
      { collection: 'thread', seq: 11, id: 't11', doc: { ...textDoc, id: 't11' } },
    ]
    const merged = mergeOlderOverlayItems(current, older)
    expect(merged.map((i) => i.id)).toEqual(['t10', 't11', 't12'])
    expect(merged.map((i) => i.seq)).toEqual([10, 11, 12])
  })
})

describe('compose thread/post ingest', () => {
  it('overlaySeq accepts numeric strings', () => {
    expect(overlaySeq(7)).toBe(7)
    expect(overlaySeq('8')).toBe(8)
    expect(Number.isFinite(overlaySeq(undefined))).toBe(false)
  })

  it('applyOverlayFrame accepts string seq from the wire', () => {
    const start: OverlayThreadItem[] = [
      { collection: 'thread', seq: 1, id: 'doc-1', doc: textDoc },
    ]
    const after = applyOverlayFrame(
      start,
      {
        collection: 'thread',
        seq: '2' as unknown as number,
        id: 'doc-2',
        doc: { id: 'doc-2', kind: 'text', from: 'k2', body: 'later' },
      },
      1,
    )
    expect(after).toHaveLength(2)
    expect(after[1].seq).toBe(2)
  })

  it('overlayItemFromThreadPost builds a thread row from POST JSON', () => {
    const item = overlayItemFromThreadPost(
      {
        ok: true,
        id: 'new-1',
        seq: 3,
        from: 'rosson',
        body: 'hello thread',
        kind: 'text',
        via: 'compose',
        conversation_id: 'conv-1',
      },
      'hello thread',
    )
    expect(item).not.toBeNull()
    expect(item?.collection).toBe('thread')
    expect(item?.id).toBe('new-1')
    expect(item?.seq).toBe(3)
    expect(item?.doc.body).toBe('hello thread')
    expect(item?.doc.via).toBe('compose')
  })

  it('ingestOverlayThreadItem notifies live subscribers', () => {
    const seen: OverlayThreadItem[] = []
    const unsub = subscribeOverlayThreadLive((item) => {
      seen.push(item)
    })
    const item = overlayItemFromThreadPost(
      { ok: true, id: 'live-1', seq: 1, from: 'k2', body: 'hi', conversation_id: 'c' },
      'hi',
    )
    expect(item).not.toBeNull()
    ingestOverlayThreadItem(item!)
    unsub()
    expect(seen).toHaveLength(1)
    expect(seen[0].id).toBe('live-1')
  })
})

describe('releaseOverlayWebSocket', () => {
  it('does not close a CONNECTING socket (defers to onopen)', () => {
    let closed = false
    const ws = {
      readyState: 0,
      close: () => {
        closed = true
      },
      onmessage: (() => {}) as ((ev: MessageEvent) => void) | null,
      onerror: (() => {}) as ((ev: Event) => void) | null,
      onopen: null as ((ev: Event) => void) | null,
    }
    releaseOverlayWebSocket(ws)
    expect(closed).toBe(false)
    expect(ws.onmessage).toBeNull()
    expect(typeof ws.onopen).toBe('function')
    ws.onopen?.({} as Event)
    expect(closed).toBe(true)
  })

  it('closes an OPEN socket immediately', () => {
    let closed = false
    const ws = {
      readyState: 1,
      close: () => {
        closed = true
      },
      onmessage: (() => {}) as ((ev: MessageEvent) => void) | null,
      onerror: null,
      onopen: null,
    }
    releaseOverlayWebSocket(ws)
    expect(closed).toBe(true)
  })
})

describe('Thread sync: merge by id (every source, no duplicates)', () => {
  const row = (seq: number, id: string, body = 'x', from = 'sales'): OverlayThreadItem => ({
    collection: 'thread',
    seq,
    id,
    doc: { id, kind: 'text', from, body },
  })

  it('mergeThreadItems adds new ids in seq order and never duplicates a known id', () => {
    const start = [row(1, 'a'), row(3, 'c')]
    const merged = mergeThreadItems(start, [row(2, 'b'), row(3, 'c'), row(4, 'd'), row(2, 'b')])
    expect(merged.map((it) => it.id)).toEqual(['a', 'b', 'c', 'd'])
  })

  it('mergeThreadItems takes the incoming doc for a known id (a card answered elsewhere)', () => {
    const start = [row(1, 'a', 'before')]
    const merged = mergeThreadItems(start, [row(1, 'a', 'after')])
    expect(merged).toHaveLength(1)
    expect(merged[0].doc.body).toBe('after')
  })

  it('mergeThreadItems returns the same list when nothing changed (no re-render)', () => {
    const start = [row(1, 'a'), row(2, 'b')]
    expect(mergeThreadItems(start, [row(2, 'b')])).toBe(start)
    expect(mergeThreadItems(start, [])).toBe(start)
  })

  it('mergeThreadItems never lets chatter into the Thread', () => {
    const start = [row(1, 'a')]
    const chatter: OverlayThreadItem = { collection: 'chatter', seq: 2, id: 'x', doc: chatterDoc }
    const chatterKind: OverlayThreadItem = { collection: 'thread', seq: 3, id: 'y', doc: chatterDoc }
    expect(mergeThreadItems(start, [chatter, chatterKind])).toBe(start)
  })

  it('a frame from another sender lands even when its seq is below one this list already has', () => {
    // This window's own send (seq 3) came back before the agent's reply
    // (seq 2) reached the socket: the reply is new, not a replay.
    const start = [row(1, 'a'), row(3, 'own', 'my send', 'rosson')]
    const after = applyOverlayFrame(
      start,
      { collection: 'thread', seq: 2, id: 'reply', doc: { id: 'reply', kind: 'text', from: 'sales', body: 'reply' } },
      threadWindowFloor(start, false),
    )
    expect(after.map((it) => it.id)).toEqual(['a', 'reply', 'own'])
  })

  it('threadWindowFloor: an older card changing below the loaded page waits for its page', () => {
    const page = [row(26, 'p26'), row(27, 'p27')]
    expect(threadWindowFloor(page, true)).toBe(26)
    expect(threadWindowFloor(page, false)).toBe(0)
    const frame = { collection: 'thread', seq: 4, id: 'old-card', doc: { id: 'old-card', kind: 'text', from: 'k2', body: 'x' } }
    expect(applyOverlayFrame(page, frame, threadWindowFloor(page, true))).toBe(page)
    expect(applyOverlayFrame(page, frame, threadWindowFloor(page, false)).map((it) => it.id)).toEqual([
      'old-card',
      'p26',
      'p27',
    ])
  })
})

// prd-daemon-activity-and-thread-working-v1 S7 (TW11): `activity` frames feed
// the Thread turn, never the items list.
describe('Thread turn: activity frames (S7)', () => {
  /** A §7.6 body as thread_activity.rs sends it. */
  function activity(over: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      turnId: 'turn-1',
      state: 'working',
      phase: 'tool',
      phaseSince: 1_000_060_000,
      line: 'Running `cargo test`',
      startedAt: 1_000_000_000,
      since: 1_000_000_000,
      subagents: 2,
      subagentsDone: 1,
      background: 0,
      tally: { read: 3, search: 0, cmd: 2, edit: 0 },
      end: null,
      serverNow: 1_000_072_000,
      rev: 4,
      ...over,
    }
  }
  const frameOf = (a: Record<string, unknown>) => ({ collection: 'activity', seq: 0, id: String(a.turnId), activity: a })

  it('T-S7c: applyOverlayFrame ignores an activity frame (items unchanged, same array)', () => {
    const items: OverlayThreadItem[] = [{ collection: 'thread', seq: 1, id: 'a', doc: textDoc }]
    expect(applyOverlayFrame(items, frameOf(activity()), 0)).toBe(items)
    expect(applyOverlayFrame([], frameOf(activity()), 0)).toEqual([])
    // Even one that (wrongly) carried a doc stays out of the list.
    expect(applyOverlayFrame(items, { ...frameOf(activity()), doc: textDoc }, 0)).toBe(items)
  })

  it('applyActivityFrame reads the §7.6 body and the serverNow skew', () => {
    const t = applyActivityFrame(null, frameOf(activity()), 1_000_070_000)
    expect(t).not.toBeNull()
    expect(t).toMatchObject({
      turnId: 'turn-1',
      state: 'working',
      phase: 'tool',
      line: 'Running `cargo test`',
      subagents: 2,
      subagentsDone: 1,
      tally: { read: 3, search: 0, cmd: 2, edit: 0 },
      end: null,
      rev: 4,
      skewMs: 2_000,
    })
    expect(isTurnLive(t)).toBe(true)
  })

  it('a thread frame leaves the turn as it is', () => {
    const t = applyActivityFrame(null, frameOf(activity()), 1_000_070_000)
    expect(applyActivityFrame(t, { collection: 'thread', seq: 2, id: 'x', doc: textDoc }, 1)).toBe(t)
  })

  it('the same turn moves only forward by rev; another turn replaces it', () => {
    const t4 = applyActivityFrame(null, frameOf(activity()), 0)
    const t3 = applyActivityFrame(t4, frameOf(activity({ rev: 3, phase: 'thinking', line: null })), 0)
    expect(t3).toBe(t4)
    const t5 = applyActivityFrame(t4, frameOf(activity({ rev: 5, phase: 'thinking', line: null })), 0)
    expect(t5?.phase).toBe('thinking')
    const other = applyActivityFrame(t5, frameOf(activity({ turnId: 'turn-2', rev: 1, phase: 'delivering' })), 0)
    expect(other?.turnId).toBe('turn-2')
  })

  it('an end frame ends the turn; a later rev of the same turn (Q7 resume) brings it back', () => {
    const live = applyActivityFrame(null, frameOf(activity()), 0)
    const ended = applyActivityFrame(live, frameOf(activity({ rev: 5, state: 'idle', end: { reason: 'reply', detail: null, at: 1_000_080_000 } })), 0)
    expect(isTurnLive(ended)).toBe(false)
    expect(ended?.end).toEqual({ reason: 'reply', detail: null, at: 1_000_080_000 })
    const resumed = applyActivityFrame(ended, frameOf(activity({ rev: 6 })), 0)
    expect(isTurnLive(resumed)).toBe(true)
  })

  it('a malformed body changes nothing', () => {
    const t = applyActivityFrame(null, frameOf(activity()), 0)
    expect(applyActivityFrame(t, { collection: 'activity', id: 'x', activity: { turnId: 'x' } }, 0)).toBe(t)
    expect(applyActivityFrame(t, { collection: 'activity', id: 'x' }, 0)).toBe(t)
    expect(coerceThreadTurn(activity({ state: 'bogus' }), 0)).toBeNull()
  })

  it('catch-up: {turn: null} clears, a body applies, a body without turn changes nothing', () => {
    const t = applyActivityFrame(null, frameOf(activity()), 0)
    expect(applyActivityCatchUp(t, { ok: true, turn: null }, 0)).toBeNull()
    expect(applyActivityCatchUp(null, { ok: true, turn: activity({ rev: 9 }) }, 1_000_072_000)?.rev).toBe(9)
    expect(applyActivityCatchUp(t, { ok: true }, 0)).toBe(t)
    expect(applyActivityCatchUp(t, null, 0)).toBe(t)
  })
})
