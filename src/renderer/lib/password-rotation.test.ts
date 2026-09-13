// lib/password-rotation — the daemon plumbing behind the forced-rotation
// step (PRD connect-login-edge-only W1/W2). Asserts the exact routes, the
// exact change-password body the daemon deserialises
// (`{currentPassword, newPassword}` — connect_users_routes.rs
// ChangePasswordReq), the status → outcome mapping, and that every call
// stays on the host's OWN origin (never the .app.k2.dev edge).

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import {
  DEFAULT_PASSWORD_POLICY,
  ROTATION_COPY,
  changeHostPassword,
  fetchPasswordPolicy,
  isPasswordChangeRequired,
  policyHint,
  validateNewPassword,
} from './password-rotation'
import type { ConnectHost } from '@/stores/connect-host'

const HOST: ConnectHost = {
  id: 'rpm',
  label: 'RPM box',
  hostname: 'rpm.k2.dev',
  username: 'alice',
  port: 443,
  secure: true,
  token: 'restricted-tok',
  remember: false,
  lastConnectedAt: null,
}

function fakeRes(status: number, jsonBody?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (jsonBody === undefined ? '' : JSON.stringify(jsonBody)),
    json: async () => {
      if (jsonBody === undefined) throw new Error('no body')
      return jsonBody
    },
  } as unknown as Response
}

beforeEach(() => {
  vi.unstubAllGlobals()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
})

describe('isPasswordChangeRequired (W2 classifier)', () => {
  it('matches the daemon 403 body, raw or extracted', () => {
    expect(isPasswordChangeRequired(403, '{"error":"password_change_required"}')).toBe(true)
    expect(isPasswordChangeRequired(403, 'password_change_required')).toBe(true)
  })
  it('never matches other statuses or bodies', () => {
    expect(isPasswordChangeRequired(401, '{"error":"password_change_required"}')).toBe(false)
    expect(isPasswordChangeRequired(403, '{"error":"Invalid or missing auth token"}')).toBe(false)
    expect(isPasswordChangeRequired(200, 'password_change_required')).toBe(false)
  })
})

describe('validateNewPassword / policyHint', () => {
  it('mirrors the daemon rules in order: length, uppercase, number, special', () => {
    const p = { minLength: 10, requireSpecial: true, requireNumber: true, requireUppercase: true }
    expect(validateNewPassword('short', p)).toBe('Password must be at least 10 characters.')
    expect(validateNewPassword('longenoughpw', p)).toBe('Password must include an uppercase letter.')
    expect(validateNewPassword('Longenoughpw', p)).toBe('Password must include a number.')
    expect(validateNewPassword('Longenoughpw1', p)).toBe('Password must include a special character.')
    expect(validateNewPassword('Longenoughpw1!', p)).toBeNull()
  })
  it('default policy is length-only', () => {
    expect(validateNewPassword('abcdefgh', DEFAULT_PASSWORD_POLICY)).toBeNull()
    expect(validateNewPassword('abcdefg', DEFAULT_PASSWORD_POLICY)).not.toBeNull()
    expect(policyHint(DEFAULT_PASSWORD_POLICY)).toBe('At least 8 characters')
    expect(
      policyHint({ minLength: 12, requireSpecial: true, requireNumber: false, requireUppercase: true }),
    ).toBe('At least 12 characters · an uppercase letter · a special character')
  })
})

describe('fetchPasswordPolicy', () => {
  it("GETs <host>/cli/users/policy?token=… on the host's OWN origin and parses it", async () => {
    const fetchMock = vi.fn(async (_url: string) =>
      fakeRes(200, { minLength: 12, requireSpecial: true, requireNumber: false, requireUppercase: true }),
    )
    vi.stubGlobal('fetch', fetchMock)
    await expect(fetchPasswordPolicy(HOST)).resolves.toEqual({
      minLength: 12,
      requireSpecial: true,
      requireNumber: false,
      requireUppercase: true,
    })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(fetchMock.mock.calls[0][0]).toBe('https://rpm.k2.dev/cli/users/policy?token=restricted-tok')
  })
  it('falls back to the default policy on a non-2xx (daemon re-validates on submit)', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(403, { error: 'nope' })))
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await expect(fetchPasswordPolicy(HOST)).resolves.toEqual(DEFAULT_PASSWORD_POLICY)
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })
})

describe('changeHostPassword', () => {
  it('POSTs the exact daemon body {currentPassword,newPassword} to <host>/cli/auth/change-password', async () => {
    const fetchMock = vi.fn(async () => fakeRes(200, { success: true }))
    vi.stubGlobal('fetch', fetchMock)
    await expect(changeHostPassword(HOST, 'temp-1', 'NewPass-123')).resolves.toEqual({ ok: true })
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://rpm.k2.dev/cli/auth/change-password?token=restricted-tok')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body as string)).toEqual({ currentPassword: 'temp-1', newPassword: 'NewPass-123' })
  })
  it('400 → weak with the daemon message verbatim', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(400, { error: 'Password must include a number.' })))
    await expect(changeHostPassword(HOST, 't', 'n')).resolves.toEqual({
      ok: false,
      kind: 'weak',
      reason: 'Password must include a number.',
    })
  })
  it('401 → bad-current with the shared copy (wrong temp OR lockout)', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(401, { error: 'unauthorized' })))
    await expect(changeHostPassword(HOST, 't', 'n')).resolves.toEqual({
      ok: false,
      kind: 'bad-current',
      reason: ROTATION_COPY.badCurrent,
    })
  })
  it('network throw → unreachable; other non-2xx → server', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('Load failed') }))
    const r1 = await changeHostPassword(HOST, 't', 'n')
    expect(r1.ok).toBe(false)
    if (!r1.ok) expect(r1.kind).toBe('unreachable')
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(503)))
    const r2 = await changeHostPassword(HOST, 't', 'n')
    expect(r2.ok).toBe(false)
    if (!r2.ok) expect(r2.kind).toBe('server')
  })
  it('W3 copy is the agreed sentence', () => {
    expect(ROTATION_COPY.lead).toBe(
      'Your server admin gave you a temporary password. Choose a new one to continue.',
    )
    expect(ROTATION_COPY.title).toBe('Set a new password')
  })
})
