// @vitest-environment jsdom
// Thread file objects must survive the trip from the daemon to the screen
// (0.46.1 RC smoke, Rosson 2026-10-18): Cortana's Thread on z3mbpZ showed
// seqs 59–62 without their chips and seq 63 (files only) as an empty
// bubble, although `GET /cli/thread` carried `attachments`. The page went
// through `threadItemsFromSnapshot` → `coerceItem`, which rebuilt the doc
// field by field and left `attachments` out. These tests feed the daemon's
// real item shape through the same mapping into the rendered Thread.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import {
  overlayItemFromThreadPost,
  threadItemsFromSnapshot,
  type OverlayThreadItem,
  type ThreadTurn,
} from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { ThreadBubble } from '@/components/Thread/ThreadZenKit'

const room = testRoom({ tabs: {}, scope: primaryScope() })

const threadHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loaded: true,
  loadOlder: async () => {},
  turn: null as ThreadTurn | null,
  turnsReported: true,
}))

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

/** One file object as `GET /cli/thread` sends it (`ThreadAttachment`
 *  serialized camelCase + the per-read `state`). */
function wireFile(path: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    kind: 'file',
    path,
    name: path.split('/').pop(),
    size: 1234,
    mime: 'text/plain',
    modifiedAt: 1_760_000_000_000,
    sha256: 'ab'.repeat(32),
    state: 'ok',
    ...extra,
  }
}

/** The `GET /cli/thread?addr=cortana&limit=10` body, trimmed to the items
 *  Rosson saw: text + files (59), and a files-only post (63). */
const WIRE = {
  ok: true,
  collection: 'thread',
  addr: 'cortana',
  conversation_id: 'conv-1',
  has_more: false,
  pastAddresses: [],
  items: [
    {
      collection: 'thread',
      seq: 59,
      id: 'm59',
      conversation_id: 'conv-1',
      doc: {
        id: 'm59',
        kind: 'text',
        from: 'cortana',
        to: 'owner',
        created_at: 1_760_000_000,
        body: 'Here are the two files.',
        attachments: [wireFile('docs/a.md', { line: 4 }), wireFile('src/b.ts')],
      },
    },
    {
      collection: 'thread',
      seq: 63,
      id: 'm63',
      conversation_id: 'conv-1',
      doc: {
        id: 'm63',
        kind: 'text',
        from: 'cortana',
        to: 'owner',
        created_at: 1_760_000_100,
        body: '',
        attachments: [wireFile('notes/x.txt'), wireFile('notes/y.txt'), wireFile('notes/z.txt')],
      },
    },
  ],
}

beforeEach(() => {
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
})
afterEach(() => {
  cleanup()
  threadHook.items = []
  resetThreadViewersForTests()
})

describe('Thread file objects survive the load', () => {
  it('a /cli/thread page keeps each doc\'s attachments, with every field the chips read', () => {
    const snap = threadItemsFromSnapshot(WIRE)
    expect(snap.items.map((it) => it.seq)).toEqual([59, 63])
    const files59 = snap.items[0].doc.attachments
    expect(files59).toEqual([
      {
        kind: 'file',
        path: 'docs/a.md',
        name: 'a.md',
        size: 1234,
        mime: 'text/plain',
        modifiedAt: 1_760_000_000_000,
        sha256: 'ab'.repeat(32),
        state: 'ok',
        line: 4,
      },
      {
        kind: 'file',
        path: 'src/b.ts',
        name: 'b.ts',
        size: 1234,
        mime: 'text/plain',
        modifiedAt: 1_760_000_000_000,
        sha256: 'ab'.repeat(32),
        state: 'ok',
      },
    ])
    expect(snap.items[1].doc.attachments?.map((f) => f.path)).toEqual(['notes/x.txt', 'notes/y.txt', 'notes/z.txt'])
  })

  it('a hidden file object (an App guest) keeps only its kind and state', () => {
    const snap = threadItemsFromSnapshot({
      items: [{ collection: 'thread', seq: 1, id: 'h', doc: { id: 'h', kind: 'text', from: 'a', body: 'x', attachments: [{ kind: 'file', state: 'hidden' }] } }],
    })
    expect(snap.items[0].doc.attachments).toEqual([{ kind: 'file', state: 'hidden' }])
  })

  it('a doc without attachments stays without the key (equal to the socket\'s doc)', () => {
    const snap = threadItemsFromSnapshot({
      items: [{ collection: 'thread', seq: 1, id: 'p', doc: { id: 'p', kind: 'text', from: 'a', body: 'plain' } }],
    })
    expect('attachments' in snap.items[0].doc).toBe(false)
  })

  it('this window\'s own post keeps the attachments the post answer carries', () => {
    const item = overlayItemFromThreadPost(
      { ok: true, id: 'mine', seq: 70, kind: 'text', from: 'owner', body: 'see', attachments: [wireFile('src/b.ts')] },
      'see',
    )
    expect(item?.doc.attachments?.map((f) => f.path)).toEqual(['src/b.ts'])
  })

  it('a loaded Thread draws the chips under the text, and a files-only post has no empty bubble', () => {
    threadHook.items = threadItemsFromSnapshot(WIRE).items
    const { container } = renderInRoom(room, <ThreadOverlayPane addr="cortana" conversationId="conv-1" />)
    const rows = [...container.querySelectorAll('[data-testid="thread-item"]')]
    expect(rows.map((r) => r.getAttribute('data-seq'))).toEqual(['59', '63'])

    const [m59, m63] = rows
    expect(m59.querySelector('[data-zen-bubble]')?.textContent).toContain('Here are the two files.')
    expect(
      [...m59.querySelectorAll('[data-thread-files] [data-thread-file]')].map((el) => el.getAttribute('data-thread-file')),
    ).toEqual(['docs/a.md', 'src/b.ts'])

    expect(m63.querySelector('[data-zen-bubble]')).toBeNull()
    expect(m63.querySelector('[data-thread-file-list]')).not.toBeNull()
    expect(
      [...m63.querySelectorAll('[data-thread-file]')].map((el) => el.getAttribute('data-thread-file')),
    ).toEqual(['notes/x.txt', 'notes/y.txt', 'notes/z.txt'])
  })

  it('the Zen Garden Thread widget\'s bubble (same ThreadBubble) draws the loaded chips too', () => {
    const [m59] = threadItemsFromSnapshot(WIRE).items
    const { container } = render(
      <ThreadBubble item={m59} mine={false} author="cortana" timeLabel="8m ago" viewer={null} />,
    )
    expect(container.querySelectorAll('[data-thread-files] [data-thread-file]').length).toBe(2)
  })
})
