// @vitest-environment jsdom
// Permission cards v1 (prd-agent-settings-cards-v1 §4, §8; the phone
// contract's examples): the desktop card for every view state, drawn from
// the daemon's `cards=2` doc as the Thread receives it.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'

vi.mock('@/components/Settings/sections/DataSourceConfirm', () => ({
  // The vault confirm (Touch ID / password) answers with a token at once.
  ConfirmStep: (p: { action: string; secretId: string; onToken: (t: string) => void; onCancel: () => void }): ReactNode => (
    <div data-testid="mock-confirm" data-action={p.action} data-secret={p.secretId}>
      <button type="button" data-testid="mock-confirm-ok" onClick={() => p.onToken('tok_1')}>
        ok
      </button>
      <button type="button" data-testid="mock-confirm-cancel" onClick={() => p.onCancel()}>
        cancel
      </button>
    </div>
  ),
}))

import { ChoiceCard } from './ChoiceCard'
import { applyOverlayFrame, threadItemsFromSnapshot, type OverlayChoice } from './overlayThread'
import { permissionTime, type PermissionCardActions } from './permissionCardView'
import { APPROVED, batchDoc, CONFIRM_VIEW, FALLBACK_CHOICE, permissionDoc, VIEWERS } from './permissionCard.fixtures'
import type { ServerScope } from '@/kessel/server-scope'

afterEach(() => cleanup())

const NOW = 1761062000

/** The choice as a GET page coerces it (`threadItemsFromSnapshot`). */
function choiceOf(doc: Record<string, unknown>): OverlayChoice {
  const snap = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 412, id: doc.id, doc }] })
  const c = snap.items[0]?.doc.choice
  if (!c) throw new Error('no choice on the doc')
  return c
}

function actionsMock(): PermissionCardActions & {
  answer: ReturnType<typeof vi.fn>
  note: ReturnType<typeof vi.fn>
  undo: ReturnType<typeof vi.fn>
} {
  return {
    answer: vi.fn(async () => ({ ok: true })),
    note: vi.fn(async () => ({ ok: true })),
    undo: vi.fn(async () => ({ ok: true })),
    confirmScope: { serverSupports: () => true } as unknown as ServerScope,
    openSettings: vi.fn(),
  }
}

function draw(doc: Record<string, unknown>, actions: PermissionCardActions | null = actionsMock()): void {
  const c = choiceOf(doc)
  render(
    <ChoiceCard
      options={c.options}
      allowCustom={c.allow_custom}
      status={c.status}
      answer={c.answer}
      expiresAt={c.expiresAt}
      nowSec={NOW}
      resultMessage={c.resultMessage}
      permission={c.permission}
      permissionActions={actions}
      from="billing"
      onPick={() => {
        throw new Error('a permission card never answers through onPick')
      }}
    />,
  )
}

const text = (id: string): string => screen.getByTestId(id).textContent ?? ''

describe('PermissionCard: approver, pending (§17.11.1)', () => {
  it('draws K2’s words, the values, the agent’s words, the notes and who it was asked for', () => {
    draw(permissionDoc({}))
    const card = screen.getByTestId('permission-card')
    expect(card.getAttribute('data-state')).toBe('pending')
    expect(card.getAttribute('data-mode')).toBe('approve')
    expect(screen.queryByTestId('thread-choice-card')).toBeNull()
    expect(text('permission-headline')).toBe("Let billing run jobs on this server's compute nodes.")
    expect(text('permission-meta')).toBe('Allow compute nodes · billing · until someone turns it off')
    expect(text('permission-values')).toBe('workspace: billing')
    expect(text('permission-direction')).toBe('Loosens access')
    expect(text('permission-why')).toBe('billing says: Run the test suite on k2n2-m1; it takes 40 min here')
    expect(text('permission-note')).toBe('dana (member): I need this for the payroll run')
    expect(text('permission-asked-for')).toBe('Asked while working with dana')
    expect(text('permission-expiry')).toBe('expires in 3 days')
    expect(text('permission-allow')).toBe('Allow')
    expect(text('permission-deny')).toBe('Deny')
    expect(screen.queryByTestId('permission-note-box')).toBeNull()
  })

  it('Allow sends the exact label', async () => {
    const actions = actionsMock()
    draw(permissionDoc({}), actions)
    fireEvent.click(screen.getByTestId('permission-allow'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledTimes(1))
    expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow' })
  })

  it('Deny takes an optional one-line reason', async () => {
    const actions = actionsMock()
    draw(permissionDoc({}), actions)
    fireEvent.click(screen.getByTestId('permission-deny'))
    expect(actions.answer).not.toHaveBeenCalled()
    fireEvent.change(screen.getByTestId('permission-deny-reason'), { target: { value: 'Not this week' } })
    fireEvent.click(screen.getByTestId('permission-deny-send'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: 'Deny', reason: 'Not this week' }))
  })

  it('a refusal says why in words', async () => {
    const actions = actionsMock()
    actions.answer.mockRejectedValueOnce(new Error('{"ok":false,"error":{"code":"request_not_pending","hint":"answered"}}'))
    draw(permissionDoc({}), actions)
    fireEvent.click(screen.getByTestId('permission-allow'))
    await waitFor(() => expect(text('permission-error')).toBe('Someone answered this first, or it closed.'))
  })

  it('Open in Settings goes to the page the card names', () => {
    const actions = actionsMock()
    draw(permissionDoc({}), actions)
    fireEvent.click(screen.getByTestId('permission-open-settings'))
    expect(actions.openSettings).toHaveBeenCalledWith('projects')
  })

  it('without actions it draws read-only', () => {
    draw(permissionDoc({}), null)
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    expect(text('permission-answer-elsewhere')).toBe('Answer this in the agent’s Thread.')
  })
})

