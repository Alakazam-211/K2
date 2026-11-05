// @vitest-environment jsdom
// The folder-trust card (2026-11-05 field report): a person tapped a card
// that had already been answered, from a view that still showed it waiting,
// and "nothing happened": the daemon's 409 went to the console only. A tap
// that doesn't count now says why on the card (the daemon's words: not the
// owner, already answered by whom and when), and a stale card updates from
// the refusal itself.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ThreadBubble } from './ThreadZenKit'
import { applyOverlayFrame, staleCardFrame, threadItemsFromSnapshot, type OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { DaemonHttpError } from '@/lib/daemon-http-error'

afterEach(() => cleanup())

const PENDING = {
  id: 'tc_ft1',
  kind: 'choice',
  from: 'k2-market-research',
  body: 'The settings in ` /home/k2/ai/K2 - Market Research ` changed while k2-market-research was running.',
  choice: {
    prompt: 'The settings in … changed',
    options: [{ label: 'Trust it' }, { label: 'Always trust this folder' }, { label: "Don't trust" }],
    allow_custom: false,
    status: 'pending',
    action: { id: 'folder.trust', params: { path: '/home/k2/ai/K2 - Market Research' }, dedupe: '/home/k2/ai/K2 - Market Research' },
  },
}

const ANSWERED = {
  ...PENDING,
  choice: {
    ...PENDING.choice,
    status: 'answered',
    answer: 'Always trust this folder',
    answeredBy: { kind: 'user', login: 'baden' },
    answeredAt: 1793978568,
    action: { ...PENDING.choice.action, result: { decision: 'always', message: 'Always trusted. Undo in Settings → Workspaces.' } },
  },
}

const STALE_HINT = '“Always trust this folder” by baden at 2026-11-05 15:12 UTC. Always trusted. Undo in Settings → Workspaces.'

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 41, id: doc.id, doc }] }).items[0]
}

function staleRefusal(): DaemonHttpError {
  const hint = `Already answered: ${STALE_HINT}`
  return new DaemonHttpError(hint, 409, 'card_not_pending', { code: 'card_not_pending', hint }, {
    ok: false,
    error: { code: 'card_not_pending', hint },
    id: 'tc_ft1',
    seq: 41,
    conversation_id: 'c',
    doc: ANSWERED,
  })
}

function renderCard(onAnswer: () => Promise<void>): void {
  render(
    <ThreadBubble item={itemOf(PENDING)} mine={false} author="k2-market-research" timeLabel="now" viewer={null} nowSec={1793978000} onAnswer={onAnswer} />,
  )
}

describe('a folder-trust card tap that does not count', () => {
  it('shows the refusal for someone who may not decide, naming the owner', async () => {
    const hint = "only this server's owner answers a settings card (the same people as Settings → Gardens → Local Data)"
    renderCard(() => Promise.reject(new DaemonHttpError(hint, 403, 'action_card_person_only', { code: 'action_card_person_only', hint })))
    expect(screen.queryByTestId('thread-choice-error')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /Always trust this folder/ }))
    await waitFor(() => expect(screen.getByTestId('thread-choice-error').textContent).toBe(hint))
    expect(screen.getByRole('alert').textContent).toContain('owner')
  })

  it('shows who already decided when the card was answered elsewhere', async () => {
    renderCard(() => Promise.reject(staleRefusal()))
    fireEvent.click(screen.getByRole('button', { name: /Trust it/ }))
    await waitFor(() => expect(screen.getByTestId('thread-choice-error').textContent).toBe(`Already answered: ${STALE_HINT}`))
  })

  it('a later successful tap clears the old words', async () => {
    let fail = true
    renderCard(() => (fail ? Promise.reject(new Error('network down')) : Promise.resolve()))
    fireEvent.click(screen.getByRole('button', { name: /Trust it/ }))
    await waitFor(() => expect(screen.getByTestId('thread-choice-error').textContent).toBe('network down'))
    fail = false
    fireEvent.click(screen.getByRole('button', { name: /Trust it/ }))
    await waitFor(() => expect(screen.queryByTestId('thread-choice-error')).toBeNull())
  })
})

describe('staleCardFrame', () => {
  it('turns the refusal’s card into a frame that updates the stale card in place', () => {
    const frame = staleCardFrame(staleRefusal())
    expect(frame).toEqual({ collection: 'thread', id: 'tc_ft1', seq: 41, doc: ANSWERED })
    const before = [itemOf(PENDING)]
    const after = applyOverlayFrame(before, frame!, 0)
    expect(after).toHaveLength(1)
    const c = after[0].doc.choice!
    expect([c.status, c.answer, c.answeredBy?.login, c.resultMessage]).toEqual([
      'answered',
      'Always trust this folder',
      'baden',
      'Always trusted. Undo in Settings → Workspaces.',
    ])
  })

  it('is null for every other refusal, and for an older daemon’s 409 without the card', () => {
    expect(staleCardFrame(new DaemonHttpError('x', 403, 'action_card_person_only'))).toBeNull()
    expect(staleCardFrame(new DaemonHttpError('gone', 409, 'card_not_pending', null, { ok: false, error: { code: 'card_not_pending', hint: 'gone' } }))).toBeNull()
    expect(staleCardFrame(new Error('card_not_pending'))).toBeNull()
    expect(staleCardFrame(null)).toBeNull()
  })

  it('renders the updated card as answered, with who answered and the result', () => {
    const [item] = applyOverlayFrame([itemOf(PENDING)], staleCardFrame(staleRefusal())!, 0)
    render(<ThreadBubble item={item} mine={false} author="k2-market-research" timeLabel="now" viewer={null} nowSec={1793978600} onAnswer={() => Promise.resolve()} />)
    expect(screen.getByTestId('thread-choice-answered-by').textContent).toBe('answered by baden')
    expect(screen.getByTestId('thread-choice-result').textContent).toBe('Always trusted. Undo in Settings → Workspaces.')
    for (const b of screen.getAllByTestId('thread-choice-chip')) expect((b as HTMLButtonElement).disabled).toBe(true)
  })
})
