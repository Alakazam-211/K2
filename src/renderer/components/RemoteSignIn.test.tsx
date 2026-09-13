// @vitest-environment jsdom
//
// RemoteSignIn — forced-rotation step (PRD connect-login-edge-only W1–W4)
// and the D3 404 message. Real connect-host store, stubbed fetch, mocked
// Tauri invoke (fake keychain), GateChrome mocked (it pulls the whole
// top-bar). Fail loud: exact URLs, exact bodies, exact copy.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import React from 'react'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

const fakeKeychain = new Map<string, string>()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'connect_hosts_read':
        return '[]'
      case 'connect_hosts_write':
        return undefined
      case 'k2_secret_set':
        fakeKeychain.set(`${args!.service}:${args!.account}`, args!.secret as string)
        return undefined
      case 'k2_secret_get':
        return fakeKeychain.get(`${args!.service}:${args!.account}`) ?? null
      case 'k2_secret_delete':
        fakeKeychain.delete(`${args!.service}:${args!.account}`)
        return undefined
      default:
        throw new Error(`unexpected invoke: ${cmd}`)
    }
  }),
}))
vi.mock('./TopBar/GateChrome', () => ({ default: () => <div data-testid="gate-chrome" /> }))

import { RemoteSignIn } from './RemoteSignIn'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  type ConnectHost,
} from '@/stores/connect-host'
import { ROTATION_COPY } from '@/lib/password-rotation'
import { LOGIN_404_K2DEV_MESSAGE } from '@/lib/login-url'

function makeHost(overrides?: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'rpm',
    label: 'RPM box',
    hostname: 'rpm.k2.dev',
    username: 'alice',
    port: 443,
    secure: true,
    token: '',
    remember: false,
    lastConnectedAt: null,
    ...overrides,
  }
}

function jsonRes(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => JSON.stringify(body),
    json: async () => body,
  } as unknown as Response
}

type Call = { url: string; init: RequestInit }
function calls(fetchMock: ReturnType<typeof vi.fn>): Call[] {
  return fetchMock.mock.calls.map(([url, init]) => ({ url: url as string, init: (init ?? {}) as RequestInit }))
}

/** A scripted daemon: temp password → restricted session; change-password
 *  revokes it; the NEW password → a full session. */
function scriptedDaemon(opts: { policy?: unknown } = {}) {
  let rotated = false
  return vi.fn(async (url: string, init?: RequestInit) => {
    if (url === 'https://rpm.app.k2.dev/cli/auth/login') {
      const body = JSON.parse((init?.body as string) ?? '{}') as { username: string; password: string }
      if (body.password === 'temp-1' && !rotated) {
        return jsonRes(200, { token: 'restricted', username: body.username, expiresAt: 'x', mustChangePassword: true })
      }
      if (body.password === 'NewPass-123' && rotated) {
        return jsonRes(200, { token: 'full', username: body.username, expiresAt: 'x', mustChangePassword: false })
      }
      return jsonRes(401, { error: 'invalid username or password' })
    }
    if (url === 'https://rpm.k2.dev/cli/users/policy?token=restricted') {
      return jsonRes(200, opts.policy ?? { minLength: 8, requireSpecial: false, requireNumber: true, requireUppercase: false })
    }
    if (url === 'https://rpm.k2.dev/cli/auth/change-password?token=restricted') {
      const body = JSON.parse((init?.body as string) ?? '{}') as { currentPassword: string; newPassword: string }
      if (body.currentPassword !== 'temp-1') return jsonRes(401, { error: 'unauthorized' })
      rotated = true
      return jsonRes(200, { success: true })
    }
    throw new Error(`unexpected fetch: ${url}`)
  })
}

