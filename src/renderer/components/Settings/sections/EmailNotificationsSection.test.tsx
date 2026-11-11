// @vitest-environment jsdom
// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8): the page only reads `/cli/notifications/email/*` and sends the
// owner's gestures, each behind a confirm. A paused sender shows why and
// resumes after a vault confirm bound to its address; the sender for K2's
// own email is never "paused"; stopped addresses are found by address and
// never shown as addresses; an answer without senders or a refusal shows
// the shared load state, never a crash.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  methods: ['local-auth'] as string[],
  paused: true,
  status: null as unknown,
  statusFail: null as Error | null,
  postFail: null as { route: string; err: Error } | null,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
// Agents are named by their workspace, as the rest of K2 names them.
vi.mock('@/stores/projects', () => {
  const state = { projects: [{ id: 'p_sales', name: 'sales', handle: 'sales' }] }
  return { useProjectsStore: (sel: (s: typeof state) => unknown) => sel(state) }
})
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  const shop = {
    id: 'ens_shop',
    address: 'noreply@notify.brand.test',
    domain: 'notify.brand.test',
    path: 'hosted',
    owner: 'app:svc_shop',
    app: 'svc_shop',
    fromName: 'Shop',
    state: 'ready',
    envelope: null,
    paused: null as unknown,
  }
  const sys = { ...shop, id: 'ens_sys', address: 'system@notify.brand.test', owner: 'system', app: null, fromName: 'K2', paused: null }
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'notifications/email') {
        if (h.statusFail) throw h.statusFail
        if (h.status !== null) return h.status
        return {
          ok: true,
          boxSwitch: 'on',
          hostedMail: true,
          airgap: false,
          systemSender: sys,
          senders: [
            sys,
            { ...shop, paused: h.paused ? { pausedAt: 1_800_000_000, reason: 'hard_bounces', rate: { sent: 80, hardBounces: 7, complaints: 0 } } : null },
          ],
          relays: [],
          settings: { logRetentionDays: 30, ownerEmail: null },
        }
      }
      if (route === 'notifications/email/agents') return { ok: true, agents: [{ workspace: 'p_sales', sender: 'notices@notify.brand.test', on: true, offReason: null, grantedAt: 1 }] }
      if (route === 'notifications/email/suppressions') {
        return { ok: true, suppressions: [{ addressSha256: 'ab'.repeat(32), domain: 'x.test', scope: 'box', scopeRef: null, reason: 'hard_bounce', createdAt: 1, createdBy: 'dsn' }] }
      }
      if (route === 'notifications/email/log') return { ok: true, items: [{ id: 'enl_1', sender: 'ens_shop', to: ['a@x.test'], recipients: 1, subject: 'Your order', status: 'submitted', origin: 'app', purpose: null, error: null, createdAt: 1 }] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (h.postFail && h.postFail.route === route) throw h.postFail.err
      if (route === 'vault/confirm/start') return { methods: h.methods, expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'notifications/email/sender/resume') {
        h.paused = false
        return { ok: true, sender: body.sender, paused: false }
      }
      if (route === 'notifications/email/sender/rm') return { ok: true, removed: body.sender, address: 'noreply@notify.brand.test', reservedDays: 30 }
      if (route === 'notifications/email/agent/off') return { ok: true }
      if (route === 'notifications/email/suppressions/rm') return { ok: true, removed: 1 }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { EmailNotificationsSection } from './EmailNotificationsSection'
import { primaryScope } from '@/kessel/server-scope'
import {
  logStatusText,
  logWhatText,
  offReasonText,
  ownerText,
  pauseText,
  pickMethod,
  readNotifyStatus,
  senderStateText,
  suppressionScopeText,
  NotifyUnreadableError,
} from './email-notify-api'

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.methods = ['local-auth']
  h.paused = true
  h.status = null
  h.statusFail = null
  h.postFail = null
})
afterEach(() => cleanup())

const page = async (): Promise<void> => {
  render(<EmailNotificationsSection scope={primaryScope()} />)
  await screen.findByTestId('email-notifications')
}

