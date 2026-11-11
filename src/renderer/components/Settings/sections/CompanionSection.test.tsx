// @vitest-environment jsdom
// Settings → K2 Companion: can a phone reach this server (the tunnel's
// status, with a way to Tunnel when it can't), the store codes, then the
// shared pairing cards.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1234, token: 'sample-token' })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: () => 'http://127.0.0.1:1234',
}))
vi.mock('@/lib/server-capabilities', () => ({ serverSupports: () => false }))
vi.mock('@/stores/session-events', () => ({
  onTunnelStatusChanged: () => () => {},
  onAppHello: () => () => {},
  onPairCodeChanged: () => () => {},
}))
const setSection = vi.hoisted(() => vi.fn())
vi.mock('@/stores/settings', () => ({ useSettingsStore: { getState: () => ({ setSection }) } }))

import { CompanionSection, COMPANION_MANIFEST } from './CompanionSection'

const RUNNING = { running: true, public_url: 'https://alex.k2.dev', frpc_installed: true }
const STOPPED = { running: false, public_url: null, frpc_installed: true }
const DEVICES = { ok: true, devices: [], pairAs: { user: 'alex', source: 'owner-linked', needsPassword: false, choices: null } }

let tunnel: { status: number; body: unknown }
beforeEach(() => {
  setSection.mockClear()
  tunnel = { status: 200, body: RUNNING }
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      if (url.includes('/cli/tunnel/status')) return new Response(JSON.stringify(tunnel.body), { status: tunnel.status })
      if (url.includes('/cli/auth/devices')) return new Response(JSON.stringify(DEVICES), { status: 200 })
      throw new Error(`unexpected ${url}`)
    }),
  )
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const statusCard = (): HTMLElement => screen.getByTestId('companion-status')

describe('Settings → K2 Companion', () => {
  it('says the server is reachable at its address, with Copy address', async () => {
    render(<CompanionSection />)
    await waitFor(() => expect(within(statusCard()).getByText('Reachable')).toBeTruthy())
    expect(within(statusCard()).getByRole('link', { name: 'https://alex.k2.dev' })).toBeTruthy()
    expect(within(statusCard()).getByRole('button', { name: 'Copy address' })).toBeTruthy()
    // The plain header line, no internal words.
    expect(document.body.textContent).toContain('Use K2 from your phone.')
    expect(document.body.textContent).not.toMatch(/daemon|one tunnel, both clients/)
  })

  it('with the tunnel off says a phone can’t reach it and opens Tunnel', async () => {
    tunnel = { status: 200, body: STOPPED }
    render(<CompanionSection />)
    await waitFor(() => expect(within(statusCard()).getByText('Not reachable from a phone')).toBeTruthy())
    expect(statusCard().textContent).toContain('which is off')
    fireEvent.click(within(statusCard()).getByRole('button', { name: 'Open Tunnel' }))
    expect(setSection).toHaveBeenCalledWith('k2-connect')
  })

  it('a failed status read says what failed, the server’s reason, and Try again', async () => {
    tunnel = { status: 500, body: { error: 'tunnel state file is unreadable' } }
    render(<CompanionSection />)
    const card = await screen.findByTestId('companion-status-error')
    expect(card.textContent).toContain("Couldn't check whether your phone can reach this server")
    expect(card.textContent).toContain('The server said: tunnel state file is unreadable.')
    tunnel = { status: 200, body: RUNNING }
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(within(statusCard()).getByText('Reachable')).toBeTruthy())
  })

  it('shows both store codes as links and the pairing cards in order', async () => {
    render(<CompanionSection />)
    const app = screen.getByRole('link', { name: 'Get K2 for iPhone on App Store' })
    expect(app.getAttribute('href')).toContain('apps.apple.com')
    expect(app.querySelector('[role="img"] svg')).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Get K2 for Android on Google Play' }).getAttribute('href')).toContain('play.google.com')
    await screen.findByText('No phones paired yet')
    const order = Array.from(document.querySelectorAll('[data-settings-id^="companion."]')).map((e) => e.getAttribute('data-settings-id'))
    expect(order).toEqual(['companion.status', 'companion.get-app', 'companion.pair', 'companion.phones'])
    expect(COMPANION_MANIFEST.map((e) => e.id).sort()).toEqual([...order].sort())
  })
})
