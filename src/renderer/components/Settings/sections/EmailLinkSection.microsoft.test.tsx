// @vitest-environment jsdom
// MCS2 (prd-app-mail-calendar-surfaces-v1): "Connect Microsoft" is enabled
// only when the daemon reports a real Microsoft OAuth app (baked into the
// build or the owner's own); otherwise it says why. Older daemons (no
// `configured` field) keep "Coming soon".
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
      title: expect.stringContaining('device code'),
    })
  })

  it('names the fix when the build has no Microsoft app', () => {
    const st = microsoftLinkState(cfg({ configured: false, clientId: 'REPLACE_ME-microsoft-client-id' }))
    expect(st.enabled).toBe(false)
    expect(st.badge).toBe('Needs an app')
    expect(st.title).toContain('OAuth apps')
  })

  it('keeps "Coming soon" for a daemon from before Graph send', () => {
    const st = microsoftLinkState(cfg({}))
    expect(st).toEqual({ enabled: false, badge: 'Coming soon', title: expect.any(String) })
  })

  it('is disabled while loading and when the read failed', () => {
    expect(microsoftLinkState(null).enabled).toBe(false)
    expect(microsoftLinkState('error')).toEqual({
      enabled: false,
      badge: 'Unavailable',
      title: expect.stringContaining("Couldn't read"),
    })
  })
})

describe('MicrosoftConnectButton', () => {
  it('starts the device flow when configured and the form is filled', () => {
    const onStart = vi.fn()
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart busy={false} onStart={onStart} />)
    const btn = screen.getByTestId('connect-microsoft') as HTMLButtonElement
    expect(btn.disabled).toBe(false)
    expect(btn.textContent).not.toContain('Coming soon')
    fireEvent.click(btn)
    expect(onStart).toHaveBeenCalledTimes(1)
  })

  it('stays disabled until the workspace + address are filled', () => {
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart={false} busy={false} onStart={vi.fn()} />)
    expect((screen.getByTestId('connect-microsoft') as HTMLButtonElement).disabled).toBe(true)
  })

  it('is disabled with a "Needs an app" badge without a Microsoft app', () => {
    const onStart = vi.fn()
    render(<MicrosoftConnectButton cfg={cfg({ configured: false })} canStart busy={false} onStart={onStart} />)
    const btn = screen.getByTestId('connect-microsoft') as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    expect(btn.textContent).toContain('Needs an app')
    fireEvent.click(btn)
    expect(onStart).not.toHaveBeenCalled()
  })

  it('shows Starting… while busy', () => {
    render(<MicrosoftConnectButton cfg={cfg({ configured: true })} canStart busy onStart={vi.fn()} />)
    expect(screen.getByTestId('connect-microsoft').textContent).toContain('Starting…')
  })
})