describe('PermissionCard: a confirm action (§17.11.3, §17.4)', () => {
  it('says it needs a confirm, runs the confirm sheet, then answers with the token', async () => {
    const actions = actionsMock()
    draw(permissionDoc({ view: CONFIRM_VIEW }), actions)
    expect(text('permission-allow')).toBe('Allow (needs your confirm)')
    fireEvent.click(screen.getByTestId('permission-allow'))
    expect(actions.answer).not.toHaveBeenCalled()
    const sheet = screen.getByTestId('permission-confirm-sheet')
    expect(sheet.textContent).toContain('the same confirm Settings asks for')
    const step = screen.getByTestId('mock-confirm')
    expect([step.getAttribute('data-action'), step.getAttribute('data-secret')]).toEqual(['app-approve', 'ar_22'])
    fireEvent.click(screen.getByTestId('mock-confirm-ok'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow', confirmToken: 'tok_1' }))
  })

  it('"Allow, and let it manage its Apps’ users" shows only when offered, says what the switch covers, and answers with its label', async () => {
    const always = "Allow, and let it manage its Apps' users from now on"
    const actions = actionsMock()
    draw(permissionDoc({ view: CONFIRM_VIEW }), actions)
    expect(screen.queryByTestId('permission-always')).toBeNull()
    cleanup()
    draw(permissionDoc({ view: CONFIRM_VIEW, options: ['Allow', 'Deny', always] }), actions)
    expect(text('permission-always')).toBe(always)
    fireEvent.click(screen.getByTestId('permission-always'))
    expect(actions.answer).not.toHaveBeenCalled()
    expect(text('permission-always-covers')).toContain('Never a box login')
    fireEvent.click(screen.getByTestId('mock-confirm-ok'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: always, confirmToken: 'tok_1' }))
  })

  it('cancel goes back to the buttons; without a server to confirm on, it says where to', () => {
    const actions = actionsMock()
    draw(permissionDoc({ view: CONFIRM_VIEW }), actions)
    fireEvent.click(screen.getByTestId('permission-allow'))
    fireEvent.click(screen.getByTestId('mock-confirm-cancel'))
    expect(screen.getByTestId('permission-allow')).toBeTruthy()
    cleanup()
    draw(permissionDoc({ view: CONFIRM_VIEW }), { answer: vi.fn() })
    expect((screen.getByTestId('permission-allow') as HTMLButtonElement).disabled).toBe(true)
    expect(text('permission-confirm-elsewhere')).toBe('Open the agent’s Thread to confirm this, or answer it on your phone.')
  })
})

describe('PermissionCard: wait (§17.11.5, §17.11.7)', () => {
  it('the asker: keep working with the agent, no buttons, their note in the box', async () => {
    const actions = actionsMock()
    draw(
      permissionDoc({ viewer: VIEWERS.dana, view: { noteBox: { allowed: true, max: 500, mine: 'I need this for the payroll run' } } }),
      actions,
    )
    expect(screen.getByTestId('permission-card').getAttribute('data-mode')).toBe('wait')
    expect(text('permission-waiting')).toBe(
      'Asked an admin to turn on Allow compute nodes for you. You can keep working with billing meanwhile.',
    )
    expect(text('permission-approvers')).toBe('Who can answer: owner or admin')
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    expect(screen.queryByTestId('permission-deny')).toBeNull()
    // Approvers see the notes; the asker sees their own in the box.
    expect(screen.queryByTestId('permission-notes')).toBeNull()
    const box = screen.getByTestId('permission-note-input') as HTMLInputElement
    expect(box.value).toBe('I need this for the payroll run')
    expect(box.placeholder).toBe('Leave a note for an admin (optional)')
    fireEvent.change(box, { target: { value: 'Payroll closes Friday' } })
    fireEvent.click(screen.getByTestId('permission-note-send'))
    await waitFor(() => expect(actions.note).toHaveBeenCalledWith('Payroll closes Friday'))
    expect(text('permission-note-saved')).toBe('Sent to an admin.')
    fireEvent.click(screen.getByTestId('permission-note-remove'))
    await waitFor(() => expect(actions.note).toHaveBeenLastCalledWith(''))
  })

  it('another member: the agent asked; an empty note box', () => {
    draw(permissionDoc({ viewer: VIEWERS.lee, view: { noteBox: { allowed: true, max: 500, mine: null } } }))
    expect(text('permission-waiting')).toBe('billing asked an admin to turn on Allow compute nodes.')
    expect(text('permission-note-send')).toBe('Send note')
    expect(screen.queryByTestId('permission-note-remove')).toBeNull()
  })

  it('an owner-floor request says "an owner"', () => {
    draw(permissionDoc({ viewer: VIEWERS.lee, view: { ...CONFIRM_VIEW, noteBox: { allowed: true, max: 500, mine: null } } }))
    expect(text('permission-waiting')).toBe('shopdev asked an owner to turn on App Approvals.')
  })

  it('a note the daemon would refuse is caught before sending', () => {
    const actions = actionsMock()
    draw(permissionDoc({ viewer: VIEWERS.lee, view: { noteBox: { allowed: true, max: 500, mine: null } } }), actions)
    fireEvent.change(screen.getByTestId('permission-note-input'), { target: { value: 'see https://example.com' } })
    expect(text('permission-error')).toBe('No links.')
    expect((screen.getByTestId('permission-note-send') as HTMLButtonElement).disabled).toBe(true)
  })

  it('no note box when the daemon says no', () => {
    draw(permissionDoc({ viewer: VIEWERS.lee }))
    expect(screen.queryByTestId('permission-note-box')).toBeNull()
  })
})

describe('PermissionCard: Already on (§17.11.9)', () => {
  it('an approver gets Close, which answers it as a no-op', async () => {
    const actions = actionsMock()
    draw(permissionDoc({ options: ['Close'], view: { state: 'already_done' } }), actions)
    expect(screen.getByTestId('permission-card').getAttribute('data-state')).toBe('already_done')
    expect(text('permission-already-on')).toBe('Already on')
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    fireEvent.click(screen.getByTestId('permission-close'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: 'Close' }))
  })

  it('after Close: nothing was applied, no button', () => {
    draw(
      permissionDoc({
        options: ['Close'],
        choice: { status: 'answered', answer: 'Close' },
        result: { message: 'Already on. Nothing was applied.', outcome: 'already_done' },
        view: { state: 'already_done', answer: { decision: 'close', by: 'rosson', via: 'phone', at: 1761061200, reason: null } },
      }),
    )
    expect(text('permission-state-line')).toBe('Already on. Nothing was applied.')
    expect(screen.queryByTestId('permission-close')).toBeNull()
    expect(screen.queryByTestId('permission-expiry')).toBeNull()
  })

  it('a member sees "Already on" and nothing to press', () => {
    draw(permissionDoc({ viewer: VIEWERS.lee, view: { state: 'already_done' } }))
    expect(text('permission-state-line')).toBe('Already on')
    expect(screen.queryByTestId('permission-close')).toBeNull()
  })
})

