// @vitest-environment jsdom
//
// app-room-stream (0.46.3): the Garden side of `thread.inbox` turns the
// daemon's one stream into `{address, item}` for widgets: only
// `thread_message` frames with a doc, only rooms this window has a row for,
// never a room or conversation id; and the custom-widget bridge projects
// the item like a Thread view's (counted as a Thread subscription).

import { describe, expect, it } from 'vitest'
import { zenInboxMessage } from './zen-thread-inbox'
import { projectZenThreadItem } from './zen-custom-projection'

const rows: Record<string, string> = { 'room-a': 'sales::local', 'room-b': 'ops::local' }
const addressOf = (room: string): string | null => rows[room] ?? null

const frame = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
  kind: 'thread_message',
  conversation_id: 'conv-a',
  conversation: 'conv-a',
  room: 'room-a',
  collection: 'thread',
  seq: 7,
  id: 'doc-7',
  rev: 41,
  doc: { id: 'doc-7', kind: 'text', from: 'sales', to: 'owner', created_at: 1_700_000_000, body: 'hi', via: 'thread' },
  ...over,
})

describe('zenInboxMessage', () => {
  it('maps a message to the row address and keeps the item', () => {
    const m = zenInboxMessage(frame(), addressOf)
    expect(m).not.toBeNull()
    expect(m!.address).toBe('sales::local')
    expect(m!.item).toMatchObject({ collection: 'thread', seq: 7, id: 'doc-7' })
    expect(m!.item.doc.body).toBe('hi')
  })

  it('drops other kinds, truncated or doc-less frames and rooms with no row', () => {
    expect(zenInboxMessage({ kind: 'inbox_subscribed', rooms: ['room-a'] }, addressOf)).toBeNull()
    expect(zenInboxMessage({ kind: 'inbox_reset', reason: 'lagged' }, addressOf)).toBeNull()
    expect(zenInboxMessage(frame({ truncated: true }), addressOf)).toBeNull()
    expect(zenInboxMessage(frame({ doc: undefined }), addressOf)).toBeNull()
    expect(zenInboxMessage(frame({ room: 'room-z' }), addressOf)).toBeNull()
    expect(zenInboxMessage(frame({ seq: '7' }), addressOf)).toBeNull()
    expect(zenInboxMessage(null, addressOf)).toBeNull()
    expect(zenInboxMessage([frame()], addressOf)).toBeNull()
  })

  it('what a widget gets names no room, conversation or rev', () => {
    const m = zenInboxMessage(frame(), addressOf)!
    const pushed = { address: m.address, ...projectZenThreadItem(m.item)! }
    const wire = JSON.stringify(pushed)
    for (const banned of ['room-a', 'conv-a', 'conversation', 'rev', 'room']) {
      expect(wire).not.toContain(banned)
    }
    expect(pushed).toEqual({
      address: 'sales::local',
      seq: 7,
      id: 'doc-7',
      doc: { id: 'doc-7', kind: 'text', from: 'sales', to: 'owner', created_at: 1_700_000_000, body: 'hi', via: 'thread', choice: null },
    })
  })
})
