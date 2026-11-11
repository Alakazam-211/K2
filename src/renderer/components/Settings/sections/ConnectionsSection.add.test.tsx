// @vitest-environment jsdom
// Connected Servers → Add a server: the address first (checked, and asked
// whether it answers), then sign in, then a done step that says it's
// connected. Each failure says what failed in plain words and what to do.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => undefined }))
vi.mock('./FederationOverview', () => ({ FederationOverview: () => null }))
vi.mock('@/lib/host-pool-instance', () => ({ signOutOfHost: vi.fn(async () => undefined) }))
vi.mock('@/lib/host-ops', async (orig) => ({
  ...(await orig<typeof import('@/lib/host-ops')>()),
  hostOpGet: vi.fn(async () => ({ federationEnabled: true })),
  hostBootStatus: vi.fn(async (creds: { base: string }) =>
    creds.base.includes('nowhere') ? null : { phase: 'ready', version: '1.0.0' },
  ),
}))
vi.mock('@/lib/federation', async (orig) => ({
  ...(await orig<typeof import('@/lib/federation')>()),
  isTrustedPeerHost: vi.fn(async () => false),
}))
vi.mock('@/lib/add-server-login', async (orig) => {
  const { useConnectHostStore } = await import('@/stores/connect-host')
  return {
    ...(await orig<typeof import('@/lib/add-server-login')>()),
    verifyHostCredentials: vi.fn(async (host: import('@/stores/connect-host').ConnectHost, password: string) => {
      if (password !== 'sample-password') {
        return { kind: 'error', reason: 'Invalid username or password.', loginKind: 'auth' }
      }
      useConnectHostStore.getState().addHost({ ...host, token: 'sample-token' })
      return { kind: 'signed-in', token: 'sample-token' }
    }),
  }
})

import { ConnectionsSection } from './ConnectionsSection'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

const SAVED: ConnectHost = {
  id: 'h1',
  label: 'Studio Mac',
  hostname: 'studio.example.com',
  port: 443,
  secure: true,
  username: 'alex',
  token: 'sample-token',
  remember: true,
  lastConnectedAt: null,
}

function openAdd(): void {
  useConnectHostStore.setState({ activeHost: 'local', hosts: [SAVED], connectionStatus: 'connected' })
  render(<ConnectionsSection />)
  fireEvent.click(screen.getByRole('button', { name: 'Add a server' }))
}

function type(label: string | RegExp, value: string): void {
  fireEvent.change(screen.getByLabelText(label), { target: { value } })
}

async function submit(): Promise<void> {
  const form = document.querySelector<HTMLFormElement>('[data-testid="connections-server-dialog"] form')!
  await act(async () => {
    fireEvent.submit(form)
  })
}

beforeEach(() => {
  useConnectHostStore.setState({ hosts: [] })
})
afterEach(cleanup)

describe('Add a server', () => {
  it('starts with the address, and says it signs in next', () => {
    openAdd()
    const dialog = screen.getByRole('dialog', { name: 'Add a server' })
    expect(dialog.textContent).toContain('Step 1 of 2')
    expect(screen.getByLabelText('Address')).toBeTruthy()
    expect(screen.getByLabelText('Name (optional)')).toBeTruthy()
    expect(screen.queryByLabelText('Password')).toBeNull()
  })

  it('a malformed address says how to fix it, under the field', async () => {
    openAdd()
    type('Address', 'http://192.168.1.50')
    await submit()
    expect(screen.getByText('Add the server’s port, like http://192.168.1.50:60710.')).toBeTruthy()
    expect(screen.getByLabelText('Address').getAttribute('aria-invalid')).toBe('true')
  })

  it('an address already saved names the saved server and offers to use it', async () => {
    openAdd()
    type('Address', 'studio.example.com')
    await submit()
    expect(screen.getByTestId('connections-form-problem').textContent).toContain('You’ve already saved this server as Studio Mac.')
    expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Use Studio Mac' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.querySelector('[data-server-row="Studio Mac"]')?.hasAttribute('data-highlight')).toBe(true)
    expect(screen.getByTestId('connections-notice').textContent).toContain('Studio Mac was already saved')
    expect(useConnectHostStore.getState().hosts).toHaveLength(1)
  })

  it('Change address goes back to the address with the problem cleared', async () => {
    openAdd()
    type('Address', 'studio.example.com')
    await submit()
    fireEvent.click(screen.getByRole('button', { name: 'Change address' }))
    expect(screen.queryByTestId('connections-form-problem')).toBeNull()
    expect(document.activeElement).toBe(screen.getByLabelText('Address'))
    expect(screen.getByRole('button', { name: 'Continue' })).toBeTruthy()
  })

  it('an address that doesn’t answer says so, and can go on anyway', async () => {
    openAdd()
    type('Address', 'nowhere.example.com')
    await submit()
    const problem = await screen.findByTestId('connections-form-problem')
    expect(problem.textContent).toContain('Couldn’t reach nowhere.example.com.')
    fireEvent.click(screen.getByRole('button', { name: 'Continue anyway' }))
    await waitFor(() => expect(screen.getByLabelText('Username')).toBeTruthy())
  })

  it('a wrong password says so and saves nothing', async () => {
    openAdd()
    type('Address', 'office.example.com')
    type('Name (optional)', 'Office Mac')
    await submit()
    await waitFor(() => expect(screen.getByLabelText('Username')).toBeTruthy())
    expect(screen.getByRole('dialog').textContent).toContain('Sign in with your account on Office Mac')
    type('Username', 'alex')
    type('Password', 'not-it')
    await submit()
    expect(screen.getByTestId('connections-form-problem').textContent).toContain('That username or password isn’t right.')
    expect(useConnectHostStore.getState().hosts.map((h) => h.label)).toEqual(['Studio Mac'])
  })

  it('signing in lands on a done step, and the server is in the list', async () => {
    openAdd()
    type('Address', 'office.example.com')
    type('Name (optional)', 'Office Mac')
    await submit()
    await waitFor(() => expect(screen.getByLabelText('Username')).toBeTruthy())
    type('Username', 'alex')
    type('Password', 'sample-password')
    await submit()
    await waitFor(() => expect(screen.getByTestId('connections-added').textContent).toContain('Connected to Office Mac'))
    expect(useConnectHostStore.getState().hosts.map((h) => h.label).sort()).toEqual(['Office Mac', 'Studio Mac'])
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.querySelector('[data-server-row="Office Mac"]')).not.toBeNull()
  })

  it('Back keeps the address and drops the password', async () => {
    openAdd()
    type('Address', 'office.example.com')
    await submit()
    await waitFor(() => expect(screen.getByLabelText('Password')).toBeTruthy())
    type('Password', 'sample-password')
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect((screen.getByLabelText('Address') as HTMLInputElement).value).toBe('office.example.com')
    await submit()
    await waitFor(() => expect((screen.getByLabelText('Password') as HTMLInputElement).value).toBe(''))
  })
})
