// @vitest-environment jsdom
// prd-thread-message-menu-v1 (Rosson 2026-10-30): a Thread message's menu.
//
//   1. The reply format the agent reads: `↩ #N <name>:`, then a whole
//      message (in full when short, its first 600 characters and "…" when
//      long) or a selection (`↩ part of #N`, exactly the selection); it
//      reads back for the sent bubble, and so does the first build's long
//      header. The agent reads message N with `k2 thread <addr> --seq N`
//      (tests/cli/thread_seq.sh).
//   2. The menu opens on right-click and on a long press (touch); a press
//      that moves is a scroll.
//   3. Reply: the chip shows above the desktop Thread's box, × and Esc
//      cancel it, Send posts the agreed text and clears it.
//   4. "Reply to selection" only for a selection inside one message.
//   5. The Zen conversation's box takes a Reply the same way.
// Stash from the menu: PromptStash/stashFromThread.test.tsx. Fail loud.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    if (route === 'tunnel/status') return { public_url: 'https://rosson.k2.dev' }
    return []
  }),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { ThreadBubble } from './ThreadZenKit'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { THREAD_LONG_PRESS_MS } from './ThreadMessageMenu'
import { ZenCompose } from '@/components/Zen/widgets/ZenCompose'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import {
  THREAD_REPLY_QUOTE_MAX,
  formatThreadReply,
  parseThreadReply,
  resetThreadReplyForTests,
  threadReplyKey,
  type ThreadReplyTarget,
} from '@/lib/thread-reply'
import { useContextMenuStore } from '@/stores/context-menu'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const WS = '/Users/me/ai/sales'
const ID = '3f2a9c1e-5b7d-4e2a-9c1e-5b7d4e2a9c1e'
const OWNER = { principal: 'owner', aliases: [] as string[] }

const scope: ServerScope = fakeScope('local', { remote: false })

const chrome = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function item(seq: number, body: string, doc: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  const id = seq === 42 ? ID : `m${seq}`
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'sales', body, created_at: 1, ...doc } }
}

/** The desktop Thread: bubbles (with the Thread's server and address) and
 *  the Thread's message box, as TerminalPane puts them side by side. */
function renderThread(items: OverlayThreadItem[], opts: { box?: boolean } = {}): void {
  const room = testRoom({
    tabs: {},
    scope,
    isPrimary: true,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <ThreadAttachmentSourceProvider value={{ scope, addr: 'sales' }}>
        {items.map((it) => (
          <ThreadBubble key={it.id} item={it} mine={false} author="sales" timeLabel="now" viewer={OWNER} />
        ))}
      </ThreadAttachmentSourceProvider>
      {opts.box === false ? null : <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination="thread" />}
    </SessionViewChromeContext.Provider>,
  )
}

function bubbles(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-zen-bubble]'))
}

function menuLabels(): string[] {
  return useContextMenuStore
    .getState()
    .items.filter((i) => i.type !== 'separator')
    .map((i) => i.label)
}

/** Select `from`..`to` of the first text node holding `text`. */
function selectIn(el: HTMLElement, text: string, from: number, to: number): void {
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT)
  let node = walker.nextNode()
  while (node && !(node.textContent ?? '').includes(text)) node = walker.nextNode()
  if (!node) throw new Error(`no text node with ${text}`)
  const range = document.createRange()
  range.setStart(node, from)
  range.setEnd(node, to)
  const sel = window.getSelection()!
  sel.removeAllRanges()
  sel.addRange(range)
}

