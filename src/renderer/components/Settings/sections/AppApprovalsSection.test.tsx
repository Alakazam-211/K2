// @vitest-environment jsdom

// Settings → Cores → App Approvals (0.46.1 door): held App requests show in
// plain words; Approve always runs a fresh vault owner confirm (`app-approve`,
// bound to THAT request id) before POST /cli/app/approve with the token;
// Deny posts the note; with no way to confirm on the box (AR43) the daemon's
// hint shows and Approve is off; daemon refusals show on the card.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

// 0.46.5: the section also reads every other server the window owns (the
// Home pool). None here: only the window's server, as before.
vi.mock('@/lib/host-pool-instance', () => ({
  hostPool: { store: { getState: () => ({ entries: {} }), subscribe: () => () => {} } },
}))

import type { AppRequest } from './app-approvals-api'
import { cardLine, diffLines, expiryText } from './app-approvals-api'
import { AppApprovalsSection, APP_APPROVALS_MANIFEST } from './AppApprovalsSection'

const now = (): number => Math.floor(Date.now() / 1000)

function req(over: Partial<AppRequest> = {}): AppRequest {
  return {
    id: 'w_dep1',
    kind: 'deploy',
    appId: 'app-shop',
    serviceId: 'svc-shop',
    appLabel: 'shop',
    workspaceId: 'ws-sales',
    workspaceHandle: 'sales',
    body: {},
    diff: { caps: { add: ['mail:send'], approvedAfter: ['thread:read', 'mail:send'] }, lines: ['+ cap mail:send'], first: false },
    summary: 'deploy shop with 1 new cap',
    reasons: ['a deploy that adds caps is the owner’s'],
    reason: 'the shop sends receipts',
    requestedBy: 'agent:sales',
    requesterWorkspace: 'ws-sales',
    requestedAt: now() - 120,
    expiresAt: now() + 6 * 86400,
    status: 'pending',
    decidedBy: null,
    decidedAt: null,
    confirmVia: null,
    decisionReason: null,
    result: null,
    ...over,
  }
}

const CONFIRM_OK = { available: true, methods: ['touch-id', 'login-password'] }

/** Answer GET app/approvals per state; anything else fails the test. */
function serve(pending: AppRequest[], all: AppRequest[] = pending, ownerConfirm: unknown = CONFIRM_OK): void {
  h.daemonCliGet.mockImplementation(async (route: string, params?: { state?: string }) => {
    if (route !== 'app/approvals') throw new Error(`unexpected GET ${route}`)
    if (params?.state === 'pending') return { ok: true, requests: pending, ownerConfirm }
    if (params?.state === 'all') return { ok: true, requests: all, ownerConfirm }
    throw new Error(`unexpected state ${String(params?.state)}`)
  })
}

/** POST routes by name; an unmapped route fails the test. */
function posts(map: Record<string, (body: unknown) => unknown>): void {
  h.daemonCliPost.mockImplementation(async (route: string, body: unknown) => {
    const f = map[route]
    if (!f) throw new Error(`unexpected POST ${route}`)
    return f(body)
  })
}

const postsTo = (route: string): unknown[] =>
  h.daemonCliPost.mock.calls.filter((c) => c[0] === route).map((c) => c[1])

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})
afterEach(() => cleanup())

