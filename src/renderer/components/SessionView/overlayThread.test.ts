import { describe, expect, it } from 'vitest'
import {
  applyChatterFrame,
  applyOverlayFrame,
  chatterItemsFromSnapshot,
  isChatterSurfaceItem,
  isThreadSurfaceItem,
  releaseOverlayWebSocket,
  threadItemsFromSnapshot,
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
    expect(snap.items).toHaveLength(1)
    expect(snap.items[0].id).toBe('doc-1')
    expect(snap.items[0].doc.kind).toBe('text')
    expect(snap.items.some((i) => i.doc.kind === 'chatter')).toBe(false)
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