function threadPosts(): Array<{ text: string }> {
  return h.daemonCliPost.mock.calls.filter((c) => c[1] === 'thread/post').map((c) => c[2] as { text: string })
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  resetThreadReplyForTests()
  useContextMenuStore.getState().close()
  window.getSelection()?.removeAllRanges()
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd9', seq: 99, kind: 'text', from: 'owner', body: '' })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('the reply format', () => {
  const target: ThreadReplyTarget = { id: ID, seq: 42, from: 'sales', addr: 'sales', text: 'Ship it?\nOr wait for QA.', part: false }

  it('a whole short message: the short marker line, the quote, then the words (no id, no command)', () => {
    expect(formatThreadReply(target, '  Ship it.  ')).toBe('↩ #42 sales:\n> Ship it?\n> Or wait for QA.\n\nShip it.')
  })

  it('a long message: its first 600 characters and "…"', () => {
    const long = 'a'.repeat(THREAD_REPLY_QUOTE_MAX) + 'b'.repeat(100)
    const sent = formatThreadReply({ ...target, text: long }, 'ok')
    expect(sent.split('\n')[1]).toBe(`> ${'a'.repeat(THREAD_REPLY_QUOTE_MAX)}…`)
    expect(sent).not.toContain('b')
    // Exactly 600 is not cut.
    expect(formatThreadReply({ ...target, text: 'a'.repeat(THREAD_REPLY_QUOTE_MAX) }, 'ok')).not.toContain('…')
  })

  it('a selection: exactly the snippet, "part of"', () => {
    expect(formatThreadReply({ ...target, text: 'wait for QA', part: true }, 'Why?')).toBe('↩ part of #42 sales:\n> wait for QA\n\nWhy?')
  })

  it('blank lines in a quote stay quoted; no seq: no #N; a name with spaces stays one line', () => {
    expect(formatThreadReply({ ...target, text: 'one\n\ntwo' }, 'x')).toBe('↩ #42 sales:\n> one\n>\n> two\n\nx')
    expect(formatThreadReply({ ...target, seq: 0, from: ' Ana  Lee ' }, 'x').split('\n')[0]).toBe('↩ Ana Lee:')
  })

  it('reads back: quote, words, part, seq; anything else is not a reply', () => {
    expect(parseThreadReply(formatThreadReply({ ...target, text: 'one\n\ntwo', part: true }, 'line 1\nline 2'))).toEqual({
      part: true,
      seq: 42,
      from: 'sales',
      id: '',
      quote: 'one\n\ntwo',
      rest: 'line 1\nline 2',
    })
    expect(parseThreadReply(formatThreadReply({ ...target, seq: 0, from: 'Ana Lee' }, 'x'))).toMatchObject({ seq: 0, from: 'Ana Lee' })
    expect(parseThreadReply('↩ #42 sales:\nno quote')).toBeNull()
    expect(parseThreadReply('↩ back soon')).toBeNull()
    expect(parseThreadReply('Replying to you later')).toBeNull()
    expect(parseThreadReply('hello')).toBeNull()
  })

  it('the first build\'s long header still reads back (id and all)', () => {
    const old =
      `Replying to part of Thread message 42 from sales (id ${ID}; read it whole: k2 thread sales --before-seq 43 --limit 1):\n` +
      '> wait for QA\n\nWhy?'
    expect(parseThreadReply(old)).toEqual({ part: true, seq: 42, from: 'sales', id: ID, quote: 'wait for QA', rest: 'Why?' })
  })
})

