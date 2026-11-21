// @vitest-environment jsdom
// Settings → Email Link: a Google or Microsoft account whose saved sign-in
// was refused says "Can’t connect" on the list with Sign in again, and its
// page offers Reconnect. Reconnecting signs in to the SAME link with the
// features it already has (the daemon replaces only the saved tokens), so
// workspace access and the calendar setting stay. Nothing disconnects.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const tauri = vi.hoisted(() => ({ invoke: vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null) }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: tauri.invoke }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const opener = vi.hoisted(() => ({ openUrl: vi.fn(async (_url: string) => undefined) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: opener.openUrl }))
vi.mock('@/components/BrowserPane/BrowserPane', () => ({ BrowserPane: () => null }))
vi.mock('@/components/Room/PrimaryRoom', () => ({ PrimaryRoom: () => null }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: Array<{ id: string; name: string }> }) => unknown) =>
    sel({ projects: [{ id: 'p1', name: 'atlas' }] }),
}))

const api = vi.hoisted(() => ({
  fetchInboxes: vi.fn(),
  fetchOauthConfig: vi.fn(),
  linkOauthUpgrade: vi.fn(),
  linkOauthStatus: vi.fn(),
  linkOauthComplete: vi.fn(),
  removeLinkedInbox: vi.fn(),
  linkedCalendars: vi.fn(),
}))
vi.mock('./email-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./email-api')>()
  return { ...real, ...api }
})
const watch = vi.hoisted(() => ({ fetchMailWatch: vi.fn() }))
vi.mock('./mail-watch-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./mail-watch-api')>()
  return { ...real, fetchMailWatch: watch.fetchMailWatch }
})

import { EmailLinkSection } from './EmailLinkSection'
import { reconnectFeatures } from './EmailLinkReconnect'
import { parseInboxes, type Inbox } from './email-api'
import type { MailWatchStatus } from './mail-watch-api'

/** Rows as GET /cli/mail/inboxes sends them (run through the real parser). */
function row(address: string, source: 'google' | 'microsoft' | 'imap', features: string | null, status = 'connected'): unknown {
  return {
    address,
    source: 'linked',
    displayName: null,
    status,
    primary: { projectId: 'p1', workspace: 'atlas', level: 'draft', canManage: false, canDelete: false, agentSend: 'off', sendState: 'no_send_level', calendarOmitDescription: true },
    grants: [{ projectId: 'p2', workspace: 'release-bot', level: 'read', canManage: false, canDelete: false, agentSend: 'off', sendState: 'no_send_level', calendarOmitDescription: true }],
    yourLevel: null,
    yourAgentSend: null,
    yourSendState: null,
    maxLevel: 'send',
    host: source === 'imap' ? 'imap.example.com' : source === 'google' ? 'imap.gmail.com' : 'graph.microsoft.com',
    tls: 'implicit-tls',
    capabilities: {
      source,
      mail: { read: true, draft: true, compose: true, send: true },
      calendar: { read: features?.includes('calendar') ?? false, write: false, invite: false, rsvp: false, freebusy: 'none', scheduling: 'none' },
      missing: features?.includes('calendar')
        ? [{ cap: 'calendar.write', reason: 'read_only_link', fix: 'the link is read-only' }]
        : source === 'imap'
          ? [{ cap: 'calendar.read', reason: 'backend_unsupported', fix: 'no calendar API' }]
          : [],
      ...(source === 'imap' ? {} : { account: { kind: 'personal', tenant: null, checked: true, features, client: 'default' } }),
    },
  }
}

const GMAIL = 'pat@gmail.example'
const MS = 'sam@contoso.example'
const HEALTHY = 'lee@contoso.example'
const IMAP = 'ops@mail.example.com'

function inboxes(rows: unknown[]): Inbox[] {
  return parseInboxes({ ok: true, count: rows.length, inboxes: rows })
}

/** GET /cli/mail/watch for the primary workspace: which inboxes it hears
 *  about, and which ones' sign-in the watcher saw refused. */