describe('PermissionCard: closed states (§17.2.4)', () => {
  it('approved: who, from where, when, what K2 did, and Turn off again', async () => {
    const actions = actionsMock()
    draw(permissionDoc(APPROVED), actions)
    expect(screen.getByTestId('permission-card').getAttribute('data-state')).toBe('approved')
    expect(text('permission-state-line')).toBe(`Allowed by you, from the phone, ${permissionTime(1761061200)}`)
    expect(text('permission-result')).toBe('Allowed. Turn it off again in Settings → Workspaces → billing → Agent.')
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    fireEvent.click(screen.getByTestId('permission-undo'))
    await waitFor(() => expect(actions.undo).toHaveBeenCalledTimes(1))
  })

  it('approved, seen by the asker: by name, no undo', () => {
    draw(permissionDoc({ ...APPROVED, viewer: VIEWERS.dana, view: { ...APPROVED.view, undo: null } }))
    expect(text('permission-state-line')).toBe(`Allowed by rosson, from the phone, ${permissionTime(1761061200)}`)
    expect(screen.queryByTestId('permission-undo')).toBeNull()
    expect(screen.queryByTestId('permission-note-box')).toBeNull()
  })

  it('undo is gone once its 7 days passed', () => {
    const doc = permissionDoc({ ...APPROVED, view: { ...APPROVED.view, undo: { label: 'Turn off again', until: NOW - 1 } } })
    draw(doc)
    expect(screen.queryByTestId('permission-undo')).toBeNull()
  })

  it('denied: who, and the reason', () => {
    draw(
      permissionDoc({
        choice: { status: 'answered', answer: 'Deny' },
        view: { state: 'denied', answer: { decision: 'deny', by: 'maya', via: 'desktop', at: 1761061200, reason: 'Not this week' } },
      }),
    )
    expect(text('permission-state-line')).toBe('Denied by maya')
    expect(text('permission-deny-reason-shown')).toBe('“Not this week”')
  })

  it('expired, stale, failed, cancelled and closed draw their lines and no buttons', () => {
    const cases: Array<[string, string, Record<string, unknown> | null]> = [
      ['expired', 'Expired unanswered', null],
      ['stale', 'Already changed; nothing applied', null],
      ['failed', 'Couldn’t apply: the node is offline', { message: 'the node is offline', outcome: 'failed' }],
      ['cancelled', 'Withdrawn by billing', null],
      ['closed', 'Closed: permission cards were turned off', null],
    ]
    for (const [state, line, result] of cases) {
      cleanup()
      draw(permissionDoc({ choice: { status: state === 'expired' ? 'expired' : 'answered' }, view: { state }, result }))
      expect([state, text('permission-state-line')]).toEqual([state, line])
      expect([state, screen.queryByTestId('permission-allow')]).toEqual([state, null])
      expect([state, screen.queryByTestId('permission-expiry')]).toEqual([state, null])
    }
  })

  it('applying: no buttons while K2 applies it', () => {
    draw(permissionDoc({ view: { state: 'applying' } }))
    expect(text('permission-state-line')).toBe('Applying…')
    expect(screen.queryByTestId('permission-allow')).toBeNull()
  })
})

