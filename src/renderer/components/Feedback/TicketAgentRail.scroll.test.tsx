// @vitest-environment jsdom
// A ticket's discussion does not move while the person types (Rosson
// 2026-10-24: "each time I type a new letter or return, the chat jumps up
// … to the top of the most recent message"):
//   - typing never writes the list's scrollTop and never scrolls an element
//     into view;
//   - at the bottom, the list stays at its true bottom when the message box
//     grows (the list's box shrinks) — through the ResizeObserver or a
//     resize's clamp scroll; scrolled up, it stays where the person is;
//   - the person's own send lands the list on the newest; an arrival while
//     they read back leaves them alone, one while at the bottom follows.
// The layout half (the box collapsing while it measures) is in
// terminalCompose.test.ts (applyComposeTextareaAutosize).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
const cli = vi.hoisted(() => ({
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (path: string) => (path === 'sessions/list-for-workspace' ? [] : {})),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ThreadTab } from './TicketAgentRail'
import type { FeedbackShow } from './feedback-api'
import { __resetActivityForTests } from '@/stores/activity'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const NOW = 1_759_241_200
const TICKET = 'aaaaaaaa-0000-4000-8000-000000000002'

const ITEM: FeedbackShow = {
  id: TICKET,
  projectId: 'p1',
  sessionId: null,
  sessionKind: null,
  agentName: 'scout',
  kind: 'question',
  title: 'Which DB?',
  body: 'Pick one.',
  options: null,
  priority: 3,
  status: 'needs_discussion',
  answer: null,
  createdAt: NOW - 120,
  updatedAt: NOW - 120,
  answeredAt: null,
  commentCount: 2,
  assignees: ['owner'],
  workspace: 'Alpha',
  projectPath: '/ws',
  canonicalSessionId: null,
  comments: [
    { author: 'scout', body: 'Which DB?', at: NOW - 120 },
    { author: 'owner', body: 'A long answer that wraps over many lines.', at: NOW - 60 },
  ],
}

function withComment(item: FeedbackShow, author: string, body: string): FeedbackShow {
  return { ...item, commentCount: item.commentCount + 1, comments: [...item.comments, { author, body, at: NOW }] }
}

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  private readonly cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
    FakeResizeObserver.instances.push(this)
  }
  observe(el: Element): void {
    this.observed.push(el)
  }
  unobserve(el: Element): void {
    this.observed = this.observed.filter((o) => o !== el)
  }
  disconnect(): void {
    this.observed = []
  }
  fire(): void {
    this.cb(
      this.observed.map((target) => ({ target, contentRect: { width: 0, height: 0 } }) as ResizeObserverEntry),
      this as unknown as ResizeObserver,
    )
  }
}

/** Fire every ResizeObserver watching `el` (the browser does after layout). */
function resized(el: Element): void {
  const watching = FakeResizeObserver.instances.filter((o) => o.observed.includes(el))
  if (watching.length === 0) throw new Error('no ResizeObserver is watching the list scroller')
  act(() => {
    for (const o of watching) o.fire()
  })
}

/** A scroll box with layout (jsdom has none): the browser clamps scrollTop
 *  to [0, scrollHeight - clientHeight]; every write is logged. */
interface Box {
  client: number
  content: number
  top: number
  writes: number[]
}
function giveLayout(el: HTMLElement, box: Box): void {
  Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => box.client })
  Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => box.content })
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get: () => box.top,
    set: (v: number) => {
      box.writes.push(v)
      box.top = Math.max(0, Math.min(v, box.content - box.client))
    },
  })
}

const frames = async (n = 2): Promise<void> => {
  for (let i = 0; i < n; i++) {
    await act(async () => {
      await new Promise<void>((r) => requestAnimationFrame(() => r()))
    })
  }
}

function Tab({ item, onChanged = () => {} }: { item: FeedbackShow; onChanged?: () => void }): React.JSX.Element {
  return (
    <ThreadTab
      item={item}
      error={null}
      nowSec={NOW}
      ticketId={TICKET}
      options={[]}
      optionsLive={false}
      onChanged={onChanged}
    />
  )
}

