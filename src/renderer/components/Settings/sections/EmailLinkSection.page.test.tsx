// @vitest-environment jsdom
// Settings → Email Link, the whole page: a reply it can't read is a failed
// read with Try again (it used to crash the page), an empty list says so,
// and the main task (connect, see it listed, disconnect after a confirm)
// says what happened at each step.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined) }))
vi.mock('@/components/BrowserPane/BrowserPane', () => ({ BrowserPane: () => null }))
vi.mock('@/components/Room/PrimaryRoom', () => ({ PrimaryRoom: () => null }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: Array<{ id: string; name: string }> }) => unknown) =>
    sel({ projects: [{ id: 'p1', name: 'atlas' }, { id: 'p2', name: 'release-bot' }] }),
}))

const api = vi.hoisted(() => ({
  fetchInboxes: vi.fn(),
  fetchOauthConfig: vi.fn(),
  linkOauthStart: vi.fn(),
  linkOauthStatus: vi.fn(),
  removeLinkedInbox: vi.fn(),
}))
vi.mock('./email-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./email-api')>()
  return { ...real, ...api }
})

const confirm = vi.hoisted(() => ({ answer: true, calls: [] as Array<{ title: string }> }))
vi.mock('@/stores/confirm-dialog', () => ({
  useConfirmDialogStore: {
    getState: () => ({
      confirm: async (o: { title: string }) => {
        confirm.calls.push(o)
        return confirm.answer
      },
    }),
  },
}))

import { EmailLinkSection } from './EmailLinkSection'
import { MailReplyError, parseInboxes, parseOauthConfig, type Inbox } from './email-api'

const ROW: Inbox = {
  address: 'alex@example.com',
  source: 'linked',
  displayName: null,
  status: 'connected',
  primary: { projectId: 'p1', workspace: 'atlas', level: 'draft', canManage: false, canDelete: false },
  grants: [],
  yourLevel: null,
  maxLevel: 'send',
  host: 'imap.example.com',
  tls: 'implicit-tls',
  capabilities: { source: 'microsoft', missing: [] },
}
const CFG = {
  providers: {
    gmail: { source: 'default' as const, secretSet: false, configured: true },
    microsoft: { source: 'default' as const, secretSet: false, configured: true },
  },
}

beforeEach(() => {
  for (const f of Object.values(api)) f.mockReset()
  api.fetchOauthConfig.mockResolvedValue(CFG)
  confirm.answer = true
  confirm.calls = []
})
afterEach(() => cleanup())