describe('the message menu', () => {
  it('opens on right-click, with Reply when the Thread has a message box', () => {
    renderThread([item(42, 'Ship it?')])
    fireEvent.contextMenu(bubbles()[0], { clientX: 10, clientY: 20 })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(menuLabels()).toEqual(['Reply', 'Copy text', 'Save to stash'])
  })

  it('opens on a long press (touch); a press that moves does not', () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    renderThread([item(42, 'Ship it?')])
    const b = bubbles()[0]
    fireEvent.pointerDown(b, { pointerType: 'touch', clientX: 5, clientY: 5 })
    fireEvent.pointerMove(b, { pointerType: 'touch', clientX: 5, clientY: 40 })
    act(() => {
      vi.advanceTimersByTime(THREAD_LONG_PRESS_MS + 50)
    })
    expect(useContextMenuStore.getState().isOpen).toBe(false)

    fireEvent.pointerDown(b, { pointerType: 'touch', clientX: 5, clientY: 5 })
    act(() => {
      vi.advanceTimersByTime(THREAD_LONG_PRESS_MS - 50)
    })
    expect(useContextMenuStore.getState().isOpen).toBe(false)
    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(menuLabels()).toContain('Reply')
    // The browser's own long-press menu right after is swallowed.
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true })
    b.dispatchEvent(ev)
    expect(ev.defaultPrevented).toBe(true)
  })

  it('a mouse press held still is not a long press', () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    renderThread([item(42, 'Ship it?')])
    fireEvent.pointerDown(bubbles()[0], { pointerType: 'mouse', clientX: 5, clientY: 5 })
    act(() => {
      vi.advanceTimersByTime(THREAD_LONG_PRESS_MS * 2)
    })
    expect(useContextMenuStore.getState().isOpen).toBe(false)
  })
})

describe('Reply', () => {
  it('shows the chip (first line), × cancels, Esc cancels', async () => {
    renderThread([item(42, 'Ship it?\nOr wait for QA.')])
    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    const chip = await screen.findByTestId('thread-reply-chip')
    expect(chip.querySelector('[data-reply-chip-text]')?.textContent).toBe('Ship it?')
    // The box takes focus for the words.
    expect(document.activeElement).toBe(screen.getByRole('textbox'))
    fireEvent.click(screen.getByTestId('thread-reply-cancel'))
    expect(screen.queryByTestId('thread-reply-chip')).toBeNull()

    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' })
    expect(screen.queryByTestId('thread-reply-chip')).toBeNull()
    expect(threadPosts()).toEqual([])
  })

  it('Send posts the quote, the reference and the words, then clears the chip', async () => {
    renderThread([item(42, 'Ship it?')])
    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'Yes, ship it.' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts().map((p) => p.text)).toEqual([
      '↩ #42 sales:\n> Ship it?\n\nYes, ship it.',
    ])
    await waitFor(() => expect(screen.queryByTestId('thread-reply-chip')).toBeNull())
  })

  it('a long message goes as its first 600 characters and "…"', async () => {
    const long = `${'x'.repeat(THREAD_REPLY_QUOTE_MAX)}TAIL`
    renderThread([item(42, long)])
    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'ok' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    const [sent] = threadPosts()
    expect(sent.text.split('\n')[1]).toBe(`> ${'x'.repeat(THREAD_REPLY_QUOTE_MAX)}…`)
    expect(sent.text).not.toContain('TAIL')
    expect(sent.text.split('\n')[0]).toBe('↩ #42 sales:')
  })

  it('a refused send keeps the chip and the words', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: false, error: 'nope' })
    renderThread([item(42, 'Ship it?')])
    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'Yes' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts()).toHaveLength(1)
    expect(screen.getByTestId('thread-reply-chip')).toBeTruthy()
    expect(box.value).toBe('Yes')
  })

  it('a sent reply draws its quote above the words, not the marker line', () => {
    const body = formatThreadReply({ id: ID, seq: 42, from: 'sales', addr: 'sales', text: 'Ship it?', part: false }, 'Yes, ship it.')
    renderThread([item(7, body, { from: 'owner', via: 'compose' })], { box: false })
    const quote = document.querySelector('[data-thread-reply-quote]') as HTMLElement
    expect(quote.textContent).toBe('sales: Ship it?')
    const bubble = bubbles()[0]
    expect(bubble.textContent).toContain('Yes, ship it.')
    expect(bubble.textContent).not.toContain('#42')
  })

  it('a reply sent with the first build\'s long header draws the same', () => {
    const body =
      `Replying to Thread message 42 from sales (id ${ID}; read it whole: k2 thread sales --before-seq 43 --limit 1):\n` +
      '> Ship it?\n\nYes, ship it.'
    renderThread([item(7, body, { from: 'owner', via: 'compose' })], { box: false })
    expect((document.querySelector('[data-thread-reply-quote]') as HTMLElement).textContent).toBe('sales: Ship it?')
    expect(bubbles()[0].textContent).not.toContain('read it whole')
  })

  it('no message box for the Thread: no Reply', () => {
    renderThread([item(42, 'Ship it?')], { box: false })
    fireEvent.contextMenu(bubbles()[0])
    expect(menuLabels()).toEqual(['Copy text', 'Save to stash'])
  })
})

