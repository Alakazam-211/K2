// @vitest-environment jsdom
// Where a message's details go (Rosson 2026-11-09, from K2 Companion):
// another sender's "name • time" over the first bubble of their run; under
// the last of yours one status line (the time, Sending…, Not sent, or the
// Thread's "Delivered" / "Read"); nothing under the bubbles between. The
// sender pictures only with two or more other senders among the drawn
// rows, one per run, beside its last bubble's tail. Thread view and Split
// draw the same pane, so one test covers both.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ThreadOverlayPane, threadPanePictures, threadPaneReceipt, threadPaneRuns } from '@/components/SessionView/ThreadOverlayPane'
import { coerceThreadTurn, threadItemsFromSnapshot, type OverlayThreadItem, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { OWNER_VIEWER, resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { threadRunStarts, threadRunWindow, type ThreadRunInput } from './threadRuns'
import { readByTurn, threadReceiptFor, threadReceiptRow, type ThreadReceiptInput } from './threadReceipt'
import { ThreadBubble } from './ThreadZenKit'
import { threadChatItem } from './ThreadChat'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'

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
  outbox: [] as OverlayThreadItem[],
  currentAddr: '',
  pastAddresses: [] as string[],
}))

vi.mock('@/components/SessionView/useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

const T0 = 1_762_700_000

/** Mine: the owner's compose post (the daemon's stamp). */
function me(id: string, at: number): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 0,
    id,
    doc: { id, kind: 'text', from: 'Rosson', via: 'compose', author: { kind: 'owner', login: 'owner' }, created_at: at, body: `me ${id}` },
  }
}

function agent(id: string, at: number, from = 'sales'): OverlayThreadItem {
  return { collection: 'thread', seq: 0, id, doc: { id, kind: 'text', from, created_at: at, body: `${from} ${id}` } }
}

function person(id: string, at: number, login: string): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 0,
    id,
    doc: { id, kind: 'text', from: login, via: 'compose', author: { kind: 'user', login }, created_at: at, body: `${login} ${id}` },
  }
}

