// @vitest-environment jsdom
// Settings → K2 API Tokens: the hire scope (0.45.6, prd-agent-hire-templates-v1
// HT3/HT5/HT8): DB door + "Allow hiring agents" with limits, the owner confirm
// before the mint, the /v1-off hint, and the hire summary on a key row.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const settingsState = vi.hoisted(() => ({ apiEnabled: true, setSection: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) => sel({ projects: [] }),
}))
vi.mock('@/stores/settings', () => {
  const useSettingsStore = (sel: (s: unknown) => unknown) => sel(settingsState)
  ;(useSettingsStore as unknown as { getState: () => unknown }).getState = () => settingsState
  return { useSettingsStore }
})

import { ApiTokensSection } from './ApiTokensSection'

const HIRE_KEY = {
  id: 'key-1',
  label: 'jobs-app',
  scope: 'owner',
  createdAt: 1,
  revokedAt: null,
  disabledAt: null,
  keySet: true,
  anthropicKeySet: false,
  allowedWorkspaces: null,
  provider: null,
  baseUrl: null,
  capabilities: { hostSessions: false, canonicalMessage: false, sandboxes: false, db: false, hire: true },
  hireLimits: {
    maxMembersPerDay: 40,
    maxLiveMembers: 200,
    maxMembersPerTeam: 12,
    db: 'off',
    link: true,
    linkTargets: 'created',
    retire: true,
    reach: 'none',
    folder: 'acme',
  },
}

beforeEach(() => {
  settingsState.apiEnabled = true
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockResolvedValue({ keys: [HIRE_KEY] })
})

afterEach(() => cleanup())

describe('ApiTokensSection hire scope', () => {
  it('shows a hire key with its limits and the hire cap', async () => {
    render(<ApiTokensSection />)
    const line = await screen.findByTestId('hire-limits-key-1')
    expect(line.textContent).toBe(
      'Hires agents: 40/day · 200 live · 12 per team · db off · link · retire · folder acme',
    )
    expect(screen.getByText(/Caps: hire/)).toBeTruthy()
    expect(screen.queryByTestId('api-surface-off')).toBeNull()
  })

  it('warns when the /v1 surface is off', async () => {
    settingsState.apiEnabled = false
    render(<ApiTokensSection />)
    expect(await screen.findByTestId('api-surface-off')).toBeTruthy()
  })

  it('confirms on the Mac, then mints a hire-only key with the limits', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-1' }
      if (route === 'api-keys/create') return { id: 'k2', key: 'k2sk_secret' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByText('host-sessions'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '5' } })
    fireEvent.click(screen.getByLabelText('May link teams'))
    fireEvent.change(screen.getByLabelText('Folder under the hire root'), { target: { value: 'acme' } })
    fireEvent.click(screen.getByText('Create'))
    await waitFor(() => expect(screen.getByText('k2sk_secret')).toBeTruthy())
    const routes = daemonCliPost.mock.calls.map((c) => c[1])
    expect(routes).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'api-keys/create'])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({ action: 'hire-key-mint' })
    expect(daemonCliPost.mock.calls[1][2]).toMatchObject({ action: 'hire-key-mint', method: 'local-auth' })
    const body = daemonCliPost.mock.calls[2][2] as Record<string, unknown>
    expect(body.confirmToken).toBe('tok-1')
    expect(body.capabilities).toEqual({ hostSessions: false, canonicalMessage: false, sandboxes: false, db: false })
    expect(body.hire).toEqual({
      maxMembersPerDay: 5,
      maxLiveMembers: 100,
      maxMembersPerTeam: 12,
      db: 'off',
      link: true,
      retire: false,
      reach: 'none',
      folder: 'acme',
    })
  })

  it('asks for the vault password when there is no Touch ID', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string, body: Record<string, unknown>) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'vault/confirm/finish') {
        expect(body.password).toBe('correct horse battery')
        return { token: 'tok-2' }
      }
      if (route === 'api-keys/create') return { id: 'k3', key: 'k2sk_two' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    fireEvent.click(screen.getByText('Create'))
    const pw = await screen.findByLabelText('Confirm password')
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.click(screen.getByText('Confirm and create'))
    await waitFor(() => expect(screen.getByText('k2sk_two')).toBeTruthy())
    expect((daemonCliPost.mock.calls[2][2] as Record<string, unknown>).confirmToken).toBe('tok-2')
  })

  it('refuses database access for hired agents without the key db door, before any confirm', async () => {
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByLabelText('Allow hiring agents'))
    const sel = screen.getByLabelText('Database access for hired agents') as HTMLSelectElement
    expect((sel.querySelector('option[value="write"]') as HTMLOptionElement).disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '0' } })
    fireEvent.click(screen.getByText('Create'))
    expect(await screen.findByText(/Agents per day must be a whole number/)).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a plain key mints without a confirm and sends the db door', async () => {
    daemonCliPost.mockResolvedValue({ id: 'k4', key: 'k2sk_plain' })
    render(<ApiTokensSection />)
    fireEvent.click(await screen.findByText('Create API key'))
    fireEvent.click(screen.getByText(/^db \(create/))
    fireEvent.click(screen.getByText('Create'))
    await waitFor(() => expect(screen.getByText('k2sk_plain')).toBeTruthy())
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    const body = daemonCliPost.mock.calls[0][2] as Record<string, unknown>
    expect(body.capabilities).toEqual({ hostSessions: true, canonicalMessage: false, sandboxes: false, db: true })
    expect(body.hire).toBeUndefined()
  })
})