describe('Reply to selection', () => {
  it('shows for a selection inside one message and quotes exactly the snippet', async () => {
    renderThread([item(42, 'Ship it today or wait for QA'), item(43, 'Second message')])
    selectIn(bubbles()[0], 'Ship it today', 17, 28)
    fireEvent.contextMenu(bubbles()[0])
    expect(menuLabels()).toEqual(['Reply to selection', 'Reply', 'Copy selection', 'Save to stash'])
    act(() => useContextMenuStore.getState().selectItem('reply-selection'))
    const chip = await screen.findByTestId('thread-reply-chip')
    expect(chip.hasAttribute('data-reply-part')).toBe(true)
    expect(chip.querySelector('[data-reply-chip-text]')?.textContent).toBe('wait for QA')
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'Why wait?' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts().map((p) => p.text)).toEqual([
      '↩ part of #42 sales:\n> wait for QA\n\nWhy wait?',
    ])
  })

  it('not without a selection; a selection across two messages keeps the webview menu', () => {
    renderThread([item(42, 'Ship it today or wait for QA'), item(43, 'Second message')])
    fireEvent.contextMenu(bubbles()[0])
    expect(menuLabels()).not.toContain('Reply to selection')
    useContextMenuStore.getState().close()

    // From inside the first message to inside the second.
    const range = document.createRange()
    const first = document.createTreeWalker(bubbles()[0], NodeFilter.SHOW_TEXT).nextNode()!
    const second = document.createTreeWalker(bubbles()[1], NodeFilter.SHOW_TEXT).nextNode()!
    range.setStart(first, 5)
    range.setEnd(second, 6)
    window.getSelection()!.removeAllRanges()
    window.getSelection()!.addRange(range)
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true })
    bubbles()[0].dispatchEvent(ev)
    expect(ev.defaultPrevented).toBe(false)
    expect(useContextMenuStore.getState().isOpen).toBe(false)
  })
})

describe('the Zen conversation box', () => {
  it('takes a Reply: chip, Esc cancels it before the draft, Send quotes it', async () => {
    const call = vi.fn(async () => undefined)
    const bridge = { call, widgetId: 'w1' } as unknown as ZenWidgetBridge
    const key = threadReplyKey(scope, 'sales')
    render(
      <>
        <ThreadAttachmentSourceProvider value={{ scope, addr: 'sales' }}>
          <ThreadBubble item={item(42, 'Ship it?')} mine={false} author="sales" timeLabel="now" viewer={OWNER} />
        </ThreadAttachmentSourceProvider>
        <ZenCompose bridge={bridge} address="local|sales" label="sales" disabled={false} replyKey={key} />
      </>,
    )
    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'Yes' } })
    fireEvent.keyDown(box, { key: 'Escape' })
    expect(screen.queryByTestId('thread-reply-chip')).toBeNull()
    expect(box.value).toBe('Yes')

    fireEvent.contextMenu(bubbles()[0])
    act(() => useContextMenuStore.getState().selectItem('reply'))
    await screen.findByTestId('thread-reply-chip')
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(call).toHaveBeenCalledWith(
      'thread.post',
      'local|sales',
      '↩ #42 sales:\n> Ship it?\n\nYes',
      {},
    )
    await waitFor(() => expect(screen.queryByTestId('thread-reply-chip')).toBeNull())
  })
})