beforeEach(() => {
  fakeKeychain.clear()
  __resetConnectHostStoreForTests()
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('RemoteSignIn — forced rotation after a temporary-password login (W1/W3)', () => {
  it('login → "Set a new password" (not switched) → change-password body → re-login → switched', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().requestSignIn(host)
    const fetchMock = scriptedDaemon()
    vi.stubGlobal('fetch', fetchMock)

    render(<RemoteSignIn host={host} />)

    fireEvent.change(screen.getByPlaceholderText('Server password'), { target: { value: 'temp-1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }))

    // The rotation step, with the W3 copy, and NOT signed in yet.
    await screen.findByText(ROTATION_COPY.title)
    expect(screen.getByText(ROTATION_COPY.lead)).toBeTruthy()
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    // Temp password prefilled from what was just typed.
    const tempInput = screen.getByLabelText(ROTATION_COPY.currentLabel) as HTMLInputElement
    expect(tempInput.value).toBe('temp-1')
    // Policy hint reflects the host's policy (requireNumber).
    await screen.findByText('At least 8 characters · a number')

    // Client-side policy check fires before any network call.
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.newLabel), { target: { value: 'nonumbers' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'nonumbers' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))
    await screen.findByText('Password must include a number.')
    expect(calls(fetchMock).some((c) => c.url.includes('change-password'))).toBe(false)

    // Mismatch check.
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.newLabel), { target: { value: 'NewPass-123' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'NewPass-124' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))
    await screen.findByText(ROTATION_COPY.mismatch)

    // Real submit.
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'NewPass-123' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))

    await waitFor(() => {
      expect(useConnectHostStore.getState().activeHost).not.toBe('local')
    })
    const s = useConnectHostStore.getState()
    expect((s.activeHost as ConnectHost).token).toBe('full')
    expect(s.pendingSignIn).toBeNull()
    expect(s.signInRotate).toBe(false)

    const seq = calls(fetchMock)
    const change = seq.find((c) => c.url.includes('/cli/auth/change-password'))!
    expect(change.init.method).toBe('POST')
    expect(JSON.parse(change.init.body as string)).toEqual({ currentPassword: 'temp-1', newPassword: 'NewPass-123' })
    // Two logins, both through the edge URL: temp, then the NEW password.
    const logins = seq.filter((c) => c.url === 'https://rpm.app.k2.dev/cli/auth/login')
    expect(logins.map((c) => (JSON.parse(c.init.body as string) as { password: string }).password)).toEqual([
      'temp-1',
      'NewPass-123',
    ])
    // Order: login → policy → change-password → login.
    const order = seq.map((c) => c.url.replace(/\?.*$/, ''))
    expect(order.indexOf('https://rpm.k2.dev/cli/auth/change-password')).toBeGreaterThan(
      order.indexOf('https://rpm.k2.dev/cli/users/policy'),
    )
    expect(order.lastIndexOf('https://rpm.app.k2.dev/cli/auth/login')).toBeGreaterThan(
      order.indexOf('https://rpm.k2.dev/cli/auth/change-password'),
    )
  })

  it('"Remember password" stores the NEW password, never the temporary one', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().requestSignIn(host)
    vi.stubGlobal('fetch', scriptedDaemon())
    render(<RemoteSignIn host={host} />)

    fireEvent.click(screen.getByLabelText(/Remember password/))
    fireEvent.change(screen.getByPlaceholderText('Server password'), { target: { value: 'temp-1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(ROTATION_COPY.title)
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.newLabel), { target: { value: 'NewPass-123' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'NewPass-123' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))

    await waitFor(() => {
      expect(fakeKeychain.get(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:rpm`)).toBe('NewPass-123')
    })
    expect(useConnectHostStore.getState().hosts.find((h) => h.id === 'rpm')?.remember).toBe(true)
  })

  it('a wrong temporary password on change-password surfaces the 401 copy and stays on the step', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().requestSignIn(host)
    vi.stubGlobal('fetch', scriptedDaemon())
    render(<RemoteSignIn host={host} />)
    fireEvent.change(screen.getByPlaceholderText('Server password'), { target: { value: 'temp-1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(ROTATION_COPY.title)
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.currentLabel), { target: { value: 'temp-WRONG' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.newLabel), { target: { value: 'NewPass-123' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'NewPass-123' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))
    await screen.findByText(ROTATION_COPY.badCurrent)
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(screen.getByText(ROTATION_COPY.title)).toBeTruthy()
  })
})

describe('RemoteSignIn — opened directly on the rotation step (W2 / pickHost)', () => {
  it('signInRotate: no login attempt; remembered temp password prefilled; completes with a switch', async () => {
    const host = makeHost({ token: 'restricted', remember: true })
    useConnectHostStore.getState().addHost(host)
    fakeKeychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:rpm`, 'temp-1')
    useConnectHostStore.getState().requestPasswordRotation(host)
    const fetchMock = scriptedDaemon()
    vi.stubGlobal('fetch', fetchMock)

    render(<RemoteSignIn host={host} />)
    await screen.findByText(ROTATION_COPY.title)
    await waitFor(() => {
      expect((screen.getByLabelText(ROTATION_COPY.currentLabel) as HTMLInputElement).value).toBe('temp-1')
    })
    // No password-form login was fired — the session already exists.
    expect(calls(fetchMock).filter((c) => c.url.includes('/cli/auth/login'))).toHaveLength(0)

    fireEvent.change(screen.getByLabelText(ROTATION_COPY.newLabel), { target: { value: 'NewPass-123' } })
    fireEvent.change(screen.getByLabelText(ROTATION_COPY.confirmLabel), { target: { value: 'NewPass-123' } })
    fireEvent.click(screen.getByRole('button', { name: ROTATION_COPY.submit }))
    await waitFor(() => {
      expect(useConnectHostStore.getState().activeHost).not.toBe('local')
    })
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('full')
    expect(fakeKeychain.get(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:rpm`)).toBe('NewPass-123')
  })

  it('Cancel on the direct step closes the overlay', async () => {
    const host = makeHost({ token: 'restricted' })
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().requestPasswordRotation(host)
    vi.stubGlobal('fetch', scriptedDaemon())
    render(<RemoteSignIn host={host} />)
    await screen.findByText(ROTATION_COPY.title)
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
    expect(useConnectHostStore.getState().signInRotate).toBe(false)
  })
})

describe('RemoteSignIn — D3: 404 from a hosted host', () => {
  it('shows the update-both-sides message', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().requestSignIn(host)
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes(404, { error: 'not_found' })))
    render(<RemoteSignIn host={host} />)
    fireEvent.change(screen.getByPlaceholderText('Server password'), { target: { value: 'pw' } })
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(LOGIN_404_K2DEV_MESSAGE)
    expect(useConnectHostStore.getState().activeHost).toBe('local')
  })
})