describe('AppApprovalsSection', () => {
  it('lists pending requests in plain words', async () => {
    serve([req(), req({ id: 'w_inst', kind: 'install', diff: { caps: { add: ['files:read'], approvedAfter: ['files:read'] }, first: true }, summary: 'install notes' })])
    render(<AppApprovalsSection />)
    const card = await screen.findByTestId('app-request-w_dep1')
    expect(card.textContent).toContain('Deploy')
    expect(card.textContent).toContain('shop')
    expect(card.textContent).toContain('sales')
    expect(card.textContent).toContain('deploy shop with 1 new cap')
    expect(card.textContent).toContain('New permission:')
    expect(card.textContent).not.toContain('+ cap')
    expect(card.textContent).toContain('asked by agent:sales')
    expect(card.textContent).toContain('Why: the shop sends receipts')
    expect(card.textContent).toContain('Held because: a deploy that adds caps is the owner’s')
    expect(card.textContent).toContain('2m ago')
    expect(card.textContent).toContain('expires in 6d')
    const install = screen.getByTestId('app-request-w_inst')
    expect(install.textContent).toContain('First install')
    expect(install.textContent).toContain('New permission: Read files')
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('App Approvals (2)')
    expect(h.daemonCliGet).toHaveBeenCalledWith('app/approvals', { state: 'pending' })
    expect(screen.queryByTestId('app-approvals-no-confirm')).toBeNull()
  })

  it('approves after a Touch ID confirm bound to that request, then posts the token', async () => {
    let approved = false
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route !== 'app/approvals') throw new Error(`unexpected GET ${route}`)
      return { ok: true, requests: approved ? [] : [req()], ownerConfirm: CONFIRM_OK }
    })
    posts({
      'vault/confirm/start': () => ({ methods: ['local-auth', 'vault-password'], expiresIn: 60 }),
      'vault/confirm/finish': () => ({ token: 'k2vc_tok', expiresIn: 60 }),
      'app/approve': () => {
        approved = true
        return { ok: true, status: 'approved', request: req({ status: 'approved' }), note: 'approved; it applies now' }
      },
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    // Asks first: the dialog shows the change; nothing is sent until Approve.
    const dialog = await screen.findByTestId('app-approve-dialog-w_dep1')
    expect(dialog.textContent).toContain('New permission:')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.click(screen.getByTestId('app-approve-confirm-w_dep1'))
    await waitFor(() => expect(postsTo('app/approve')).toEqual([{ request: 'w_dep1', confirmToken: 'k2vc_tok' }]))
    expect(postsTo('vault/confirm/start')).toEqual([{ action: 'app-approve', secretId: 'w_dep1' }])
    expect(postsTo('vault/confirm/finish')).toEqual([
      { action: 'app-approve', secretId: 'w_dep1', method: 'local-auth', label: 'shop: deploy shop with 1 new cap' },
    ])
    // Confirm strictly before approve.
    const order = h.daemonCliPost.mock.calls.map((c) => c[0])
    expect(order).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'app/approve'])
    await waitFor(() => expect(screen.queryByTestId('app-request-w_dep1')).toBeNull())
    expect(screen.getByTestId('app-approvals-notice').textContent).toBe('Approved: shop can now deploy shop with 1 new cap.')
  })

  it('asks for a password where there is no Touch ID, and never approves without it', async () => {
    serve([req()])
    posts({
      'vault/confirm/start': () => ({ methods: ['vault-password'], expiresIn: 60 }),
      'vault/confirm/finish': () => ({ token: 'k2vc_pw', expiresIn: 60 }),
      'app/approve': () => ({ ok: true, status: 'approved', request: req({ status: 'approved' }) }),
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    fireEvent.click(await screen.findByTestId('app-approve-confirm-w_dep1'))
    const pw = (await screen.findByTestId('app-approve-password')) as HTMLInputElement
    expect(postsTo('app/approve')).toEqual([])
    expect(screen.getByText('Your vault password')).toBeTruthy()
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.submit(pw.closest('form') as HTMLFormElement)
    await waitFor(() => expect(postsTo('app/approve')).toEqual([{ request: 'w_dep1', confirmToken: 'k2vc_pw' }]))
    expect(postsTo('vault/confirm/finish')).toEqual([
      {
        action: 'app-approve',
        secretId: 'w_dep1',
        method: 'vault-password',
        label: 'shop: deploy shop with 1 new cap',
        password: 'correct horse battery',
      },
    ])
    expect(pw.value).toBe('')
  })

  it('a password-only server asks for the password in the same dialog: Approve… then Approve', async () => {
    serve([req()], [req()], { available: true, methods: ['vault-password'] })
    posts({
      'vault/confirm/start': () => ({ methods: ['vault-password'], expiresIn: 60 }),
      'vault/confirm/finish': () => ({ token: 'k2vc_pw', expiresIn: 60 }),
      'app/approve': () => ({ ok: true, status: 'approved', request: req({ status: 'approved' }) }),
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    const pw = (await screen.findByTestId('app-approve-password')) as HTMLInputElement
    expect(screen.getByTestId('app-approve-dialog-w_dep1').textContent).toContain('New permission:')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.click(screen.getByTestId('app-approve-submit-w_dep1'))
    await waitFor(() => expect(postsTo('app/approve')).toEqual([{ request: 'w_dep1', confirmToken: 'k2vc_pw' }]))
    expect(h.daemonCliPost.mock.calls.map((c) => c[0])).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'app/approve'])
  })

  it('a wrong password says so plainly and keeps the field for another try', async () => {
    serve([req()], [req()], { available: true, methods: ['vault-password'] })
    posts({
      'vault/confirm/start': () => ({ methods: ['vault-password'], expiresIn: 60 }),
      'vault/confirm/finish': () => {
        throw new Error('{"error":{"code":"confirm_failed","hint":"that isn\'t the vault password"}}')
      },
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    const pw = (await screen.findByTestId('app-approve-password')) as HTMLInputElement
    fireEvent.change(pw, { target: { value: 'wrong' } })
    fireEvent.click(screen.getByTestId('app-approve-submit-w_dep1'))
    expect(await screen.findByText('That password didn’t match. Try again.')).toBeTruthy()
    expect(screen.getByTestId('app-approve-password')).toBeTruthy()
    expect(postsTo('app/approve')).toEqual([])
  })

  // 0.46.13 (Rosson 2026-11-15): an Admin login approves too, with its own
  // K2 Connect password. The daemon says so on the list (canApprove, and
  // ownerConfirm with only connect-password for an Admin).
  it('an Admin login sees what it decides on and approves with its own login password', async () => {
    let approved = false
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route !== 'app/approvals') throw new Error(`unexpected GET ${route}`)
      return {
        ok: true,
        requests: approved ? [] : [req()],
        canApprove: true,
        ownerConfirm: { available: true, methods: ['connect-password'], server: 'rosson.k2.dev', login: 'ada' },
      }
    })
    posts({
      'vault/confirm/start': () => ({ methods: ['connect-password'], expiresIn: 60, server: 'rosson.k2.dev', login: 'ada' }),
      'vault/confirm/finish': () => ({ token: 'k2vc_admin', expiresIn: 60 }),
      'app/approve': () => {
        approved = true
        return { ok: true, status: 'approved', request: req({ status: 'approved', decidedBy: 'user:ada', confirmVia: 'connect-password' }) }
      },
    })
    render(<AppApprovalsSection />)
    // Everything it needs to decide: the change, who asked and why, why it was held.
    const card = await screen.findByTestId('app-request-w_dep1')
    expect(card.textContent).toContain('New permission:')
    expect(card.textContent).toContain('asked by agent:sales for sales')
    expect(card.textContent).toContain('Why: the shop sends receipts')
    expect(card.textContent).toContain('Held because:')
    expect(screen.queryByTestId('app-approvals-no-confirm')).toBeNull()
    expect(screen.queryByTestId('app-approvals-approvers-only')).toBeNull()
    const approve = screen.getByTestId('app-approve-w_dep1') as HTMLButtonElement
    expect(approve.disabled).toBe(false)
    fireEvent.click(approve)
    // Its own login password, asked for at once, naming this server and the
    // login (0.46.13 field report: "K2 Connect password" read as the k2.dev
    // account's); the password manager gets the login as the username.
    const pw = (await screen.findByTestId('app-approve-password')) as HTMLInputElement
    expect(screen.getByText('Your sign-in password for rosson.k2.dev (login ada)')).toBeTruthy()
    const user = screen.getByTestId('app-approve-username') as HTMLInputElement
    expect([user.value, user.getAttribute('autocomplete'), user.hidden, user.form]).toEqual(['ada', 'username', true, pw.form])
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.change(pw, { target: { value: 'admins own password' } })
    fireEvent.click(screen.getByTestId('app-approve-submit-w_dep1'))
    await waitFor(() => expect(postsTo('app/approve')).toEqual([{ request: 'w_dep1', confirmToken: 'k2vc_admin' }]))
    expect(postsTo('vault/confirm/finish')).toEqual([
      {
        action: 'app-approve',
        secretId: 'w_dep1',
        method: 'connect-password',
        label: 'shop: deploy shop with 1 new cap',
        password: 'admins own password',
      },
    ])
    expect(h.daemonCliPost.mock.calls.map((c) => c[0])).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'app/approve'])
    expect(pw.value).toBe('')
  })

  it('a Member login reads and denies but gets no Approve button', async () => {
    h.daemonCliGet.mockImplementation(async () => ({ ok: true, requests: [req()], canApprove: false, ownerConfirm: { available: false, methods: [] } }))
    render(<AppApprovalsSection />)
    const card = await screen.findByTestId('app-request-w_dep1')
    expect(card.textContent).toContain('deploy shop with 1 new cap')
    expect(screen.queryByTestId('app-approve-w_dep1')).toBeNull()
    expect(screen.getByTestId('app-approvals-approvers-only').textContent).toBe('Only the server’s owner or an admin approves App requests.')
    // Not the "no way to confirm" banner: the server can, this login may not.
    expect(screen.queryByTestId('app-approvals-no-confirm')).toBeNull()
    expect((screen.getByTestId('app-deny-w_dep1') as HTMLButtonElement).disabled).toBe(false)
  })

  it('deny posts the note the requester reads', async () => {
    serve([req()])
    posts({ 'app/deny': () => ({ ok: true, status: 'denied', request: req({ status: 'denied' }) }) })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-deny-w_dep1'))
    // Deny asks first, in a dialog with the optional note.
    expect(screen.getByTestId('app-deny-dialog-w_dep1')).toBeTruthy()
    const note = screen.getByTestId('app-deny-note-w_dep1') as HTMLTextAreaElement
    expect(note.maxLength).toBe(500)
    fireEvent.change(note, { target: { value: '  no mail from the shop yet  ' } })
    fireEvent.click(screen.getByTestId('app-deny-send-w_dep1'))
    await waitFor(() => expect(postsTo('app/deny')).toEqual([{ request: 'w_dep1', reason: 'no mail from the shop yet' }]))
    expect(postsTo('vault/confirm/start')).toEqual([])
    expect(await screen.findByTestId('app-approvals-notice')).toBeTruthy()
  })

  it('deny without a note sends no reason', async () => {
    serve([req()])
    posts({ 'app/deny': () => ({ ok: true, status: 'denied' }) })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-deny-w_dep1'))
    fireEvent.click(screen.getByTestId('app-deny-send-w_dep1'))
    await waitFor(() => expect(postsTo('app/deny')).toEqual([{ request: 'w_dep1' }]))
  })

  it('shows the daemon hint and turns Approve off when the box has no way to confirm (AR43)', async () => {
    const hint = 'no vault password and no Owner Connect login on this box: App approvals stay held until one is set (k2 vault password set)'
    serve([req()], [req()], { available: false, methods: [], hint })
    render(<AppApprovalsSection />)
    const banner = await screen.findByTestId('app-approvals-no-confirm')
    // Plain words and the next step, never the daemon's CLI hint.
    expect(banner.textContent).toContain('this server has no way to confirm it’s you')
    expect(banner.textContent).not.toContain('k2 vault')
    expect(screen.getByTestId('app-approvals-open-vault')).toBeTruthy()
    const approve = screen.getByTestId('app-approve-w_dep1') as HTMLButtonElement
    expect(approve.disabled).toBe(true)
    fireEvent.click(approve)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    // Deny still works.
    expect((screen.getByTestId('app-deny-w_dep1') as HTMLButtonElement).disabled).toBe(false)
  })

  it('shows a daemon refusal inline on the card', async () => {
    serve([req()])
    posts({
      'vault/confirm/start': () => ({ methods: ['local-auth'], expiresIn: 60 }),
      'vault/confirm/finish': () => ({ token: 't', expiresIn: 60 }),
      'app/approve': () => {
        throw new Error('{"error":{"code":"owner_only","hint":"only the server\'s owner or an admin approves App requests, in person"}}')
      },
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    fireEvent.click(await screen.findByTestId('app-approve-confirm-w_dep1'))
    const err = await screen.findByTestId('app-request-error-w_dep1')
    expect(err.textContent).toBe("only the server's owner or an admin approves App requests, in person")
    // The request is still there, waiting.
    expect(screen.getByTestId('app-approve-w_dep1')).toBeTruthy()
  })

  it('an approval that failed to apply says so', async () => {
    let approved = false
    h.daemonCliGet.mockImplementation(async () => ({ ok: true, requests: approved ? [] : [req({ kind: 'db_raw' })], ownerConfirm: CONFIRM_OK }))
    posts({
      'vault/confirm/start': () => ({ methods: ['local-auth'], expiresIn: 60 }),
      'vault/confirm/finish': () => ({ token: 't', expiresIn: 60 }),
      'app/approve': () => {
        approved = true
        return { ok: false, status: 'failed', request: req({ status: 'failed' }), error: { code: 'no_applier', hint: "nothing on this daemon applies 'db_raw' requests yet" } }
      },
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_dep1'))
    fireEvent.click(await screen.findByTestId('app-approve-confirm-w_dep1'))
    const notice = await screen.findByTestId('app-approvals-notice')
    expect(notice.textContent).toContain("Approved, but it didn’t apply: nothing on this daemon applies 'db_raw' requests yet.")
  })

  it('shows a load failure', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('{"error":{"code":"unauthorized","hint":"sign in again"}}'))
    render(<AppApprovalsSection />)
    const card = await screen.findByRole('alert')
    expect(card.textContent).toContain('K2 couldn’t read the waiting requests')
    expect(card.textContent).toContain('The server said: sign in again.')
  })

  it('History shows decided requests from state=all', async () => {
    const denied = req({ id: 'w_old', kind: 'egress', diff: { egress: { add: ['api.stripe.com'] } }, hosts: ['api.stripe.com:443'], status: 'denied', decidedBy: 'owner', decidedAt: now() - 3600, decisionReason: 'not now', summary: 'let shop reach api.stripe.com' })
    const failed = req({ id: 'w_fail', status: 'failed', decidedBy: 'owner', confirmVia: 'local-auth', decidedAt: now() - 60, result: { code: 'no_applier', hint: 'nothing applies it yet' } })
    serve([req()], [req(), denied, failed])
    render(<AppApprovalsSection />)
    await screen.findByTestId('app-request-w_dep1')
    expect(screen.queryByTestId('app-approvals-history')).toBeNull()
    fireEvent.click(screen.getByTestId('app-approvals-history-toggle'))
    const row = await screen.findByTestId('app-history-w_old')
    expect(h.daemonCliGet).toHaveBeenCalledWith('app/approvals', { state: 'all' })
    expect(row.textContent).toContain('Denied')
    expect(row.textContent).toContain('by owner')
    expect(row.textContent).toContain('Note: not now')
    const f = screen.getByTestId('app-history-w_fail')
    expect(f.textContent).toContain('Approved, failed to apply')
    expect(f.textContent).toContain('by owner with Touch ID or the Mac password')
    expect(f.textContent).toContain('nothing applies it yet')
    // The pending one is not a history row; it waits on the Waiting tab.
    expect(screen.queryByTestId('app-history-w_dep1')).toBeNull()
    fireEvent.click(screen.getByRole('tab', { name: /Waiting/ }))
    expect(await screen.findByTestId('app-request-w_dep1')).toBeTruthy()
  })

  it('reloads when the window regains focus', async () => {
    serve([req()])
    render(<AppApprovalsSection />)
    await screen.findByTestId('app-request-w_dep1')
    const before = h.daemonCliGet.mock.calls.length
    fireEvent.focus(window)
    await waitFor(() => expect(h.daemonCliGet.mock.calls.length).toBe(before + 1))
  })

  it('has a search entry on its own page', () => {
    expect(APP_APPROVALS_MANIFEST.map((e) => e.section)).toEqual(['app-approvals'])
  })
})

