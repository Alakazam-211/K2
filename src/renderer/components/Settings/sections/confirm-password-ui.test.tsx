// @vitest-environment jsdom

// 0.46.13 field report: an Owner's correct sign-in password kept failing at
// a card's password confirm. "Your K2 Connect password" read as the k2.dev
// account's password, and autofill offered that saved entry. The shared
// confirm step (the Thread card's, Data Sources', Garden guests') names this
// server and the login, and carries a hidden username field beside the
// password so a password manager picks this site's entry.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({ post: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (_scope: unknown, route: string, body?: unknown) => h.post(route, body),
}))

import { ConfirmStep } from './DataSourceConfirm'
import { confirmPasswordLabel, readConfirmIdentity, scopeServerName } from './confirm-password-ui'
import type { ServerScope } from '@/kessel/server-scope'

const PRIMARY = { id: 'primary', isPrimary: true, isRemote: false, hostKey: 'local', label: 'This computer' } as unknown as ServerScope

beforeEach(() => h.post.mockReset())
afterEach(() => cleanup())

describe('the password confirm names the server and the login', () => {
  it('ConfirmStep: "Your sign-in password for <server> (login <name>)" and a username field for autofill', async () => {
    const onToken = vi.fn()
    h.post.mockImplementation(async (route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['connect-password'], expiresIn: 60, server: 'rosson.k2.dev', login: 'rosson' }
      if (route === 'vault/confirm/finish') return { token: 'k2vc_ok', expiresIn: 60 }
      return {}
    })
    render(<ConfirmStep scope={PRIMARY} action="app-approve" secretId="w_1" label="App Approvals" onToken={onToken} onCancel={() => {}} />)
    const pw = (await screen.findByTestId('sources-confirm-password')) as HTMLInputElement
    expect(screen.getByText('Your sign-in password for rosson.k2.dev (login rosson)')).toBeTruthy()
    expect(screen.queryByText(/K2 Connect password/)).toBeNull()
    expect(pw.getAttribute('autocomplete')).toBe('current-password')
    const user = screen.getByTestId('sources-confirm-username') as HTMLInputElement
    expect([user.value, user.getAttribute('autocomplete'), user.type, user.hidden, user.form]).toEqual(['rosson', 'username', 'text', true, pw.form])
    fireEvent.change(pw, { target: { value: 'right one' } })
    fireEvent.submit(pw.closest('form') as HTMLFormElement)
    await waitFor(() => expect(onToken).toHaveBeenCalledWith('k2vc_ok'))
    expect(h.post.mock.calls.slice(0, 2).map((c) => c[0])).toEqual(['vault/confirm/start', 'vault/confirm/finish'])
    expect(h.post.mock.calls[1]).toEqual([
      'vault/confirm/finish',
      { action: 'app-approve', secretId: 'w_1', method: 'connect-password', label: 'App Approvals', password: 'right one' },
    ])
  })

  it('the vault password keeps its words and gets no username field', async () => {
    h.post.mockResolvedValue({ methods: ['vault-password'], expiresIn: 60, server: null, login: null })
    render(<ConfirmStep scope={PRIMARY} action="app-approve" secretId="w_1" label="x" onToken={() => {}} onCancel={() => {}} />)
    await screen.findByTestId('sources-confirm-password')
    expect(screen.getByText('Your vault password')).toBeTruthy()
    expect(screen.queryByTestId('sources-confirm-username')).toBeNull()
  })

  it('the words: the daemon’s server name, else the window’s name for a remote server', () => {
    expect(confirmPasswordLabel('connect-password', { server: 'box.k2.dev', login: 'ada' })).toBe('Your sign-in password for box.k2.dev (login ada)')
    expect(confirmPasswordLabel('connect-password', { login: 'ada' }, 'Shop server')).toBe('Your sign-in password for Shop server (login ada)')
    expect(confirmPasswordLabel('connect-password', {})).toBe('Your sign-in password for this server')
    expect(confirmPasswordLabel('vault-password', { login: 'ada' })).toBe('Your vault password')
    expect(readConfirmIdentity({ server: ' box.k2.dev ', login: '' })).toEqual({ server: 'box.k2.dev', login: null })
    expect(scopeServerName(PRIMARY)).toBeUndefined()
    expect(scopeServerName({ isPrimary: true, isRemote: true, hostKey: 'box.k2.dev', label: 'x' })).toBe('box.k2.dev')
    expect(scopeServerName({ isPrimary: false, isRemote: true, hostKey: 'box.k2.dev', label: 'Shop server' })).toBe('Shop server')
  })
})
