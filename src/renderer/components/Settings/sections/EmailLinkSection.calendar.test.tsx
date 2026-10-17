// @vitest-environment jsdom
// MCS3/MCS4 (prd-app-mail-calendar-surfaces-v1 §3.2): a Microsoft or Google
// link consented for mail only shows "Add calendar", which re-consents the
// SAME link for calendars: Microsoft by device code, Google through its
// consent page (client-capture loopback, the code relayed to the daemon).
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const tauri = vi.hoisted(() => ({ invoke: vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null) }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: tauri.invoke }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const opener = vi.hoisted(() => ({ openUrl: vi.fn(async (_url: string) => undefined) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: opener.openUrl }))
vi.mock('@/components/BrowserPane/BrowserPane', () => ({ BrowserPane: () => null }))
vi.mock('@/components/Room/PrimaryRoom', () => ({ PrimaryRoom: () => null }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: unknown[] }) => unknown) => sel({ projects: [] }),
}))

const api = vi.hoisted(() => ({
  linkOauthUpgrade: vi.fn(),
  linkOauthStatus: vi.fn(),
  linkOauthComplete: vi.fn(),
}))
vi.mock('./email-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./email-api')>()
  return {
    ...real,
    linkOauthUpgrade: api.linkOauthUpgrade,
    linkOauthStatus: api.linkOauthStatus,
    linkOauthComplete: api.linkOauthComplete,
  }
})

import { AddCalendarAccess, needsCalendarConsent } from './EmailLinkSection'
import type { Inbox, InboxCapabilities } from './email-api'
import { useToastStore } from '@/stores/toast'

function inbox(capabilities?: InboxCapabilities): Inbox {
  return {
    address: 'rosson@contoso.example',
    source: 'linked',
    displayName: null,
    status: 'connected',
    primary: { projectId: 'p1', workspace: 'repo', level: 'draft', canManage: false, canDelete: false },
    grants: [],
    yourLevel: null,
    maxLevel: 'send',
    capabilities,
  }
}

const UPGRADE_FIX = 'k2 mail link upgrade rosson@contoso.example --features calendar (owner)'

const msMailOnly: InboxCapabilities = {
  source: 'microsoft',
  missing: [
    { cap: 'calendar.read', reason: 'scope_not_granted', fix: UPGRADE_FIX },
    { cap: 'calendar.write', reason: 'scope_not_granted', fix: UPGRADE_FIX },
  ],
}

const GOOGLE_FIX = 'k2 mail link upgrade rosson@gmail.example --features calendar (owner)'
const googleMailOnly: InboxCapabilities = {
  source: 'google',
  missing: [
    { cap: 'calendar.read', reason: 'scope_not_granted', fix: GOOGLE_FIX },
    { cap: 'calendar.write', reason: 'scope_not_granted', fix: GOOGLE_FIX },
  ],
}

beforeEach(() => {
  api.linkOauthUpgrade.mockReset()
  api.linkOauthStatus.mockReset()
  api.linkOauthComplete.mockReset()
  tauri.invoke.mockReset()
  tauri.invoke.mockImplementation(async () => null)
  opener.openUrl.mockClear()
})
afterEach(() => cleanup())

describe('needsCalendarConsent', () => {
  it('is true for a Microsoft or Google link missing the calendar consent', () => {
    expect(needsCalendarConsent(inbox(msMailOnly))).toBe(true)
    expect(needsCalendarConsent(inbox(googleMailOnly))).toBe(true)
    // An app-password IMAP inbox has no calendar API — nothing to consent.
    expect(
      needsCalendarConsent(
        inbox({ source: 'imap', missing: [{ cap: 'calendar.read', reason: 'backend_unsupported', fix: 'oauth' }] }),
      ),
    ).toBe(false)
    expect(
      needsCalendarConsent(
        inbox({ source: 'google', missing: [{ cap: 'calendar.invite', reason: 'not_built', fix: 'later' }] }),
      ),
    ).toBe(false)
    // Calendar consented: only invites are off (not built) — nothing to add.
    expect(
      needsCalendarConsent(
        inbox({ source: 'microsoft', missing: [{ cap: 'calendar.invite', reason: 'not_built', fix: 'later' }] }),
      ),
    ).toBe(false)
    expect(
      needsCalendarConsent(
        inbox({ source: 'microsoft', missing: [{ cap: 'calendar.read', reason: 'owner_disabled', fix: 'k2 mail link calendars on' }] }),
      ),
    ).toBe(false)
    expect(needsCalendarConsent(inbox(undefined))).toBe(false)
  })
})

