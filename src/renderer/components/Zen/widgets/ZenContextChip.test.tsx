// @vitest-environment jsdom
// prd-garden-column-collapse-and-agent-list-v1 §4 (0.46.3 r10): the Board
// context chip is visible and removable, never hidden text.
//   - the message box of a Conversation that follows the widget shows the
//     chip; × removes it, and then nothing rides the send;
//   - with the chip, the send carries exactly the text the chip showed;
//   - a context that didn't go leaves a note where the chip was;
//   - a sent message with a context shows the widget's line, marked as the
//     widget's (the desktop Thread and Zen draw the same bubble);
//   - a Thread item keeps `context` from a post answer and a page, and a
//     widget reading the Thread gets `{text, widget, placement}` only.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZenCompose } from './ZenCompose'
import { ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { overlayItemFromThreadPost, type OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { projectZenThreadItem } from '@/lib/zen/zen-custom-projection'
import {
  __resetZenThreadContextForTests,
  noteZenThreadContext,
  setZenThreadContext,
  zenThreadContextKey,
} from '@/lib/zen/zen-thread-context'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const G = 'g-test0001'
const KEY = zenThreadContextKey(G, 'board')
const ORIGIN = { widget: 'board-builder', garden: G, placement: 'board', hash: 'h1' }

function fakeBridge(): { bridge: ZenWidgetBridge; posts: unknown[][] } {
  const posts: unknown[][] = []
  const bridge = {
    widgetId: 'chat',
    caps: new Set(['thread:post']),
    gardens: { current: () => ({ id: G, name: 'Garden 1', index: 1 }) },
    call: vi.fn((verb: string, ...args: unknown[]) => {
      if (verb === 'thread.post') {
        posts.push(args)
        return Promise.resolve({ id: 'm1', seq: 1 })
      }
      return null
    }),
  } as unknown as ZenWidgetBridge
  return { bridge, posts }
}

function q(sel: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(sel)
}

async function type(text: string): Promise<void> {
  const box = q('[data-zen-compose-input]') as HTMLTextAreaElement
  await act(async () => {
    fireEvent.change(box, { target: { value: text } })
  })
  await act(async () => {
    fireEvent.keyDown(box, { key: 'Enter' })
  })
}

beforeEach(() => __resetZenThreadContextForTests())
afterEach(() => {
  cleanup()
  __resetZenThreadContextForTests()
})

describe('the chip in the message box', () => {
  it('shows the widget’s line, rides the send exactly, and × removes it', async () => {
    const { bridge, posts } = fakeBridge()
    render(<ZenCompose bridge={bridge} address="alice::local" label="Alice" disabled={false} />)
    expect(q('[data-zen-context-chip]')).toBeNull()
    act(() => setZenThreadContext(KEY, { text: 'Board: K2 Boards', origin: ORIGIN }, ['chat']))
    const chip = q('[data-zen-context-chip]')
    expect(chip?.textContent).toContain('Board: K2 Boards')
    expect(chip?.getAttribute('aria-label')).toContain('board-builder')
    await type('ship it')
    expect(posts).toEqual([['alice::local', 'ship it', { context: 'Board: K2 Boards' }]])

    // ×: the chip goes and nothing rides the next send.
    await act(async () => {
      fireEvent.click(q('[data-zen-context-remove]')!)
    })
    expect(q('[data-zen-context-chip]')).toBeNull()
    await type('and this')
    expect(posts[1]).toEqual(['alice::local', 'and this', {}])
  })

  it('a Conversation that doesn’t follow the widget shows nothing and sends nothing extra', async () => {
    const { bridge, posts } = fakeBridge()
    act(() => setZenThreadContext(KEY, { text: 'Board: K2 Boards', origin: ORIGIN }, ['other']))
    render(<ZenCompose bridge={bridge} address="alice::local" label="Alice" disabled={false} />)
    expect(q('[data-zen-context-chip]')).toBeNull()
    await type('plain')
    expect(posts).toEqual([['alice::local', 'plain', {}]])
  })

  it('a context that didn’t go leaves a note where the chip was', () => {
    const { bridge } = fakeBridge()
    render(<ZenCompose bridge={bridge} address="alice::local" label="Alice" disabled={false} />)
    act(() => setZenThreadContext(KEY, { text: 'Board: K2 Boards', origin: ORIGIN }, ['chat']))
    act(() => noteZenThreadContext(KEY, 'Not sent: dtl is older'))
    expect(q('[data-zen-context-chip]')).toBeNull()
    expect(q('[data-zen-context-note]')?.textContent).toContain('Not sent: dtl is older')
  })
})

describe('the sent message', () => {
  const CONTEXT = { text: 'Board: K2 Boards', widget: 'board-builder', garden: G, placement: 'board' }

  it('a post answer and a page keep `context`; a doc without one has none', () => {
    const item = overlayItemFromThreadPost(
      { ok: true, id: 'm1', seq: 4, kind: 'text', from: 'rosson', body: 'ship it', via: 'compose', context: CONTEXT },
      'ship it',
    )
    expect(item?.doc.context).toEqual(CONTEXT)
    expect(item?.doc.body, 'the body is the person’s text only').toBe('ship it')
    const plain = overlayItemFromThreadPost({ ok: true, id: 'm2', seq: 5, kind: 'text', from: 'rosson', body: 'x' }, 'x')
    expect(plain && 'context' in plain.doc).toBe(false)
    // A malformed one is dropped.
    const bad = overlayItemFromThreadPost({ ok: true, id: 'm3', seq: 6, kind: 'text', from: 'r', body: 'x', context: { text: 'y' } }, 'x')
    expect(bad && 'context' in bad.doc).toBe(false)
  })

  it('the bubble shows the widget’s line, marked as the widget’s', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 'm1',
      doc: { id: 'm1', kind: 'text', from: 'rosson', body: 'ship it', via: 'compose', context: CONTEXT },
    } as OverlayThreadItem
    render(<ThreadBubble item={item} mine author="You" timeLabel="now" viewer={null} nowSec={0} />)
    const line = q('[data-thread-context]')
    expect(line?.textContent).toBe('board-builder:Board: K2 Boards')
    expect(line?.getAttribute('title')).toContain('not typed by the sender')
  })

  it('a widget reading the Thread gets {text, widget, placement} only', () => {
    const item = {
      collection: 'thread',
      seq: 4,
      id: 'm1',
      doc: { id: 'm1', kind: 'text', from: 'rosson', body: 'ship it', via: 'compose', context: CONTEXT },
    } as OverlayThreadItem
    expect(projectZenThreadItem(item)?.doc).toMatchObject({ context: { text: 'Board: K2 Boards', widget: 'board-builder', placement: 'board' } })
    const projected = projectZenThreadItem(item)?.doc as { context?: Record<string, unknown> }
    expect(Object.keys(projected.context ?? {}).sort()).toEqual(['placement', 'text', 'widget'])
    const plain = projectZenThreadItem({ ...item, doc: { ...item.doc, context: undefined } })
    expect('context' in (plain?.doc ?? {})).toBe(false)
  })
})
