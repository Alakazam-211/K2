// @vitest-environment jsdom
// MCS3 (prd-app-mail-calendar-surfaces-v1 §3.2): a Microsoft link consented
// for mail only shows "Add calendar", which re-consents the SAME link for
// calendars through the device-code upgrade path. Google rows get no button
// (Google calendars aren't built yet).
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined) }))
vi.mock('@/components/BrowserPane/BrowserPane', () => ({ BrowserPane: () => null }))
vi.mock('@/components/Room/PrimaryRoom', () => ({ PrimaryRoom: () => null }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: unknown[] }) => unknown) => sel({ projects: [] }),
}))

const api = vi.hoisted(() => ({
  linkOauthUpgrade: vi.fn(),
  linkOauthStatus: vi.fn(),
}))
vi.mock('./email-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./email-api')>()
  return { ...real, linkOauthUpgrade: api.linkOauthUpgrade, linkOauthStatus: api.linkOauthStatus }
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

beforeEach(() => {
  api.linkOauthUpgrade.mockReset()
  api.linkOauthStatus.mockReset()
})
afterEach(() => cleanup())

describe('needsCalendarConsent', () => {
  it('is true only for a Microsoft link missing the calendar consent', () => {
    expect(needsCalendarConsent(inbox(msMailOnly))).toBe(true)
    expect(
      needsCalendarConsent(
        inbox({ source: 'google', missing: [{ cap: 'calendar.read', reason: 'scope_not_granted', fix: UPGRADE_FIX }] }),
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

  it('renders nothing for a Google row or a Microsoft row that already has calendars', () => {
    const { container, rerender } = render(
      <AddCalendarAccess
        inbox={inbox({ source: 'google', missing: [{ cap: 'calendar.read', reason: 'scope_not_granted', fix: UPGRADE_FIX }] })}
        canMutate
        onUpgraded={vi.fn()}
      />,
    )
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
