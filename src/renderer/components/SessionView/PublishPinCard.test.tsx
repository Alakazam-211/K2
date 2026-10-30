// @vitest-environment jsdom
// K2's own PIN form on a card (PV-7, PV-11): `publish.pin`'s Allow opens it, the
// PIN goes straight to the route as this person, and only then does the card
// close, with no PIN in its answer. The upgrade card's "Add a PIN" opens the same
// form.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const post = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: (...a: unknown[]) => post(...a),
}))

import { ChoiceCard } from './ChoiceCard'
import { applyOverlayFrame, threadItemsFromSnapshot, type OverlayChoice } from './overlayThread'
import { permissionDoc } from './permissionCard.fixtures'
import type { PermissionCardActions } from './permissionCardView'

const scope = { id: 'local', hostKey: 'local', isPrimary: true } as unknown as ServerScope

const PIN_VIEW = {
  requestId: 'pr_pin1',
  action: 'publish.pin',
  setting: 'Put docs behind a PIN',
  whatChanges: "You choose a PIN in K2's own form.",
  grant: 'one-time',
  scope: 'the site docs',
  duration: 'until someone changes it',
  consequences: 'agent-x never sees the PIN.',
  values: [
    { key: 'workspace', value: 'agent-x' },
    { key: 'label', value: 'docs' },
  ],
  why: { by: 'agent-x', text: 'I need this site behind a PIN.' },
  notes: [],
  askedFor: null,
  approvers: { floor: 'admin', label: 'owner or admin', names: ['rosson'] },
  settingsPage: 'k2-connect',
}

function choiceOf(doc: Record<string, unknown>): OverlayChoice {
  const snap = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 9, id: doc.id, doc }] })
  const c = snap.items[0]?.doc.choice
  if (!c) throw new Error('no choice on the doc')
  return c
}

function pinCard(actions: PermissionCardActions): void {
  const c = choiceOf(permissionDoc({ view: PIN_VIEW }))
  render(
    <ChoiceCard
      options={c.options}
      allowCustom={c.allow_custom}
      status={c.status}
      answer={c.answer}
      expiresAt={c.expiresAt}
      nowSec={1761062000}
      permission={c.permission}
      permissionActions={actions}
      from="agent-x"
      onPick={() => {
        throw new Error('a permission card never answers through onPick')
      }}
    />,
  )
}

beforeEach(() => post.mockReset())
afterEach(() => cleanup())

describe('publish.pin', () => {
  it('Allow opens K2 form; the PIN goes to the route and never into the answer', async () => {
    post.mockResolvedValue({ label: 'docs', mode: 'pin', dev: false, pinSet: true, reason: null, enforced: true, url: null, warnings: [] })
    const answer = vi.fn(async () => ({ ok: true }))
    pinCard({ answer, confirmScope: scope })
    expect(screen.getByTestId('permission-pin-note').textContent).toContain('never sees it')
    expect(screen.queryByTestId('site-pin-form')).toBeNull()
    fireEvent.click(screen.getByTestId('permission-allow'))
    expect(answer).not.toHaveBeenCalled()
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: 'correct-horse' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: 'correct-horse' } })
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(answer).toHaveBeenCalled())
    expect(post).toHaveBeenCalledWith(scope, 'publish/visibility', { label: 'docs', mode: 'pin', pin: 'correct-horse' })
    expect(JSON.stringify(answer.mock.calls)).not.toContain('correct-horse')
  })

  it('a refused PIN leaves the card open and unanswered', async () => {
    post.mockImplementationOnce(async () => {
      throw new Error('only the owner or an admin can set a PIN')
    })
    const answer = vi.fn(async () => ({ ok: true }))
    pinCard({ answer, confirmScope: scope })
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: 'correct-horse' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: 'correct-horse' } })
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(screen.getByTestId('site-pin-error').textContent).toContain('owner or an admin'))
    expect(answer).not.toHaveBeenCalled()
  })
})

describe('the upgrade card (PV-11)', () => {
  const OPTIONS = [{ label: 'docs: Make private' }, { label: 'docs: Add a PIN' }, { label: 'docs: Keep public' }]

  it('"Add a PIN" opens K2 form for that site, and it closes when done', async () => {
    post.mockResolvedValue({ label: 'docs', mode: 'pin', dev: false, pinSet: true, reason: null, enforced: true, url: null, warnings: [] })
    render(
      <ChoiceCard
        options={OPTIONS}
        allowCustom={false}
        status="answered"
        answer="docs: Add a PIN"
        resultMessage="Set docs's PIN in K2's form."
        openPinForm="docs"
        pinScope={scope}
        onPick={() => undefined}
      />,
    )
    expect(screen.getByTestId('site-pin-form').textContent).toContain('docs')
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: '12345678' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: '12345678' } })
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(screen.queryByTestId('site-pin-form')).toBeNull())
    expect(post).toHaveBeenCalledWith(scope, 'publish/visibility', { label: 'docs', mode: 'pin', pin: '12345678' })
  })

  it('a client with no server to send it to shows only the words', () => {
    render(
      <ChoiceCard options={OPTIONS} allowCustom={false} status="answered" answer="docs: Add a PIN" resultMessage="k2 publish visibility docs pin" openPinForm="docs" onPick={() => undefined} />,
    )
    expect(screen.queryByTestId('site-pin-form')).toBeNull()
    expect(screen.getByTestId('thread-choice-result').textContent).toContain('k2 publish visibility docs pin')
  })

  it('the frame carries the site from action.result', () => {
    const items = applyOverlayFrame(
      [],
      {
        collection: 'thread',
        seq: 1,
        id: 'k1',
        doc: {
          id: 'k1',
          kind: 'choice',
          from: 'k2',
          choice: {
            prompt: 'K2 now lets you choose',
            options: OPTIONS,
            allow_custom: false,
            status: 'answered',
            answer: 'docs: Add a PIN',
            action: { id: 'publish.upgrade-sites', params: {}, result: { message: 'Set it.', openPinForm: 'docs' } },
          } as never,
        },
      },
      1,
    )
    const c = items[0].doc.choice
    expect(c?.openPinForm).toBe('docs')
    expect(c?.resultMessage).toBe('Set it.')
  })
})