describe('app approvals plain words', () => {
  it('diffLines covers each diff shape', () => {
    expect(diffLines({ diff: ['set PORT=8080', 'drop DEBUG'] })).toEqual(['set PORT=8080', 'drop DEBUG'])
    expect(diffLines({ diff: { caps: { add: ['a', 'b'], approvedAfter: ['a', 'b'] }, first: true } })).toEqual(['First install', '+ cap a', '+ cap b'])
    expect(diffLines({ diff: { lines: ['k2 app restart shop'], route: '/cli/app/restart' } })).toEqual(['k2 app restart shop'])
    expect(diffLines({ diff: { quotas: { from: { rps: 5, bodyBytes: 1 }, to: { rps: 50, bodyBytes: 1 } }, lines: [] } })).toEqual(['rps: 5 → 50'])
    expect(diffLines({ diff: { egress: { add: ['api.stripe.com'] } } })).toEqual(['+ host api.stripe.com'])
    expect(diffLines({ diff: { egress: { add: ['x'] } }, hosts: ['api.stripe.com:443'] })).toEqual(['+ host api.stripe.com:443'])
    expect(diffLines({ diff: { egressMode: 'broad' } })).toEqual(['Egress mode → broad'])
  })

  // 0.46.17: a held request's permission card, and who it was sent to.
  it('cardLine says where the card is and who it asks, while it waits', () => {
    const c = { id: 'w_dep1', state: 'pending', card: 'c1', thread: 'sales', routedTo: [{ login: 'ada', role: 'admin' }], narrowed: true }
    expect(cardLine(c)).toBe("Card on sales's Thread, sent to ada")
    expect(cardLine({ ...c, routedTo: [{ login: 'owner', role: 'owner' }] })).toBe("Card on sales's Thread, sent to the owner")
    expect(cardLine({ ...c, narrowed: false, routedTo: [{ login: 'ada', role: 'admin' }, { login: 'bo', role: 'owner' }] })).toBe(
      "Card on sales's Thread, for every owner and admin",
    )
    expect(cardLine({ ...c, state: 'approved' })).toBeNull()
    expect(cardLine(null)).toBeNull()
    expect(cardLine(undefined)).toBeNull()
  })

  it('a row shows its card line', async () => {
    serve([req({ card: { id: 'w_dep1', state: 'pending', card: 'c1', thread: 'sales', routedTo: [{ login: 'ada', role: 'admin' }], narrowed: true } })])
    render(<AppApprovalsSection />)
    expect((await screen.findByTestId('app-request-card')).textContent).toBe("Card on sales's Thread, sent to ada")
  })

  it('expiryText counts down and says expired', () => {
    expect(expiryText(1000 + 6 * 86400, 1000)).toBe('expires in 6d')
    expect(expiryText(1000 + 5 * 3600, 1000)).toBe('expires in 5h')
    expect(expiryText(1000 + 120, 1000)).toBe('expires in 2m')
    expect(expiryText(999, 1000)).toBe('expired')
  })
})