function watchStatus(refused: string[], lastPollAt = '2026-11-20T09:00:00+00:00'): MailWatchStatus {
  return {
    enabled: true,
    vipOutsideWindow: false,
    snippets: false,
    vipSessionLine: true,
    pausedUntil: null,
    inboxes: [GMAIL, MS, HEALTHY, IMAP].map((address) => ({
      address,
      source: 'linked' as const,
      wake: 'vip' as const,
      primary: true,
      personMailbox: false,
      lastPollAt,
      lastError: refused.includes(address) ? 'needs_relink' : null,
      needsRelink: refused.includes(address),
    })),
    vips: [],
    digest: null,
    last24h: { vipDelivered: 0, digested: 0, failedAuth: 0 },
  }
}

const CFG = {
  providers: {
    gmail: { source: 'default' as const, secretSet: false, configured: true },
    microsoft: { source: 'default' as const, secretSet: false, configured: true },
  },
}

beforeEach(() => {
  for (const f of Object.values(api)) f.mockReset()
  watch.fetchMailWatch.mockReset()
  api.fetchOauthConfig.mockResolvedValue(CFG)
  api.linkedCalendars.mockResolvedValue({ access: 'read-only', calendars: [] })
  tauri.invoke.mockReset()
  tauri.invoke.mockImplementation(async () => null)
  opener.openUrl.mockClear()
})
afterEach(() => cleanup())

function seed(opts: { refused?: string[]; msStatus?: string } = {}): void {
  api.fetchInboxes.mockResolvedValue(
    inboxes([
      row(GMAIL, 'google', 'mail,calendar-read'),
      row(MS, 'microsoft', 'mail', opts.msStatus ?? 'connected'),
      row(HEALTHY, 'microsoft', 'mail'),
      row(IMAP, 'imap', null, 'error'),
    ]),
  )
  watch.fetchMailWatch.mockResolvedValue(watchStatus(opts.refused ?? [GMAIL]))
}

async function openAccount(address: string): Promise<HTMLElement> {
  fireEvent.click(await screen.findByTestId(`account-row-${address}`))
  return screen.findByTestId('email-account')
}

describe('reconnectFeatures', () => {
  it('asks for exactly what the link has now', () => {
    const [g, m, , i] = inboxes([
      row(GMAIL, 'google', 'mail,calendar-read'),
      row(MS, 'microsoft', 'mail'),
      row(HEALTHY, 'microsoft', 'mail'),
      row(IMAP, 'imap', null),
    ])
    expect(reconnectFeatures(g)).toBe('mail,calendar-read')
    expect(reconnectFeatures(m)).toBe('mail')
    // An app password isn't a sign-in the provider can redo.
    expect(reconnectFeatures(i)).toBeNull()
  })
})

