// @vitest-environment jsdom
// MCS2 (prd-app-mail-calendar-surfaces-v1): "Connect Microsoft" is enabled
// only when the daemon reports a real Microsoft OAuth app (baked into the
// build or the owner's own); otherwise it says why. Older daemons (no
// `configured` field) say the server needs an update.
import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined) }))
vi.mock('@/components/BrowserPane/BrowserPane', () => ({ BrowserPane: () => null }))
vi.mock('@/components/Room/PrimaryRoom', () => ({ PrimaryRoom: () => null }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: unknown[] }) => unknown) => sel({ projects: [] }),
}))

import { MicrosoftConnectButton, microsoftLinkState } from './EmailLinkSection'
import type { OauthConfig, OauthProviderConfig } from './email-api'

function cfg(microsoft: Partial<OauthProviderConfig>): OauthConfig {
  return {
    providers: {
      gmail: { source: 'default', secretSet: false, configured: true },
      microsoft: { source: 'default', secretSet: false, ...microsoft },
    },
  }
}

afterEach(() => cleanup())

describe('microsoftLinkState', () => {
  it('is enabled only for a configured Microsoft app', () => {
    expect(microsoftLinkState(cfg({ configured: true, clientId: 'abc' }))).toEqual({
      enabled: true,
      badge: null,
      title: expect.stringContaining('short code'),
    })
  })

  it('names the fix when the build has no Microsoft app', () => {
    const st = microsoftLinkState(cfg({ configured: false, clientId: 'REPLACE_ME-microsoft-client-id' }))
    expect(st.enabled).toBe(false)
    expect(st.badge).toBe('Needs an app')
    expect(st.title).toContain('Sign-in apps')
  })

  it('says to update a daemon from before Graph send', () => {
    const st = microsoftLinkState(cfg({}))
    expect(st).toEqual({ enabled: false, badge: 'Update needed', title: expect.stringContaining('Update it') })
  })

  it('is disabled while loading and when the read failed', () => {
    expect(microsoftLinkState(null).enabled).toBe(false)
    expect(microsoftLinkState('error')).toEqual({
      enabled: false,
      badge: 'Unavailable',
      title: expect.stringContaining("Couldn't check"),
    })
  })
})

describe('MicrosoftConnectButton', () => {
  it('starts the device flow when configured and the form is filled', () => {
    const onStart = vi.fn()
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart busy={false} onStart={onStart} />)
    const btn = screen.getByTestId('connect-microsoft') as HTMLButtonElement
    expect(btn.disabled).toBe(false)
    expect(screen.queryByText('Update needed')).toBeNull()
    fireEvent.click(btn)
    expect(onStart).toHaveBeenCalledTimes(1)
  })

  it('stays disabled until the workspace + address are filled', () => {
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart={false} busy={false} onStart={vi.fn()} />)
    expect((screen.getByTestId('connect-microsoft') as HTMLButtonElement).disabled).toBe(true)
  })

  it('is disabled with a "Needs an app" tag without a Microsoft app', () => {
    const onStart = vi.fn()
    render(<MicrosoftConnectButton cfg={cfg({ configured: false })} canStart busy={false} onStart={onStart} />)
    const btn = screen.getByTestId('connect-microsoft') as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    expect(screen.getByText('Needs an app')).toBeTruthy()
    fireEvent.click(btn)
    expect(onStart).not.toHaveBeenCalled()
  })

  it('shows Starting… while busy', () => {
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart busy onStart={vi.fn()} />)
    expect(screen.getByTestId('connect-microsoft').textContent).toContain('Starting…')
  })
})
