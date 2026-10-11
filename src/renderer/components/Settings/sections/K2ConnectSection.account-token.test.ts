// getAccountAccessToken — the k2.dev access token Settings → Compute's
// one-click pairing uses. Reads the stored account session, refreshes it and
// persists a rotated refresh token the same way the Tunnel panel does.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const invokeMock = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { setConnectedAirgap } from '@/lib/airgap'
import { getAccountAccessToken } from './K2ConnectSection'

function respond(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  setConnectedAirgap(false)
  invokeMock.mockReset()
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
  setConnectedAirgap(false)
})

function storedSession(refreshToken: string, email: string): void {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'k2_account_session_get') return JSON.stringify({ refreshToken, email })
    if (cmd === 'k2_account_session_set') return null
    if (cmd === 'k2_secret_delete') return null
    if (cmd === 'k2_secret_get') return null
    throw new Error(`unexpected invoke ${cmd}`)
  })
}

describe('getAccountAccessToken', () => {
  it('refreshes the stored session and persists a rotated refresh token', async () => {
    storedSession('old-refresh', 'r@k2.dev')
    fetchMock.mockResolvedValue(
      respond(200, { access_token: 'fresh-access', refresh_token: 'new-refresh', user: { email: 'r@k2.dev' } }),
    )
    expect(await getAccountAccessToken()).toBe('fresh-access')
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toContain('/auth/v1/token?grant_type=refresh_token')
    expect(JSON.parse(String(init.body))).toEqual({ refresh_token: 'old-refresh' })
    expect(invokeMock).toHaveBeenCalledWith('k2_account_session_set', { refreshToken: 'new-refresh', email: 'r@k2.dev' })
  })

  it('does not rewrite the keychain when nothing rotated', async () => {
    storedSession('same', 'r@k2.dev')
    fetchMock.mockResolvedValue(respond(200, { access_token: 'a', refresh_token: 'same', user: { email: 'r@k2.dev' } }))
    expect(await getAccountAccessToken()).toBe('a')
    expect(invokeMock).not.toHaveBeenCalledWith('k2_account_session_set', expect.anything())
  })

  it('returns null when no one is signed in', async () => {
    invokeMock.mockImplementation(async () => null)
    expect(await getAccountAccessToken()).toBeNull()
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('returns null (and keeps the stored session) when the refresh fails', async () => {
    storedSession('stale', 'r@k2.dev')
    fetchMock.mockResolvedValue(respond(400, { error_description: 'Invalid Refresh Token' }))
    expect(await getAccountAccessToken()).toBeNull()
    expect(invokeMock).not.toHaveBeenCalledWith('k2_account_session_clear')
  })

  it('returns null under air-gap without touching the keychain', async () => {
    setConnectedAirgap(true)
    expect(await getAccountAccessToken()).toBeNull()
    expect(invokeMock).not.toHaveBeenCalled()
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