describe('AddCalendarAccess', () => {
  it('upgrades the same link for calendars by device code, then refreshes', async () => {
    api.linkOauthUpgrade.mockResolvedValue({
      flow: 'device',
      linkId: 'lnk-1',
      userCode: 'FAKE-0042',
      verificationUrl: 'https://microsoft.example/devicelogin',
      expiresIn: 900,
    })
    api.linkOauthStatus.mockResolvedValueOnce({ state: 'pending' }).mockResolvedValue({ state: 'connected', address: 'rosson@contoso.example' })
    const addToast = vi.spyOn(useToastStore.getState(), 'addToast')
    const onUpgraded = vi.fn()
    render(<AddCalendarAccess inbox={inbox(msMailOnly)} canMutate onUpgraded={onUpgraded} pollMs={10} />)

    fireEvent.click(screen.getByTestId('add-calendar-button'))
    expect(api.linkOauthUpgrade).toHaveBeenCalledWith({ inbox: 'rosson@contoso.example', features: 'calendar' })
    await screen.findByText('FAKE-0042')
    expect(screen.getByTestId('add-calendar-device').textContent).toContain('https://microsoft.example/devicelogin')
    await waitFor(() => expect(onUpgraded).toHaveBeenCalledTimes(1))
    expect(api.linkOauthStatus).toHaveBeenCalledWith('lnk-1')
    expect(addToast).toHaveBeenCalledWith('Calendar access added for rosson@contoso.example', 'success')
    expect(screen.queryByTestId('add-calendar-device')).toBeNull()
  })

  it('shows why an upgrade failed, with the admin consent page', async () => {
    api.linkOauthUpgrade.mockResolvedValue({
      flow: 'device',
      linkId: 'lnk-2',
      userCode: 'FAKE-0043',
      verificationUrl: 'https://microsoft.example/devicelogin',
      expiresIn: 900,
    })
    api.linkOauthStatus.mockResolvedValue({
      state: 'error',
      code: 'admin_consent_required',
      hint: 'ask your admin',
      adminConsentUrl: 'https://login.microsoftonline.com/organizations/adminconsent?client_id=x',
    })
    const onUpgraded = vi.fn()
    render(<AddCalendarAccess inbox={inbox(msMailOnly)} canMutate onUpgraded={onUpgraded} pollMs={10} />)
    fireEvent.click(screen.getByTestId('add-calendar-button'))
    const err = await screen.findByTestId('add-calendar-error')
    expect(err.textContent).toContain('admin needs to approve calendar access')
    expect(err.textContent).toContain('ask your admin')
    expect(screen.getByText('Open the admin consent page')).toBeTruthy()
    expect(onUpgraded).not.toHaveBeenCalled()
  })

  it('shows a start error from the daemon', async () => {
    api.linkOauthUpgrade.mockRejectedValue(new Error('owner only'))
    render(<AddCalendarAccess inbox={inbox(msMailOnly)} canMutate onUpgraded={vi.fn()} />)
    fireEvent.click(screen.getByTestId('add-calendar-button'))
    expect((await screen.findByTestId('add-calendar-error')).textContent).toContain('owner only')
  })

  it('upgrades a Google link through its consent page, relaying the code, then refreshes', async () => {
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
      authorizationUrl: 'https://accounts.google.example/o/oauth2/v2/auth?include_granted_scopes=true',
      state: 'st-1',
    })
    api.linkOauthComplete.mockResolvedValue({ ok: true })
    let connected = false
    api.linkOauthStatus.mockImplementation(async () => (connected ? { state: 'connected', address: 'rosson@gmail.example' } : { state: 'pending' }))
    const onUpgraded = vi.fn()
    const g = { ...inbox(googleMailOnly), address: 'rosson@gmail.example' }
    render(<AddCalendarAccess inbox={g} canMutate onUpgraded={onUpgraded} pollMs={10} />)

    fireEvent.click(screen.getByTestId('add-calendar-button'))
    await screen.findByTestId('add-calendar-browser')
    expect(api.linkOauthUpgrade).toHaveBeenCalledWith({
      inbox: 'rosson@gmail.example',
      features: 'calendar',
      clientCapture: true,
      redirectUri: 'http://127.0.0.1:5555/cb',
    })
    expect(opener.openUrl).toHaveBeenCalledWith('https://accounts.google.example/o/oauth2/v2/auth?include_granted_scopes=true')
    expect(screen.queryByTestId('add-calendar-device')).toBeNull()
    expect(onUpgraded).not.toHaveBeenCalled()

    release({ code: 'g-code', state: 'st-1', error: null })
    await waitFor(() =>
      expect(api.linkOauthComplete).toHaveBeenCalledWith({ linkId: 'lnk-g', code: 'g-code', state: 'st-1', error: undefined }),
    )
    connected = true
    await waitFor(() => expect(onUpgraded).toHaveBeenCalledTimes(1))
    expect(api.linkOauthStatus).toHaveBeenCalledWith('lnk-g')
    expect(screen.queryByTestId('add-calendar-browser')).toBeNull()
  })

  it('says when Google calendar access was not allowed on the consent page', async () => {
    tauri.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'oauth_loopback_bind') return { captureId: 'cap-2', redirectUri: 'http://127.0.0.1:5556/cb', port: 5556 }
      if (cmd === 'oauth_loopback_wait') return new Promise(() => undefined)
      return null
    })
    api.linkOauthUpgrade.mockResolvedValue({
      flow: 'loopback',
      linkId: 'lnk-g2',
      clientCapture: true,
      authorizationUrl: 'https://accounts.google.example/consent',
      state: 'st-2',
    })
    api.linkOauthStatus.mockResolvedValue({ state: 'error', code: 'scope_not_granted', hint: 'Nothing was stored' })
    const onUpgraded = vi.fn()
    render(<AddCalendarAccess inbox={inbox(googleMailOnly)} canMutate onUpgraded={onUpgraded} pollMs={10} />)
    fireEvent.click(screen.getByTestId('add-calendar-button'))
    const err = await screen.findByTestId('add-calendar-error')
    expect(err.textContent).toContain('Calendar access was not allowed')
    expect(err.textContent).toContain('Nothing was stored')
    expect(onUpgraded).not.toHaveBeenCalled()
    await waitFor(() => expect(tauri.invoke).toHaveBeenCalledWith('oauth_loopback_cancel', { captureId: 'cap-2' }))
  })

  it('renders nothing for an IMAP row or a row that already has calendars', () => {
    const { container, rerender } = render(
      <AddCalendarAccess
        inbox={inbox({ source: 'imap', missing: [{ cap: 'calendar.read', reason: 'backend_unsupported', fix: 'oauth' }] })}
        canMutate
        onUpgraded={vi.fn()}
      />,
    )
    expect(screen.queryByTestId('add-calendar-button')).toBeNull()
    rerender(<AddCalendarAccess inbox={inbox({ source: 'google', missing: [] })} canMutate onUpgraded={vi.fn()} />)
    expect(screen.queryByTestId('add-calendar-button')).toBeNull()
    rerender(<AddCalendarAccess inbox={inbox({ source: 'microsoft', missing: [] })} canMutate onUpgraded={vi.fn()} />)
    expect(screen.queryByTestId('add-calendar-button')).toBeNull()
    expect(container.innerHTML).toBe('')
  })

  it('is disabled in viewer mode', () => {
    render(<AddCalendarAccess inbox={inbox(msMailOnly)} canMutate={false} onUpgraded={vi.fn()} />)
    const btn = screen.getByTestId('add-calendar-button') as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    fireEvent.click(btn)
    expect(api.linkOauthUpgrade).not.toHaveBeenCalled()
  })
})