describe('PermissionBatchCard (§17.11.12, §4.5)', () => {
  it('lists every item; Allow all hides while a confirm item is pending; Deny all stays', () => {
    draw(batchDoc())
    expect(screen.getByTestId('permission-batch-card').getAttribute('data-state')).toBe('pending')
    expect(text('permission-batch-headline')).toBe('billing asks for 3 permissions.')
    expect(screen.getAllByTestId('permission-batch-item').map((e) => e.getAttribute('data-request'))).toEqual(['pr_a1', 'pr_a2', 'ar_23'])
    expect(screen.queryByTestId('permission-allow-all')).toBeNull()
    expect(text('permission-deny-all')).toBe('Deny all')
    expect(screen.getAllByTestId('permission-allow').map((b) => b.textContent)).toEqual([
      'Allow',
      'Allow',
      'Allow (needs your confirm)',
    ])
    expect(text('permission-batch-confirm')).toBe('needs your confirm')
  })

  it('one item answers with its request id', async () => {
    const actions = actionsMock()
    draw(batchDoc(), actions)
    fireEvent.click(screen.getAllByTestId('permission-allow')[1])
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow', item: 'pr_a2' }))
    // The confirm item: the sheet, then the token, still for that item.
    fireEvent.click(screen.getAllByTestId('permission-allow')[2])
    fireEvent.click(screen.getByTestId('mock-confirm-ok'))
    await waitFor(() => expect(actions.answer).toHaveBeenLastCalledWith({ answer: 'Allow', confirmToken: 'tok_1', item: 'ar_23' }))
  })

  it('Allow all (N) counts the pending items and sends the exact label', async () => {
    const actions = actionsMock()
    draw(batchDoc({ canAnswerAll: true }), actions)
    expect(text('permission-allow-all')).toBe('Allow all (3)')
    fireEvent.click(screen.getByTestId('permission-allow-all'))
    await waitFor(() => expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow all' }))
    fireEvent.click(screen.getByTestId('permission-deny-all'))
    await waitFor(() => expect(actions.answer).toHaveBeenLastCalledWith({ answer: 'Deny all' }))
  })

  it('each item keeps its own state; a waiting person gets no buttons', () => {
    const doc = batchDoc({ viewer: { ...VIEWERS.lee, canAnswerAll: false } })
    draw(doc)
    expect(screen.queryByTestId('permission-deny-all')).toBeNull()
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    expect(screen.getAllByTestId('permission-waiting')[0].textContent).toBe('billing asked an owner to turn on Build and run Apps.')
    expect(text('permission-approvers')).toBe('Who can answer: owner')
  })
})

describe('A3’s real projection details', () => {
  it('the owner token (desktop): viewer "owner", answered by "owner" reads as you, from the desktop', () => {
    const owner = { login: 'owner', role: 'owner', via: 'owner-token', mode: 'approve', canAnswer: true, isAsker: false }
    draw(
      permissionDoc({
        ...APPROVED,
        viewer: owner,
        view: { ...APPROVED.view, answer: { decision: 'allow', by: 'owner', via: 'desktop', at: 1761061200, reason: null } },
      }),
    )
    expect(text('permission-state-line')).toBe(`Allowed by you, from the desktop, ${permissionTime(1761061200)}`)
  })

  it('buttons follow the options A3 offers this viewer: none offered, none drawn', () => {
    draw(permissionDoc({ options: [] }))
    expect(screen.queryByTestId('permission-allow')).toBeNull()
    expect(screen.queryByTestId('permission-deny')).toBeNull()
  })

  it('a batch item’s note and Turn off again name the item', async () => {
    const actions = actionsMock()
    const waiting = batchDoc({ viewer: { ...VIEWERS.lee, canAnswerAll: false } })
    const items = (waiting.choice as { action: { view: { items: Record<string, unknown>[] } } }).action.view.items
    items[0].noteBox = { allowed: true, max: 500, mine: null }
    draw(waiting, actions)
    fireEvent.change(screen.getByTestId('permission-note-input'), { target: { value: 'Needed for the launch' } })
    fireEvent.click(screen.getByTestId('permission-note-send'))
    await waitFor(() => expect(actions.note).toHaveBeenCalledWith('Needed for the launch', 'pr_a1'))
    cleanup()
    const done = batchDoc({ state: 'done' })
    const doneItems = (done.choice as { action: { view: { items: Record<string, unknown>[] } } }).action.view.items
    Object.assign(doneItems[1], APPROVED.view)
    draw(done, actions)
    fireEvent.click(screen.getByTestId('permission-undo'))
    await waitFor(() => expect(actions.undo).toHaveBeenCalledWith('pr_a2'))
  })
})

describe('routing and the wire', () => {
  it('the fallback (no view) stays an ordinary choice card', () => {
    const c = choiceOf(FALLBACK_CHOICE)
    expect('permission' in c).toBe(false)
    render(<ChoiceCard options={c.options} allowCustom={false} status="pending" permission={c.permission} onPick={() => undefined} />)
    expect(screen.getByTestId('thread-choice-card')).toBeTruthy()
    expect(screen.queryByTestId('permission-card')).toBeNull()
  })

  it('a socket frame carries the view as a GET page does', () => {
    const doc = permissionDoc({})
    const items = applyOverlayFrame([], { collection: 'thread', seq: 412, id: 'tc_7f3a2c90', doc: doc as never }, 0)
    const fromFrame = items[0].doc.choice?.permission
    expect(fromFrame).toEqual(choiceOf(doc).permission)
    expect(fromFrame?.view.kind).toBe('permission')
  })

  it('an unreadable view (another version) falls back to the plain card', () => {
    const doc = permissionDoc({ view: { v: 2 } })
    expect('permission' in choiceOf(doc)).toBe(false)
    const badState = permissionDoc({ view: { state: 'mystery' } })
    expect('permission' in choiceOf(badState)).toBe(false)
  })
})