/** Opens the ticket with the list at its bottom (300 tall, 1000 of rows). */
async function openTicket(onChanged?: () => void): Promise<{
  scroller: HTMLElement
  box: Box
  input: HTMLTextAreaElement
  rerender: (item: FeedbackShow) => void
}> {
  const view = render(<Tab item={ITEM} onChanged={onChanged} />)
  const list = document.querySelector(`[data-ticket-thread="${TICKET}"]`)
  const scroller = list?.parentElement
  if (!scroller) throw new Error('no ticket thread scroller')
  const box: Box = { client: 300, content: 1000, top: 0, writes: [] }
  giveLayout(scroller, box)
  // The browser's first observation, once laid out.
  resized(scroller)
  await frames()
  expect(box.top).toBe(700)
  box.writes = []
  return {
    scroller,
    box,
    input: screen.getByTestId('ticket-compose-input') as HTMLTextAreaElement,
    rerender: (item) => view.rerender(<Tab item={item} onChanged={onChanged} />),
  }
}

/** The person scrolls the list to `top`. */
function scrollTo(scroller: HTMLElement, box: Box, top: number): void {
  box.top = top
  fireEvent.scroll(scroller)
}

let scrollIntoView: ReturnType<typeof vi.fn>
let elementScrollTo: ReturnType<typeof vi.fn>

beforeEach(() => {
  localStorage.clear()
  __resetActivityForTests()
  FakeResizeObserver.instances = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  scrollIntoView = vi.fn()
  elementScrollTo = vi.fn()
  Element.prototype.scrollIntoView = scrollIntoView as unknown as Element['scrollIntoView']
  Element.prototype.scrollTo = elementScrollTo as unknown as Element['scrollTo']
  cli.post.mockReset()
  cli.post.mockImplementation(async (path: string) =>
    path === 'feedback/comment' ? { ok: true, delivered: true, deliveryReason: null } : {},
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('a ticket’s discussion while typing', () => {
  it('typing letters and new lines never scrolls the list', async () => {
    const { box, input } = await openTicket()
    let text = ''
    for (const ch of 'hello\nworld\n') {
      if (ch === '\n') fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
      text += ch
      fireEvent.change(input, { target: { value: text } })
    }
    await frames()
    expect(input.value).toBe('hello\nworld\n')
    expect(box.writes).toEqual([])
    expect(box.top).toBe(700)
    expect(scrollIntoView).not.toHaveBeenCalled()
    expect(elementScrollTo).not.toHaveBeenCalled()
    expect(cli.post).not.toHaveBeenCalledWith('feedback/comment', expect.anything())
  })

  it('at the bottom, the box growing keeps the true bottom in view', async () => {
    const { scroller, box } = await openTicket()
    // A new line: the box grows 40px, the list's box shrinks by as much.
    box.client = 260
    resized(scroller)
    expect(box.top).toBe(740)
    // A clamp scroll that reaches the list before the observer does is
    // layout, not the person: it stays pinned.
    box.client = 220
    fireEvent.scroll(scroller)
    expect(box.top).toBe(780)
    box.content = 1060
    resized(scroller)
    expect(box.top).toBe(840)
  })

  it('scrolled up, the box growing leaves the reading position alone', async () => {
    const { scroller, box } = await openTicket()
    scrollTo(scroller, box, 200)
    box.writes = []
    box.client = 260
    resized(scroller)
    fireEvent.scroll(scroller)
    box.client = 220
    resized(scroller)
    expect(box.writes).toEqual([])
    expect(box.top).toBe(200)
  })
})

describe('a ticket’s discussion on new messages', () => {
  it('the person’s own send lands the list on the newest, even scrolled up', async () => {
    const onChanged = vi.fn()
    const { scroller, box, input, rerender } = await openTicket(onChanged)
    scrollTo(scroller, box, 200)
    fireEvent.change(input, { target: { value: 'Postgres, please' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(onChanged).toHaveBeenCalled())
    expect(cli.post).toHaveBeenCalledWith('feedback/comment', expect.objectContaining({ body: 'Postgres, please' }))
    // The refetch brings the comment back.
    box.content = 1100
    rerender(withComment(ITEM, 'owner', 'Postgres, please'))
    await frames()
    expect(box.top).toBe(800)
  })

  it('an arrival while reading back leaves the person where they are', async () => {
    const { scroller, box, rerender } = await openTicket()
    scrollTo(scroller, box, 200)
    box.writes = []
    box.content = 1100
    rerender(withComment(ITEM, 'scout', 'Noted.'))
    await frames()
    expect(box.writes).toEqual([])
    expect(box.top).toBe(200)
  })

  it('an arrival while at the bottom follows it', async () => {
    const { box, rerender } = await openTicket()
    box.content = 1100
    rerender(withComment(ITEM, 'scout', 'Noted.'))
    await frames()
    expect(box.top).toBe(800)
  })
})