describe('the accounts list', () => {
  it('shows Can’t connect and Sign in again on a refused sign-in, and not on a healthy one', async () => {
    seed({ refused: [GMAIL], msStatus: 'error' })
    render(<EmailLinkSection />)
    const refused = await screen.findByTestId(`account-item-${GMAIL}`)
    await waitFor(() => expect(within(refused).getByText('Can’t connect')).toBeTruthy())
    expect(within(refused).getByRole('button', { name: `Sign in again to ${GMAIL}` })).toBeTruthy()
    // A row the daemon marks errored is offered the same way.
    const errored = screen.getByTestId(`account-item-${MS}`)
    expect(within(errored).getByText('Can’t connect')).toBeTruthy()
    expect(within(errored).getByRole('button', { name: `Sign in again to ${MS}` })).toBeTruthy()

    const healthy = screen.getByTestId(`account-item-${HEALTHY}`)
    expect(within(healthy).getByText('Connected')).toBeTruthy()
    expect(within(healthy).queryByRole('button', { name: /Sign in again/ })).toBeNull()
    // An app-password account can't sign in again here.
    const imap = screen.getByTestId(`account-item-${IMAP}`)
    expect(within(imap).getByText('Can’t connect')).toBeTruthy()
    expect(within(imap).queryByRole('button', { name: /Sign in again/ })).toBeNull()
    // The watcher is asked once, for the workspace that manages them.
    expect(watch.fetchMailWatch).toHaveBeenCalledWith('p1')
  })

  it('Sign in again opens the account and starts the same sign-in', async () => {
    seed({ refused: [MS] })
    api.linkOauthUpgrade.mockResolvedValue({ flow: 'device', linkId: 'l1', userCode: 'FAKE-0101', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    api.linkOauthStatus.mockResolvedValue({ state: 'pending' })
    render(<EmailLinkSection />)
    const item = await screen.findByTestId(`account-item-${MS}`)
    fireEvent.click(await within(item).findByRole('button', { name: `Sign in again to ${MS}` }))
    await screen.findByTestId('email-account')
    await screen.findByText('FAKE-0101')
    expect(api.linkOauthUpgrade).toHaveBeenCalledWith({ inbox: MS, features: 'mail' })
  })
})

describe('the account page', () => {
  it('says the provider refused the sign-in and offers Reconnect, with Disconnect still there', async () => {
    seed({ refused: [GMAIL] })
    render(<EmailLinkSection />)
    const page = await openAccount(GMAIL)
    const notice = await within(page).findByTestId('reconnect-notice')
    expect(notice.textContent).toContain(`Can’t connect to ${GMAIL}`)
    expect(notice.textContent).toContain(
      'Google refused the saved sign-in. Sign in again to reconnect. Workspace access and the calendar setting stay as they are.',
    )
    const btn = within(notice).getByRole('button', { name: 'Reconnect' })
    expect(btn.getAttribute('title')).toBe('Sign in to Google again')
    expect(within(page).getByRole('button', { name: 'Disconnect…' })).toBeTruthy()
  })

  it('a healthy account has no notice', async () => {
    seed({ refused: [] })
    render(<EmailLinkSection />)
    const page = await openAccount(HEALTHY)
    expect(within(page).queryByTestId('reconnect-notice')).toBeNull()
  })

  it('Google: signs in again for mail and the read-only calendar, then says what happens next', async () => {
    seed({ refused: [GMAIL] })
    let release: (v: unknown) => void = () => undefined
    const redirect = new Promise((r) => {
      release = r
    })
    tauri.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'oauth_loopback_bind') return { captureId: 'cap-1', redirectUri: 'http://127.0.0.1:5555/cb', port: 5555 }
      if (cmd === 'oauth_loopback_wait') return redirect
      return null
    })
    api.linkOauthUpgrade.mockResolvedValue({
      flow: 'loopback',
      linkId: 'lnk-g',
      clientCapture: true,
      authorizationUrl: 'https://accounts.google.example/o/oauth2/v2/auth',
      state: 'st-1',
    })
    api.linkOauthComplete.mockResolvedValue({ ok: true, state: 'connected', linkId: 'lnk-g' })
    let connected = false
    api.linkOauthStatus.mockImplementation(async () => (connected ? { state: 'connected', address: GMAIL } : { state: 'pending' }))
    render(<EmailLinkSection pollMs={10} />)
    const page = await openAccount(GMAIL)
    fireEvent.click(within(await within(page).findByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' }))

    const waiting = await screen.findByTestId('reconnect-browser')
    expect(waiting.textContent).toContain('Waiting for you to sign in…')
    expect(api.linkOauthUpgrade).toHaveBeenCalledWith({
      inbox: GMAIL,
      features: 'mail,calendar-read',
      clientCapture: true,
      redirectUri: 'http://127.0.0.1:5555/cb',
    })
    expect(opener.openUrl).toHaveBeenCalledWith('https://accounts.google.example/o/oauth2/v2/auth')

    // After the sign-in the row reads connected, but the watcher hasn't
    // looked again yet: say so rather than claim mail is being checked.
    api.fetchInboxes.mockResolvedValue(
      inboxes([row(GMAIL, 'google', 'mail,calendar-read'), row(MS, 'microsoft', 'mail'), row(HEALTHY, 'microsoft', 'mail'), row(IMAP, 'imap', null, 'error')]),
    )
    release({ code: 'g-code', state: 'st-1', error: null })
    connected = true
    const done = await screen.findByTestId('reconnect-done')
    await waitFor(() => expect(done.textContent).toContain('Signed in. Mail checks resume on the next check.'))
    expect(api.linkOauthComplete).toHaveBeenCalledWith({ linkId: 'lnk-g', code: 'g-code', state: 'st-1', error: undefined })
    expect(screen.queryByTestId('reconnect-notice')).toBeNull()
    expect(api.removeLinkedInbox).not.toHaveBeenCalled()
  })

  it('Microsoft: a device code, then Reconnected once the watcher is clear', async () => {
    seed({ refused: [MS] })
    api.linkOauthUpgrade.mockResolvedValue({ flow: 'device', linkId: 'lnk-m', userCode: 'FAKE-0202', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    let connected = false
    api.linkOauthStatus.mockImplementation(async () => (connected ? { state: 'connected', address: MS } : { state: 'pending' }))
    render(<EmailLinkSection pollMs={10} />)
    const page = await openAccount(MS)
    expect((await within(page).findByTestId('reconnect-notice')).textContent).toContain('Microsoft refused the saved sign-in.')
    fireEvent.click(within(page).getByRole('button', { name: 'Reconnect' }))
    const device = await screen.findByTestId('reconnect-device')
    expect(device.textContent).toContain('FAKE-0202')
    expect(device.textContent).toContain('Waiting for you to sign in…')
    expect(api.linkOauthUpgrade).toHaveBeenCalledWith({ inbox: MS, features: 'mail' })

    watch.fetchMailWatch.mockResolvedValue(watchStatus([], '2026-11-20T09:05:00+00:00'))
    connected = true
    const done = await screen.findByTestId('reconnect-done')
    await waitFor(() => expect(done.textContent).toContain('Reconnected. Agents can use this account again.'))
    expect(api.removeLinkedInbox).not.toHaveBeenCalled()
  })

  it('a different account: says which one to sign in as, with Try again', async () => {
    seed({ refused: [MS] })
    api.linkOauthUpgrade.mockResolvedValue({ flow: 'device', linkId: 'lnk-x', userCode: 'FAKE-0303', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    api.linkOauthStatus.mockResolvedValue({ state: 'error', code: 'account_mismatch', hint: 'account mismatch: nothing was stored' })
    render(<EmailLinkSection pollMs={10} />)
    const page = await openAccount(MS)
    fireEvent.click(within(await within(page).findByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' }))
    const err = await screen.findByTestId('reconnect-error')
    expect(err.textContent).toContain(`That’s a different account. Sign in as ${MS}.`)
    const retry = within(screen.getByTestId('reconnect-notice')).getByRole('button', { name: 'Try again' })
    api.linkOauthUpgrade.mockClear()
    fireEvent.click(retry)
    await waitFor(() => expect(api.linkOauthUpgrade).toHaveBeenCalledWith({ inbox: MS, features: 'mail' }))
    expect(api.removeLinkedInbox).not.toHaveBeenCalled()
  })

  it('any other failure: the reason, plus Try again', async () => {
    seed({ refused: [MS] })
    api.linkOauthUpgrade.mockRejectedValue(new Error('owner only'))
    render(<EmailLinkSection pollMs={10} />)
    const page = await openAccount(MS)
    fireEvent.click(within(await within(page).findByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' }))
    const err = await screen.findByTestId('reconnect-error')
    expect(err.textContent).toContain('owner only')
    expect(within(screen.getByTestId('reconnect-notice')).getByRole('button', { name: 'Try again' })).toBeTruthy()
  })

  it('Cancel, or closing the sign-in, goes back to the notice', async () => {
    seed({ refused: [MS] })
    api.linkOauthUpgrade.mockResolvedValue({ flow: 'device', linkId: 'lnk-c', userCode: 'FAKE-0404', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    api.linkOauthStatus.mockResolvedValue({ state: 'pending' })
    render(<EmailLinkSection pollMs={10} />)
    const page = await openAccount(MS)
    fireEvent.click(within(await within(page).findByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' }))
    await screen.findByTestId('reconnect-device')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByTestId('reconnect-device')).toBeNull()
    expect(within(screen.getByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' })).toBeTruthy()
    expect(screen.queryByTestId('reconnect-error')).toBeNull()

    // Declining on the provider's page ends the same way.
    api.linkOauthStatus.mockResolvedValue({ state: 'denied' })
    fireEvent.click(within(screen.getByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' }))
    await waitFor(() => expect(within(screen.getByTestId('reconnect-notice')).getByRole('button', { name: 'Reconnect' })).toBeTruthy())
    await waitFor(() => expect(screen.queryByTestId('reconnect-device')).toBeNull())
    expect(screen.queryByTestId('reconnect-error')).toBeNull()
    expect(api.removeLinkedInbox).not.toHaveBeenCalled()
  })
})