describe('EmailNotificationsSection', () => {
  it('shows a paused App sender with why, and resumes it after a confirm bound to its address', async () => {
    await page()
    expect(screen.getByTestId('notify-paused-ens_shop').textContent).toContain('7 of its 80 emails in a day bounced')
    expect(screen.getByText(/never paused: you get a warning instead/)).toBeTruthy()
    // The sender for K2's own email has no resume and no menu.
    expect(screen.queryByTestId('notify-resume-ens_sys')).toBeNull()
    expect(screen.queryByTestId('notify-more-ens_sys')).toBeNull()
    fireEvent.click(screen.getByTestId('notify-resume-ens_shop'))
    const dialog = await screen.findByTestId('notify-resume-dialog')
    expect(dialog.textContent).toContain('K2 paused it because 7 of its 80 emails in a day bounced')
    // Nothing is sent until the confirm.
    expect(h.posts).toEqual([])
    fireEvent.click(screen.getByTestId('notify-resume-confirm'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/sender/resume')).toBe(true))
    const start = h.posts.find((p) => p.route === 'vault/confirm/start')
    expect(start?.body).toEqual({ action: 'email-resume', secretId: 'noreply@notify.brand.test' })
    const finish = h.posts.find((p) => p.route === 'vault/confirm/finish')
    expect(finish?.body.method).toBe('local-auth')
    const resume = h.posts.find((p) => p.route === 'notifications/email/sender/resume')
    expect(resume?.body).toEqual({ sender: 'noreply@notify.brand.test', confirmToken: 'tok-1' })
    expect((await screen.findByTestId('notify-done')).textContent).toContain('noreply@notify.brand.test is sending again')
    await waitFor(() => expect(screen.queryByTestId('notify-resume-ens_shop')).toBeNull())
    expect(screen.queryByTestId('notify-resume-dialog')).toBeNull()
  })

  it('asks for the vault password when the server offers no Touch ID', async () => {
    h.methods = ['vault-password']
    await page()
    fireEvent.click(screen.getByTestId('notify-resume-ens_shop'))
    fireEvent.click(await screen.findByTestId('notify-resume-confirm'))
    const pw = await screen.findByLabelText('Vault password')
    // Asking for the password is not an error.
    expect(screen.queryByTestId('dialog-error')).toBeNull()
    expect((screen.getByTestId('notify-resume-confirm') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByTestId('notify-resume-confirm'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/sender/resume')).toBe(true))
    const finish = h.posts.find((p) => p.route === 'vault/confirm/finish')
    expect(finish?.body).toMatchObject({ method: 'vault-password', password: 'hunter2' })
  })

  it('a refused resume keeps the dialog open with what failed and the server’s reason', async () => {
    h.postFail = { route: 'notifications/email/sender/resume', err: new Error('confirm again: a confirm works once') }
    await page()
    fireEvent.click(screen.getByTestId('notify-resume-ens_shop'))
    fireEvent.click(await screen.findByTestId('notify-resume-confirm'))
    const err = await screen.findByTestId('dialog-error')
    expect(err.textContent).toContain('Couldn’t resume noreply@notify.brand.test')
    expect(err.textContent).toContain('The server said: confirm again: a confirm works once.')
    expect(screen.getByTestId('notify-resume-dialog')).toBeTruthy()
  })

  it('turns a workspace off only after its confirm', async () => {
    await page()
    fireEvent.click(screen.getByTestId('notify-agent-off-p_sales'))
    const dialog = await screen.findByTestId('notify-agent-off-dialog')
    expect(dialog.textContent).toContain('Turn off email notices for sales?')
    expect(h.posts).toEqual([])
    fireEvent.click(screen.getByTestId('notify-agent-off-dialog-confirm'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/agent/off')?.body).toEqual({ workspace: 'p_sales' }))
    expect((await screen.findByTestId('notify-done')).textContent).toContain('Email notices for sales are off')
  })

  it('removes a sender from its menu after a confirm', async () => {
    await page()
    fireEvent.click(screen.getByTestId('notify-more-ens_shop'))
    fireEvent.click(await screen.findByRole('menuitem', { name: /Remove sender/ }))
    fireEvent.click(await screen.findByTestId('notify-remove-dialog-confirm'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/sender/rm')?.body).toEqual({ sender: 'ens_shop' }))
    expect((await screen.findByTestId('notify-done')).textContent).toContain('held for 30 days')
  })

  it('finds a stopped address and takes it off the list after a confirm, never showing addresses it keeps hashed', async () => {
    await page()
    expect(screen.getByText('An address at x.test')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Address'), { target: { value: 'gone@x.test' } })
    fireEvent.click(screen.getByTestId('notify-find'))
    await waitFor(() => expect(h.gets.at(-1)).toEqual({ route: 'notifications/email/suppressions', params: { address: 'gone@x.test', limit: 100 } }))
    fireEvent.click(await screen.findByTestId('notify-unsuppress'))
    const dialog = await screen.findByTestId('notify-unsuppress-dialog')
    expect(within(dialog).getByText('Let mail go to gone@x.test again?')).toBeTruthy()
    fireEvent.click(screen.getByTestId('notify-unsuppress-dialog-confirm'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/suppressions/rm')?.body).toEqual({ address: 'gone@x.test', all: true }))
    expect((await screen.findByTestId('notify-done')).textContent).toContain('gone@x.test is off the stop list')
  })

  it('an answer without senders shows the error card, not a crash, and Try again reads again', async () => {
    h.status = { ok: true }
    render(<EmailNotificationsSection scope={primaryScope()} />)
    const card = await screen.findByTestId('email-notifications-error')
    expect(card.getAttribute('data-load-state')).toBe('error')
    expect(card.textContent).toContain('sent an answer K2 couldn’t read')
    h.status = null
    fireEvent.click(screen.getByText('Try again'))
    await screen.findByTestId('email-notifications')
  })

  it('a failed read says so with the server’s reason; a refusal says who can see the page', async () => {
    h.statusFail = Object.assign(new Error('database is locked'), { status: 500 })
    render(<EmailNotificationsSection scope={primaryScope()} />)
    expect((await screen.findByTestId('email-notifications-error')).textContent).toContain('The server said: database is locked.')
    cleanup()
    h.statusFail = Object.assign(new Error('owner only'), { status: 403, code: 'owner_only' })
    render(<EmailNotificationsSection scope={primaryScope()} />)
    expect((await screen.findByTestId('email-notifications-not-allowed')).textContent).toContain('Only the owner and Admins can see this')
  })
})

describe('email-notify-api words', () => {
  it('names owners, states, pauses, scopes, log rows and confirm methods', () => {
    const base = { id: 'a', address: 'a@b', domain: 'b', path: 'hosted', fromName: 'Shop', state: 'ready', envelope: null, paused: null }
    expect(ownerText({ ...base, owner: 'system', app: null })).toBe('K2’s own email')
    expect(ownerText({ ...base, owner: 'app:svc', app: 'svc' })).toBe('The Shop App')
    expect(ownerText({ ...base, owner: null, app: null })).toBe('Not used by an App')
    expect(senderStateText('needs_dmarc')).toEqual({ text: 'Needs a DNS record', tone: 'warn' })
    expect(pauseText({ pausedAt: 1, reason: 'complaints', rate: { sent: 1000, complaints: 2 } })).toBe('2 of its 1000 emails in a day were reported as spam')
    const x = { addressSha256: 'x', domain: 'd', scope: 'sender', scopeRef: 'ens_1', reason: 'complaint', createdAt: 1, createdBy: 'arf' }
    expect(suppressionScopeText(x, [{ ...base, id: 'ens_1', address: 'n@d', owner: null, app: null }])).toBe('Stopped for n@d')
    expect(suppressionScopeText(x)).toBe('Stopped for one sender')
    expect(logWhatText({ subject: '', purpose: 'owner_reset', origin: 'system' })).toBe('Password reset')
    expect(logStatusText('refused').text).toBe('Not sent')
    expect(offReasonText('off by user:alex')).toBe('Turned off by alex')
    expect(pickMethod(['vault-password'])).toBeNull()
    expect(pickMethod(['vault-password', 'local-auth'])).toBe('local-auth')
  })

  it('reads a status answer safely: senders must be a list, the rest falls back', () => {
    expect(() => readNotifyStatus({})).toThrow(NotifyUnreadableError)
    expect(() => readNotifyStatus(null)).toThrow(NotifyUnreadableError)
    const st = readNotifyStatus({ senders: [{ id: 's', address: 'a@b' }, 'junk'] })
    expect(st.senders).toHaveLength(1)
    expect(st.senders[0].paused).toBeNull()
    expect(st.systemSender).toBeNull()
    expect(st.relays).toEqual([])
    expect(st.settings.logRetentionDays).toBe(30)
  })
})