function pending(id: string, at: number, state: 'sending' | 'failed'): OverlayThreadItem {
  return { ...me(id, at), pending: { clientMsgId: id, state } }
}

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-zen-message="${id}"]`)
  if (!el) throw new Error(`row ${id} is not drawn`)
  return el
}

const nameLine = (id: string) => row(id).querySelector<HTMLElement>('[data-thread-meta="name"]')
const statusLine = (id: string) => row(id).querySelector<HTMLElement>('[data-thread-meta="status"]')
const bubbleOf = (id: string) => row(id).querySelector<HTMLElement>('[data-zen-bubble]')!
const before = (a: Element, b: Element) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0

function pane(items: OverlayThreadItem[], outbox: OverlayThreadItem[] = []): void {
  threadHook.items = items
  threadHook.outbox = outbox
  renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
}

beforeEach(() => {
  resetThreadViewersForTests()
  setThreadViewerForTests(room.scope, OWNER_VIEWER)
  threadHook.turn = null
  threadHook.outbox = []
})

afterEach(() => {
  cleanup()
  resetThreadViewersForTests()
})

describe('runs: where a run starts', () => {
  const mine = (at: number | null): ThreadRunInput => ({ mine: true, from: 'person:owner:owner', at })
  const them = (from: string, at: number | null): ThreadRunInput => ({ mine: false, from, at })

  it('a new side, a new sender or a gap over 120s starts one', () => {
    const rows = [them('agent:a', T0), them('agent:a', T0 + 30), mine(T0 + 40), mine(T0 + 50), them('agent:b', T0 + 60), them('agent:b', T0 + 300)]
    expect(threadRunStarts(rows)).toEqual([true, false, true, false, true, true])
  })

  it('the row before a render window keeps a run coming in from above', () => {
    expect(threadRunStarts([mine(T0 + 10)], mine(T0))).toEqual([false])
    expect(threadRunStarts([mine(T0 + 10)], them('agent:a', T0))).toEqual([true])
    const all = [mine(T0), mine(T0 + 1), mine(T0 + 2), them('agent:a', T0 + 3)]
    expect(threadRunWindow(all, 1, 3)).toEqual({ starts: [false, false], ends: [false, true] })
  })
})

describe('the receipt (Delivered / Read)', () => {
  const r = (mine: boolean, confirmed = true, agentPost = !mine, at: number | null = T0): ThreadReceiptInput => ({ mine, confirmed, agent: agentPost, at })

  it('sits on your newest confirmed message, never a pending one', () => {
    expect(threadReceiptRow([r(true), r(false), r(true), r(true, false)])).toBe(2)
    expect(threadReceiptRow([r(false)])).toBe(-1)
  })

  it('Read once an agent posts after it; another person posting is not the agent reading', () => {
    expect(threadReceiptFor([r(true), r(false)], 0)).toBe('read')
    expect(threadReceiptFor([r(true), r(false, true, false)], 0)).toBe('delivered')
    expect(threadReceiptFor([r(true)], 0)).toBe('delivered')
    expect(threadReceiptFor([r(true, false)], 0)).toBeNull()
  })

  it('Read once the agent’s turn picked it up (started at or after it, past delivering)', () => {
    const turn = (startedAt: number, phase: ThreadTurn['phase'], end: ThreadTurn['end'] = null) => ({ startedAt, phase, end })
    expect(readByTurn(T0, turn(T0 * 1000 + 500, 'working'))).toBe(true)
    expect(readByTurn(T0, turn(T0 * 1000 - 1500, 'working'))).toBe(true) // clock slack
    expect(readByTurn(T0, turn(T0 * 1000 - 60_000, 'working'))).toBe(false) // an older turn
    expect(readByTurn(T0, turn(T0 * 1000 + 500, 'delivering'))).toBe(false)
    expect(readByTurn(T0, turn(T0 * 1000 + 500, 'delivering', { reason: 'delivery_failed', detail: null, at: 0 }))).toBe(false)
    expect(threadReceiptFor([r(true)], 0, turn(T0 * 1000 + 500, 'tool'))).toBe('read')
  })
})

describe('the Thread pane: names above others’ runs, your status line below yours', () => {
  it('another sender’s name and time sit over the first bubble of their run, and nowhere else', () => {
    pane([agent('a1', T0), agent('a2', T0 + 10), me('m1', T0 + 20), agent('a3', T0 + 30)])
    const name = nameLine('a1')!
    expect(name).not.toBeNull()
    expect(name.querySelector('[data-thread-author]')?.textContent).toBe('sales')
    expect(name.querySelector('[data-thread-time]')?.textContent).toBeTruthy()
    expect(name.textContent).toContain('•')
    expect(before(name, bubbleOf('a1'))).toBe(true)
    // The rest of the run: no line over or under.
    expect(row('a2').querySelector('[data-zen-message-meta]')).toBeNull()
    expect(row('a2').getAttribute('data-run')).toBe('last')
    // A new run (after yours) gets its own.
    expect(nameLine('a3')).not.toBeNull()
    // Yours never carries a name.
    expect(row('m1').querySelector('[data-thread-author]')).toBeNull()
  })

  it('your details sit under the last bubble of your run, once', () => {
    pane([agent('a1', T0), me('m1', T0 + 10), me('m2', T0 + 20), me('m3', T0 + 30)])
    expect(row('m1').querySelector('[data-zen-message-meta]')).toBeNull()
    expect(row('m2').querySelector('[data-zen-message-meta]')).toBeNull()
    const status = statusLine('m3')!
    expect(status).not.toBeNull()
    expect(before(bubbleOf('m3'), status)).toBe(true)
    // The newest confirmed, nothing after it: Delivered, in place of the time
    // (the time stays on hover).
    expect(status.getAttribute('data-receipt')).toBe('delivered')
    expect(status.querySelector('[data-thread-receipt]')?.textContent).toBe('Delivered')
    expect(status.querySelector('[data-thread-receipt]')?.getAttribute('title')).toBeTruthy()
    expect(status.querySelector('[data-thread-time]')).toBeNull()
    // A run's bubbles sit closer together than two runs do.
    expect(row('m2').style.marginTop).toBe('-6px')
    expect(row('m1').style.marginTop).toBe('')
  })

  it('an agent’s reply makes it Read; an older run of yours keeps its time', () => {
    pane([me('m1', T0), agent('a1', T0 + 10), me('m2', T0 + 20), agent('a2', T0 + 30)])
    expect(statusLine('m1')?.querySelector('[data-thread-time]')).not.toBeNull()
    expect(statusLine('m1')?.hasAttribute('data-receipt')).toBe(false)
    expect(statusLine('m2')?.getAttribute('data-receipt')).toBe('read')
    expect(statusLine('m2')?.textContent).toBe('Read')
  })

  it('the agent’s turn picking it up makes it Read', () => {
    const at = (T0 + 21) * 1000
    threadHook.turn = coerceThreadTurn(
      {
        turnId: 't1',
        state: 'working',
        phase: 'tool',
        phaseSince: at,
        line: 'Reading',
        startedAt: at,
        since: at,
        subagents: 0,
        subagentsDone: 0,
        background: 0,
        tally: { read: 1, search: 0, cmd: 0, edit: 0 },
        end: null,
        serverNow: at + 1000,
        rev: 1,
      },
      at + 1000,
    )
    expect(threadHook.turn).not.toBeNull()
    pane([agent('a1', T0), me('m1', T0 + 20)])
    expect(statusLine('m1')?.getAttribute('data-receipt')).toBe('read')
  })

  it('a pending send says Sending… under itself; the receipt waits on the one before', () => {
    pane([agent('a1', T0), me('m1', T0 + 10)], [pending('p1', T0 + 20, 'sending')])
    expect(statusLine('p1')?.textContent).toBe('Sending…')
    expect(statusLine('p1')?.getAttribute('data-send-status')).toBe('sending')
    expect(statusLine('p1')?.hasAttribute('data-receipt')).toBe(false)
    // m1 is mid-run now: no line, so no receipt shows (as the phone).
    expect(statusLine('m1')).toBeNull()
  })

  it('a failed send: Not sent, with Retry and Discard on the same line', () => {
    pane([agent('a1', T0)], [pending('p1', T0 + 20, 'failed')])
    const status = statusLine('p1')!
    expect(status.querySelector('[data-thread-time]')?.textContent).toBe('Not sent')
    expect(status.querySelector('[data-thread-retry]')).not.toBeNull()
    expect(status.querySelector('[data-thread-discard]')).not.toBeNull()
  })
})

describe('sender pictures: two or more other senders among the loaded messages', () => {
  it('one other sender: no pictures', () => {
    pane([agent('a1', T0), me('m1', T0 + 10), agent('a2', T0 + 20)])
    expect(document.querySelector('[data-sender-column="on"]')).toBeNull()
    expect(threadPanePictures(threadHook.items, { viewer: OWNER_VIEWER })).toBe(false)
  })

  it('a second agent or another person: a picture at the bottom left of each run’s last bubble, level with its tail; every bubble keeps the gutter', () => {
    pane([agent('a1', T0), agent('a2', T0 + 10), agent('b1', T0 + 20, 'ops'), me('m1', T0 + 30), person('p1', T0 + 40, 'ada')])
    expect(row('a1').getAttribute('data-sender-column')).toBe('on')
    // Not on the first of the run, where the name line is.
    expect(row('a1').querySelector('[data-sender-avatar]')).toBeNull()
    expect(nameLine('a1')).not.toBeNull()
    const pic = row('a2').querySelector<HTMLElement>('[data-sender-avatar]')!
    // On the last: beside the bubble (its row), at its bottom, so it sits
    // level with the tail and never below files or chips under the bubble.
    expect(pic.parentElement).toBe(bubbleOf('a2').closest('[data-zen-bubble-row]'))
    expect(pic.style.bottom).toBe('0px')
    expect(pic.style.top).toBe('')
    // The rest of the run: no picture, the same gutter, so the run lines up.
    expect(row('a2').style.marginLeft).toBe(row('a1').style.marginLeft)
    expect(row('a1').style.marginLeft).not.toBe('0px')
    // A run of one draws it on its only bubble.
    expect(row('b1').querySelector('[data-sender-avatar]')).not.toBeNull()
    expect(row('p1').querySelector('[data-sender-avatar]')).not.toBeNull()
    expect(row('m1').querySelector('[data-sender-avatar]')).toBeNull()
  })

  it('a gap, another sender, a deleted message or one of K2’s notices ends a run, so the bubble before it draws the picture', () => {
    const notice: OverlayThreadItem = {
      collection: 'thread',
      seq: 0,
      id: 'n1',
      doc: { id: 'n1', kind: 'text', from: 'k2', author: { kind: 'system', login: 'k2' }, created_at: T0 + 640, body: 'K2: a notice', systemNotice: { kind: 'info', code: 'x' } },
    }
    const gone: OverlayThreadItem = { ...agent('a5', T0 + 660), doc: { ...agent('a5', T0 + 660).doc, deleted: true } }
    const items = [
      agent('a1', T0),
      agent('a2', T0 + 600),
      agent('a3', T0 + 610),
      agent('b1', T0 + 620, 'ops'),
      agent('a4', T0 + 630),
      notice,
      agent('a6', T0 + 650),
      gone,
      agent('a7', T0 + 670),
    ]
    const runs = threadPaneRuns(items, { lo: 0, hi: items.length, viewer: OWNER_VIEWER, motion: false, host: null })
    expect(runs.senders.on).toBe(true)
    const drawn = [...runs.senders.slots.entries()].filter(([, s]) => s.last).map(([id]) => id)
    expect(drawn).toEqual(['a1', 'a3', 'b1', 'a4', 'a6', 'a7'])
  })

  it('a new message from the same sender takes the picture down to it', () => {
    pane([agent('a1', T0), agent('b1', T0 + 5, 'ops'), agent('a2', T0 + 10), agent('a3', T0 + 20)])
    expect(row('a3').querySelector('[data-sender-avatar]')).not.toBeNull()
    expect(row('a2').querySelector('[data-sender-avatar]')).toBeNull()
    cleanup()
    pane([agent('a1', T0), agent('b1', T0 + 5, 'ops'), agent('a2', T0 + 10), agent('a3', T0 + 20), agent('a4', T0 + 30)])
    expect(row('a3').querySelector('[data-sender-avatar]')).toBeNull()
    expect(row('a4').querySelector('[data-sender-avatar]')).not.toBeNull()
    expect(document.querySelectorAll('[data-sender-avatar]').length).toBe(3)
  })

  it('the receipt reads every message in memory, not just the drawn ones', () => {
    const items = [me('m1', T0), agent('a1', T0 + 10)]
    expect(threadPaneReceipt(items, { viewer: OWNER_VIEWER })).toEqual({ id: 'm1', state: 'read' })
    expect(threadPaneReceipt([agent('a1', T0)], { viewer: OWNER_VIEWER })).toBeNull()
  })
})

describe('the bubble without runs (Gardens) is as before', () => {
  it('"author · time" under every bubble, the menu button in it', () => {
    const { container } = render(
      <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'sales' }}>
        <ThreadBubble item={threadChatItem({ id: 'g1', author: 'k2', body: 'hi', createdAt: T0 })} mine={false} author="k2" timeLabel="now" viewer={null} />
      </ThreadAttachmentSourceProvider>,
    )
    const meta = container.querySelector('[data-zen-message-meta]')!
    expect(meta.textContent).toBe('k2 · now')
    expect(before(container.querySelector('[data-zen-bubble]')!, meta)).toBe(true)
    expect(meta.querySelector('[data-zen-message-more]')).not.toBeNull()
    expect(container.querySelector('[data-run]')).toBeNull()
  })

  it('with runs, another sender’s menu button sits beside the bubble (no line under each to hold it)', () => {
    const { container } = render(
      <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'sales' }}>
        <ThreadBubble item={threadChatItem({ id: 'g2', author: 'k2', body: 'hi', createdAt: T0 })} mine={false} author="k2" timeLabel="now" viewer={null} runStart={false} tail={false} />
      </ThreadAttachmentSourceProvider>,
    )
    expect(container.querySelector('[data-zen-message-meta]')).toBeNull()
    const slot = container.querySelector('[data-zen-message-more-slot]')!
    expect(slot.querySelector('[data-zen-message-more]')).not.toBeNull()
    expect(slot.parentElement?.hasAttribute('data-zen-bubble-row')).toBe(true)
  })
})

describe('the ⋯ menu on your own messages (Rosson 2026-11-09)', () => {
  it('sits at the status line’s inner end, before the time or receipt, out of the flow', () => {
    pane([agent('a1', T0), me('m1', T0 + 10)])
    const status = statusLine('m1')!
    const slot = status.querySelector<HTMLElement>('[data-zen-message-more-slot]')!
    expect(slot.querySelector('[data-zen-message-more]')).not.toBeNull()
    expect(before(slot, status.querySelector('[data-thread-receipt]')!)).toBe(true)
    expect(slot.style.position).toBe('absolute')
    // Not beside the bubble.
    expect(row('m1').querySelector('[data-zen-bubble-row]')).toBeNull()
    expect(row('m1').querySelectorAll('[data-zen-message-more]')).toHaveLength(1)
  })

  it('a bubble inside your run shows a status line with its time and the ⋯ on hover, laid over the space below', () => {
    pane([agent('a1', T0), me('m1', T0 + 10), me('m2', T0 + 20)])
    expect(statusLine('m1')).toBeNull()
    const hover = row('m1').querySelector<HTMLElement>('[data-thread-hover-status]')!
    expect(hover).not.toBeNull()
    expect(hover.querySelector('[data-thread-time]')?.textContent).toBeTruthy()
    const slot = hover.querySelector('[data-zen-message-more-slot]')!
    expect(before(slot, hover.querySelector('[data-thread-time]')!)).toBe(true)
    // Out of the flow (nothing below moves), hidden until hover or focus.
    expect(hover.style.position).toBe('absolute')
    expect(row('m1').style.position).toBe('relative')
    const css = document.querySelector('style[data-thread-zen-styles]')?.textContent ?? ''
    expect(css).toContain('[data-thread-hover-status] { opacity: 0; pointer-events: none;')
    expect(css).toContain('[data-zen-message]:hover [data-thread-hover-status]')
    expect(css).toContain('[data-thread-hover-status]:focus-within')
    // The keyboard reaches it; the last bubble has its own line, no hover one.
    expect(hover.querySelector('button[data-zen-message-more]')).not.toBeNull()
    expect(row('m2').querySelector('[data-thread-hover-status]')).toBeNull()
  })

  it('others’ messages keep the ⋯ beside the bubble, and no hover line', () => {
    pane([agent('a1', T0), agent('a2', T0 + 10)])
    expect(row('a1').querySelector('[data-zen-bubble-row] [data-zen-message-more-slot] [data-zen-message-more]')).not.toBeNull()
    expect(row('a1').querySelector('[data-thread-hover-status]')).toBeNull()
  })
})

describe('the chats that wear the Thread (project chat, a ticket)', () => {
  it('take the runs in the app, never inside a Garden', async () => {
    const { ThreadChatBubble, ThreadChatZenContext } = await import('./ThreadChat')
    const bubble = (id: string) => (
      <ThreadChatBubble id={id} author="ada" body="hi" createdAt={T0} mine={false} timeLabel="now" runStart tail />
    )
    const app = render(bubble('c1'))
    expect(app.container.querySelector('[data-thread-meta="name"]')).not.toBeNull()
    cleanup()
    const zen = render(<ThreadChatZenContext.Provider value={true}>{bubble('c2')}</ThreadChatZenContext.Provider>)
    expect(zen.container.querySelector('[data-thread-meta]')).toBeNull()
    expect(zen.container.querySelector('[data-zen-message-meta]')?.textContent).toBe('ada · now')
  })
})

// A deleted message after coming back to the Thread (thread-message-delete-v1):
// the Thread is read again from the server (or painted from this computer's
// copy first). The tombstone draws as one quiet "Message deleted" line in
// its place: no bubble, no status line, no receipt; the receipt moves to the
// newest message of yours that still stands.
describe('a deleted message after returning to the Thread', () => {
  /** The daemon's tombstone for `me(id)`, as `GET /cli/thread` serves it. */
  function servedTombstone(id: string, seq: number, at: number): unknown {
    return {
      collection: 'thread',
      seq,
      id,
      doc: {
        id,
        kind: 'text',
        from: 'Rosson',
        via: 'compose',
        author: { kind: 'owner', login: 'owner' },
        created_at: at,
        deleted: true,
        deletedAt: at + 30,
        deletedBy: { kind: 'owner', login: 'owner' },
      },
    }
  }

  function returning(rows: unknown[]): OverlayThreadItem[] {
    return threadItemsFromSnapshot({ conversation_id: 'c', items: rows }).items
  }

  const seqd = (it: OverlayThreadItem, seq: number): OverlayThreadItem => ({ ...it, seq })

  it('draws one muted "Message deleted" line, never an empty bubble with Read under it', () => {
    pane(returning([seqd(agent('a1', T0), 1), seqd(me('m1', T0 + 10), 2), servedTombstone('m2', 3, T0 + 20), seqd(agent('a2', T0 + 40), 4)]))
    const gone = row('m2')
    expect(gone.querySelector('[data-zen-bubble]')).toBeNull()
    const line = gone.querySelector<HTMLElement>('[data-thread-deleted]')!
    expect(line).not.toBeNull()
    expect(line.textContent).toBe('Message deleted')
    expect(gone.textContent).toBe('Message deleted')
    // A quiet line, not a bubble: no outline, no fill.
    expect(line.style.border).toBe('')
    expect(line.style.background).toBe('')
    expect(gone.querySelector('[data-thread-meta]')).toBeNull()
    expect(gone.querySelector('[data-thread-receipt]')).toBeNull()
    expect(gone.querySelector('[data-zen-message-more]')).toBeNull()
    // The receipt is on the newest message of yours that stands.
    expect(statusLine('m1')?.getAttribute('data-receipt')).toBe('read')
  })

  it('the receipt never sits on a deleted message, even the newest of yours', () => {
    const items = returning([seqd(me('m1', T0), 1), servedTombstone('m2', 2, T0 + 10)])
    expect(threadPaneReceipt(items, { viewer: OWNER_VIEWER })).toEqual({ id: 'm1', state: 'delivered' })
    expect(threadPaneReceipt(returning([servedTombstone('m2', 1, T0)]), { viewer: OWNER_VIEWER })).toBeNull()
  })

  it('a deleted message ends the run before it, so that message keeps its status line', () => {
    pane(returning([seqd(me('m1', T0), 1), servedTombstone('m2', 2, T0 + 10), seqd(me('m3', T0 + 20), 3)]))
    expect(statusLine('m1')).not.toBeNull()
    expect(row('m2').querySelector('[data-thread-meta]')).toBeNull()
    expect(statusLine('m3')?.getAttribute('data-receipt')).toBe('delivered')
  })

  it('draws the same from this computer’s saved copy (the items as stored, no read)', () => {
    const stored = JSON.parse(JSON.stringify(returning([seqd(me('m1', T0), 1), servedTombstone('m2', 2, T0 + 10)]))) as OverlayThreadItem[]
    pane(stored)
    expect(row('m2').textContent).toBe('Message deleted')
    expect(row('m2').querySelector('[data-zen-bubble]')).toBeNull()
    expect(row('m2').querySelector('[data-thread-receipt]')).toBeNull()
  })

  it('reads "Message deleted" with no menu or meta line, wherever it is drawn', () => {
    const tomb = returning([servedTombstone('m9', 9, T0)])[0]
    const { container } = render(
      <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'sales' }}>
        <ThreadBubble item={tomb} mine author="You" timeLabel="now" viewer={null} />
      </ThreadAttachmentSourceProvider>,
    )
    expect(container.textContent).toBe('Message deleted')
    expect(container.querySelector('[data-zen-message-meta]')).toBeNull()
  })
})
