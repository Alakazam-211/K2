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

// The owner link an answer hands back goes to the app's open_owner_link
// (lib/owner-link.ts), never to the opener plugin's openUrl.
const opened = vi.hoisted(() => ({ invoke: vi.fn(async (..._a: unknown[]) => undefined), openUrl: vi.fn() }))
vi.mock('@tauri-apps/api/core', async (orig) => {
  const real = await orig<typeof import('@tauri-apps/api/core')>()
  return {
    ...real,
    invoke: (cmd: string, args?: Record<string, unknown>) =>
      cmd === 'open_owner_link' ? opened.invoke(cmd, args) : real.invoke(cmd, args),
  }
})
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (...a: unknown[]) => opened.openUrl(...a) }))

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
  opened.invoke.mockClear()
  opened.openUrl.mockClear()
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

  it('an owner link in the answer opens through open_owner_link, never openUrl', async () => {
    const link = 'https://chat.example.test/k2/owner-handoff#code=k2sho_TESTSECRET0001'
    daemonCliPost.mockResolvedValueOnce({ ok: true, forYou: { openUrl: link } } as unknown as { ok: boolean })
    const a = permissionCardActions(scopeWith([]), 'billing', 'tc_owner')
    await a.answer({ answer: 'Allow' })
    expect(opened.invoke.mock.calls).toEqual([['open_owner_link', { link }]])
    expect(opened.openUrl).not.toHaveBeenCalled()
  })

  it('an answer without an https owner link opens nothing', async () => {
    daemonCliPost.mockResolvedValueOnce({ ok: true, forYou: { openUrl: 'http://chat.example.test/x' } } as unknown as { ok: boolean })
    await permissionCardActions(scopeWith([]), 'billing', 'tc_plain').answer({ answer: 'Allow' })
    expect(opened.invoke).not.toHaveBeenCalled()
    expect(opened.openUrl).not.toHaveBeenCalled()
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
