// @vitest-environment jsdom
// Permission cards v1 (§4.2, §17.1, §17.3): the `cards=2` opt-in goes only
// to a server with the feature on, and a card's answer, note and undo go to
// the Thread's own server with the contract's bodies. In the Thread bubble
// the card draws itself (no prompt text) and answers through the surface.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const daemonCliPost = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => ({ ok: true })))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
  daemonCliGet: () => Promise.reject(new Error('permissionCardWire.test: no GET here')),
}))

import type { ServerScope } from '@/kessel/server-scope'
import {
  PERMISSION_CARDS_FEATURE,
  threadAnswerBody,
  threadCardsParams,
  threadCardsQuery,
  threadItemsFromSnapshot,
  type OverlayThreadItem,
} from './overlayThread'
import { permissionAnswerBody, permissionCardActions } from './permissionCardView'
import { permissionDoc } from './permissionCard.fixtures'
import { ThreadBubble, threadBubbleText } from '@/components/Thread/ThreadZenKit'
import { ThreadAttachmentSourceProvider } from '@/components/Thread/ThreadImageObjects'

afterEach(() => {
  cleanup()
  daemonCliPost.mockClear()
})

const scopeWith = (features: string[]): ServerScope =>
  ({ serverSupports: (f: string) => features.includes(f) }) as unknown as ServerScope

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 412, id: doc.id, doc }] }).items[0]
}

describe('the cards=2 opt-in (§17.1)', () => {
  it('only to a server that lists permission-cards-v1', () => {
    expect(PERMISSION_CARDS_FEATURE).toBe('permission-cards-v1')
    expect(threadCardsParams(scopeWith(['permission-cards-v1']))).toEqual({ cards: '2' })
    expect(threadCardsQuery(scopeWith(['permission-cards-v1']))).toBe('&cards=2')
    // Switch off (or an older server): nothing is sent, nothing changes.
    expect(threadCardsParams(scopeWith(['thread-read-v1']))).toEqual({})
    expect(threadCardsQuery(scopeWith([]))).toBe('')
  })
})

describe('answer bodies (§17.3)', () => {
  it('the permission answer: exact label, item, reason, token, where from', () => {
    expect(permissionAnswerBody('billing', 'tc_1', { answer: 'Allow' })).toEqual({
      addr: 'billing',
      id: 'tc_1',
      answer: 'Allow',
      client: 'desktop',
    })
    expect(
      permissionAnswerBody('billing', 'tc_1', { answer: 'Deny', item: 'pr_a1', reason: `  ${'x'.repeat(250)} ` }),
    ).toEqual({ addr: 'billing', id: 'tc_1', answer: 'Deny', item: 'pr_a1', reason: 'x'.repeat(200), client: 'desktop' })
    expect(permissionAnswerBody('shopdev', 'tc_9', { answer: 'Allow', confirmToken: 'tok' }).confirmToken).toBe('tok')
  })

  it('the shared thread/answer body carries the same fields; other cards are as before', () => {
    expect(threadAnswerBody('a', 'c', { answer: 'Yes' })).toEqual({ addr: 'a', id: 'c', answer: 'Yes' })
    expect(threadAnswerBody('a', 'c', { answer: 'Deny', item: 'pr_1', reason: 'no', client: 'desktop' })).toEqual({
      addr: 'a',
      id: 'c',
      answer: 'Deny',
      item: 'pr_1',
      reason: 'no',
      client: 'desktop',
    })
  })

  it('answer, note and undo go to the Thread’s server, by card id', async () => {
    const scope = scopeWith([])
    const a = permissionCardActions(scope, 'billing', 'tc_7f3a2c90')
    await a.answer({ answer: 'Allow' })
    await a.note?.('Payroll closes Friday')
    await a.undo?.()
    expect(daemonCliPost.mock.calls).toEqual([
      [scope, 'thread/answer', { addr: 'billing', id: 'tc_7f3a2c90', answer: 'Allow', client: 'desktop' }],
      [scope, 'thread/permission-note', { addr: 'billing', id: 'tc_7f3a2c90', text: 'Payroll closes Friday' }],
      [scope, 'thread/permission-undo', { addr: 'billing', id: 'tc_7f3a2c90' }],
    ])
    expect(a.confirmScope).toBe(scope)
  })
})

describe('in the Thread bubble', () => {
  const bubble = (item: OverlayThreadItem, onAnswer?: (p: unknown) => unknown) => (
    <ThreadBubble item={item} mine={false} author="billing" timeLabel="now" viewer={null} nowSec={1761062000} onAnswer={onAnswer} />
  )

  it('a permission card draws itself: no prompt text above it', () => {
    const item = itemOf(permissionDoc({}))
    expect(threadBubbleText(item)).toBe('')
    render(bubble(item))
    expect(screen.getByTestId('permission-card')).toBeTruthy()
    expect(screen.getByTestId('thread-item').textContent).not.toContain('billing asks:')
  })

  it('on a surface that names the Thread’s server, Allow posts there', async () => {
    const scope = scopeWith([])
    render(
      <ThreadAttachmentSourceProvider value={{ scope, addr: 'billing' }}>{bubble(itemOf(permissionDoc({})))}</ThreadAttachmentSourceProvider>,
    )
    fireEvent.click(screen.getByTestId('permission-allow'))
    await waitFor(() =>
      expect(daemonCliPost).toHaveBeenCalledWith(scope, 'thread/answer', {
        addr: 'billing',
        id: 'tc_7f3a2c90',
        answer: 'Allow',
        client: 'desktop',
      }),
    )
  })

  it('elsewhere it answers through the surface’s onAnswer (no note box, no confirm)', async () => {
    const onAnswer = vi.fn(async () => undefined)
    render(bubble(itemOf(permissionDoc({})), onAnswer))
    fireEvent.click(screen.getByTestId('permission-deny'))
    fireEvent.change(screen.getByTestId('permission-deny-reason'), { target: { value: 'later' } })
    fireEvent.click(screen.getByTestId('permission-deny-send'))
    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith({ answer: 'Deny', reason: 'later', client: 'desktop' }))
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('an ordinary choice card is untouched', () => {
    const item = itemOf({
      id: 'c1',
      kind: 'choice',
      from: 'billing',
      choice: { prompt: 'Ship it?', options: [{ label: 'Yes' }], allow_custom: false, status: 'pending' },
    })
    expect(threadBubbleText(item)).toBe('Ship it?')
    render(bubble(item))
    expect(screen.getByTestId('thread-choice-card')).toBeTruthy()
    expect(screen.queryByTestId('permission-card')).toBeNull()
  })
})
