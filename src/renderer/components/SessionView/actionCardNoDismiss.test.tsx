// @vitest-environment jsdom
// C13 (lane B), as changed in 0.46.11 (prd-popup-close-and-card-dismiss-v1,
// Rosson: cards need a dismiss "because sometimes the question is no longer
// relevant"): a person who may answer an action card may now dismiss it
// (`thread/void`, the daemon checks the answer gate; nothing is applied or
// denied). It is never a one-tap X: a Dismiss link opens a confirm line
// first, and a viewer who only waits on a permission card gets none. Every
// Thread surface draws cards through the shared ThreadBubble: the desktop
// Thread (ThreadItemRow), Zen's conversation and the ticket / project chat
// (ThreadChat). A secret card keeps its one-tap dismiss.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { ThreadItemRow } from './ThreadOverlayPane'
import { ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { threadItemsFromSnapshot, type OverlayThreadItem } from './overlayThread'
import { batchDoc, permissionDoc, VIEWERS } from './permissionCard.fixtures'

afterEach(() => cleanup())

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 7, id: doc.id, doc }] }).items[0]
}

const KEEP_AS_MINE = {
  id: 'tc_keep1',
  kind: 'choice',
  from: 'k2',
  via: 'garden',
  choice: {
    prompt: 'The widget board was made by builder. Keep it as yours?',
    options: [{ label: 'Keep as mine' }, { label: 'Leave it as builder’s' }],
    allow_custom: false,
    status: 'pending',
    action: { id: 'zen.widget.keep', params: { widget: 'board' }, dedupe: 'keep:board' },
  },
}

const ACTION_CARDS: Array<[string, Record<string, unknown>]> = [
  ['keep-as-mine', KEEP_AS_MINE],
  ['permission, approver', permissionDoc({})],
  ['permission, waiting', permissionDoc({ viewer: VIEWERS.lee, view: { noteBox: { allowed: true, max: 500, mine: null } } })],
  ['permission, already on', permissionDoc({ options: ['Close'], view: { state: 'already_done' } })],
  ['permission batch', batchDoc()],
  ['permission fallback (no view)', { ...permissionDoc({}), choice: { ...(permissionDoc({}).choice as object), action: { id: 'permission', params: { request: 'pr_7f3a2c' } } } }],
]

/** Anything that reads as a dismiss: a void/dismiss test id, an X glyph, or
 *  a dismiss/void/remove label. The two-step Dismiss link (0.46.11) is
 *  reported apart, by `twoStep`. */
function dismissControls(root: HTMLElement): string[] {
  const out: string[] = []
  for (const el of Array.from(root.querySelectorAll<HTMLElement>('button, [role="button"], [data-testid]'))) {
    const id = el.getAttribute('data-testid') ?? ''
    if (id === 'thread-card-dismiss') continue
    const label = `${el.getAttribute('aria-label') ?? ''} ${el.getAttribute('title') ?? ''}`
    const text = (el.textContent ?? '').trim()
    if (/void|dismiss/i.test(id) || /dismiss|void|remove card/i.test(label) || /^[×✕✖xX]$/.test(text)) out.push(id || label || text)
  }
  return out
}

const twoStep = (root: HTMLElement): number => root.querySelectorAll('[data-testid="thread-card-dismiss"]').length

/** Cards whose viewer may answer them: they get the two-step Dismiss. The
 *  waiting viewer (no answer) gets none. */
const DISMISSABLE = new Set(['keep-as-mine', 'permission, approver', 'permission, already on', 'permission fallback (no view)'])

describe('an action card is never closed in one tap, on any Thread surface (C13, 0.46.11)', () => {
  for (const [name, doc] of ACTION_CARDS) {
    it(`${name}: no X; a Dismiss only for a viewer who may answer, and only behind its confirm`, () => {
      const onVoid = vi.fn()
      const item = itemOf(doc)
      expect(item.doc.kind).toBe('choice')
      const a = render(<ThreadItemRow item={item} nowSec={1761062000} onAnswer={() => undefined} onVoid={onVoid} />)
      expect([name, dismissControls(a.container)]).toEqual([name, []])
      if (DISMISSABLE.has(name)) expect([name, twoStep(a.container)]).toEqual([name, 1])
      if (name === 'permission, waiting') expect([name, twoStep(a.container)]).toEqual([name, 0])
      cleanup()
      const b = render(
        <ThreadBubble item={item} mine={false} author="billing" timeLabel="now" viewer={null} nowSec={1761062000} onAnswer={() => undefined} onVoid={onVoid} />,
      )
      expect([name, dismissControls(b.container)]).toEqual([name, []])
      if (DISMISSABLE.has(name)) {
        // Pressing Dismiss only opens the confirm line: nothing is sent yet.
        fireEvent.click(screen.getByTestId('thread-card-dismiss'))
        expect(screen.getByTestId('thread-card-dismiss-form')).toBeTruthy()
      }
      expect(onVoid).not.toHaveBeenCalled()
    })
  }

  it('the check is real: a pending secret card still draws its dismiss', () => {
    const item = itemOf({ id: 's1', kind: 'secret', from: 'billing', secret: { name: 'STRIPE_KEY', status: 'pending', prompt: 'Stripe key' } })
    const r = render(<ThreadItemRow item={item} nowSec={1761062000} onAnswer={() => undefined} onVoid={() => undefined} />)
    expect(screen.getByTestId('thread-secret-dismiss')).toBeTruthy()
    expect(dismissControls(r.container).length).toBeGreaterThan(0)
  })

  it('the Garden kit (k2-thread@1) has no void or dismiss for any card', () => {
    const kit = readFileSync(resolve(__dirname, '../../zen-lib/k2-thread@1.1.1/k2-thread.js'), 'utf8')
    expect(kit.includes('thread.void')).toBe(false)
    expect(/dismiss/i.test(kit)).toBe(false)
  })
})
