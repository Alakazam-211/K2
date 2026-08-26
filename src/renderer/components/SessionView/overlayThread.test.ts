import { describe, expect, it } from 'vitest'
import {
  applyOverlayFrame,
  isThreadSurfaceItem,
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