describe('EmailLinkSection', () => {
  it('shows a failed read with Try again for replies it cannot read, instead of crashing', async () => {
    // The mock daemon answers `{}`: both guards refuse it.
    api.fetchInboxes.mockImplementation(async () => parseInboxes({}))
    api.fetchOauthConfig.mockImplementation(async () => parseOauthConfig({}))
    render(<EmailLinkSection />)
    const card = await screen.findByTestId('accounts-error')
    expect(card.textContent).toContain('Couldn’t load your email accounts')
    expect(card.textContent).toContain('sent an answer K2 couldn’t read')
    // The raw parser text never shows.
    expect(card.textContent).not.toContain('object without')
    expect(await screen.findByText('Couldn’t load the sign-in apps')).toBeTruthy()
    // The Microsoft button is off and says why.
    expect((screen.getByTestId('connect-microsoft') as HTMLButtonElement).disabled).toBe(true)

    api.fetchInboxes.mockResolvedValue([ROW])
    fireEvent.click(card.querySelector('button') as HTMLButtonElement)
    expect(await screen.findByTestId('account-row-alex@example.com')).toBeTruthy()
  })

  it("shows the server's reason quietly when the read fails", async () => {
    api.fetchInboxes.mockRejectedValue(new Error('database is locked'))
    render(<EmailLinkSection />)
    const card = await screen.findByTestId('accounts-error')
    expect(card.textContent).toContain('The server said: database is locked.')
  })

  it('says when nothing is connected yet', async () => {
    api.fetchInboxes.mockResolvedValue([])
    render(<EmailLinkSection />)
    expect((await screen.findByTestId('accounts-empty')).textContent).toContain('No email accounts yet')
  })

  it('lists only linked accounts', async () => {
    api.fetchInboxes.mockResolvedValue([ROW, { ...ROW, address: 'bot@mail.example.net', source: 'hosted' }])
    render(<EmailLinkSection />)
    await screen.findByTestId('account-row-alex@example.com')
    expect(screen.queryByTestId('account-row-bot@mail.example.net')).toBeNull()
  })

  it('connects a Microsoft account, lists it and says so', async () => {
    let connected = false
    api.fetchInboxes.mockImplementation(async () => (connected ? [ROW] : []))
    api.linkOauthStart.mockResolvedValue({ flow: 'device', linkId: 'l1', userCode: 'FAKE-0001', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    api.linkOauthStatus.mockImplementation(async () => {
      connected = true
      return { state: 'connected', address: 'alex@example.com' }
    })
    render(<EmailLinkSection />)
    await screen.findByTestId('accounts-empty')

    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    fireEvent.click(document.querySelector('[data-setting-dropdown-menu] [data-value="p1"]') as HTMLElement)
    fireEvent.change(screen.getByLabelText('Email address'), { target: { value: 'alex@example.com' } })
    fireEvent.click(screen.getByTestId('connect-microsoft'))

    const notice = await screen.findByTestId('email-notice')
    expect(notice.textContent).toContain('alex@example.com is connected. Agents in atlas can use it now.')
    expect(api.linkOauthStart).toHaveBeenCalledWith({ address: 'alex@example.com', provider: 'microsoft', workspace: 'p1' })
    expect(await screen.findByTestId('account-row-alex@example.com')).toBeTruthy()
  })

  it('says why a sign-in failed and offers Try again', async () => {
    api.fetchInboxes.mockResolvedValue([])
    api.linkOauthStart.mockResolvedValue({ flow: 'device', linkId: 'l1', userCode: 'FAKE-0001', verificationUrl: 'https://login.example/device', expiresIn: 900 })
    api.linkOauthStatus.mockResolvedValue({ state: 'denied', hint: 'declined' })
    render(<EmailLinkSection />)
    await screen.findByTestId('accounts-empty')
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    fireEvent.click(document.querySelector('[data-setting-dropdown-menu] [data-value="p1"]') as HTMLElement)
    fireEvent.change(screen.getByLabelText('Email address'), { target: { value: 'alex@example.com' } })
    fireEvent.click(screen.getByTestId('connect-microsoft'))
    const err = await screen.findByTestId('connect-error')
    expect(err.textContent).toContain('Couldn’t connect Microsoft')
    expect(err.textContent).toContain('Access wasn’t allowed')
    expect(err.textContent).toContain('The server said: declined.')
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })

  it('a start reply it cannot read is a failed connect, not a crash', async () => {
    api.fetchInboxes.mockResolvedValue([])
    api.linkOauthStart.mockRejectedValue(new MailReplyError('the sign-in step', 'the server sent nothing'))
    render(<EmailLinkSection />)
    await screen.findByTestId('accounts-empty')
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    fireEvent.click(document.querySelector('[data-setting-dropdown-menu] [data-value="p1"]') as HTMLElement)
    fireEvent.change(screen.getByLabelText('Email address'), { target: { value: 'alex@example.com' } })
    fireEvent.click(screen.getByTestId('connect-microsoft'))
    expect((await screen.findByTestId('connect-error')).textContent).toContain('couldn’t read')
  })

  it('disconnects only after a confirm, then says so on the list', async () => {
    let gone = false
    api.fetchInboxes.mockImplementation(async () => (gone ? [] : [ROW]))
    api.removeLinkedInbox.mockImplementation(async () => {
      gone = true
      return { ok: true, address: ROW.address, removed: true }
    })
    render(<EmailLinkSection />)
    fireEvent.click(await screen.findByTestId('account-row-alex@example.com'))
    await screen.findByTestId('email-account')

    confirm.answer = false
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect…' }))
    await waitFor(() => expect(confirm.calls).toHaveLength(1))
    expect(api.removeLinkedInbox).not.toHaveBeenCalled()
    expect(confirm.calls[0].title).toBe('Disconnect alex@example.com?')

    confirm.answer = true
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect…' }))
    const notice = await screen.findByTestId('email-notice')
    expect(notice.textContent).toContain('alex@example.com is disconnected.')
    expect(api.removeLinkedInbox).toHaveBeenCalledWith('alex@example.com')
    expect(await screen.findByTestId('accounts-empty')).toBeTruthy()
  })

  it('keeps the account and says why when a disconnect fails', async () => {
    api.fetchInboxes.mockResolvedValue([ROW])
    api.removeLinkedInbox.mockRejectedValue(new Error('vault is locked'))
    render(<EmailLinkSection />)
    fireEvent.click(await screen.findByTestId('account-row-alex@example.com'))
    fireEvent.click(await screen.findByRole('button', { name: 'Disconnect…' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t disconnect: vault is locked. The account is still connected; try again.')
    expect(screen.getByTestId('email-account')).toBeTruthy()
  })
})
