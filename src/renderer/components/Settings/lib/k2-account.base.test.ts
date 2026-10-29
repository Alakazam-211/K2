// 0.46.6: the account server address comes from the daemon (connectDbUrl /
// K2_CONNECT_DB_URL); the default is unchanged and nothing ever switches
// hosts on its own.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setConnectedAirgap } from '@/lib/airgap'
import {
  DEFAULT_ACCOUNT_DB_URL,
  accountDbBase,
  claimSubdomain,
  listSubdomains,
  refreshSession,
  releaseSubdomain,
  setAccountDbBaseLoader,
  signIn,
  signOut,
  supabaseRestGet,
} from './k2-account'

function respond(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

const SESSION = { access_token: 'a', refresh_token: 'r', user: { email: 'r@k2.dev' } }

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  setConnectedAirgap(false)
  setAccountDbBaseLoader(null)
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
  setAccountDbBaseLoader(null)
})

/** Every account call, each answered with a body it accepts. */
async function callEverything(): Promise<void> {
  fetchMock.mockImplementation(async (url: string) => {
    if (url.includes('/auth/v1/token')) return respond(200, SESSION)
    if (url.includes('/rest/v1/subdomains')) return respond(200, [])
    if (url.includes('/rpc/claim_subdomain')) return respond(200, [{ claimed: true }])
    return respond(200, {})
  })
  await signIn('r@k2.dev', 'pw')
  await refreshSession('r')
  await listSubdomains('a')
  await claimSubdomain('a', 'rosson', 'dev-1')
  await releaseSubdomain('a', 'rosson', 'dev-1')
  await supabaseRestGet('compute_machines?select=sub', 'a')
  await signOut('a')
}

describe('account server base URL', () => {
  it('defaults to the Supabase project when no loader is installed', async () => {
    expect(DEFAULT_ACCOUNT_DB_URL).toBe('https://ttgcalfrzzgkxnfepkiu.supabase.co')
    expect(await accountDbBase()).toBe(DEFAULT_ACCOUNT_DB_URL)
    await callEverything()
    expect(fetchMock).toHaveBeenCalledTimes(7)
    for (const [url] of fetchMock.mock.calls as [string][]) {
      expect(url.startsWith(`${DEFAULT_ACCOUNT_DB_URL}/`)).toBe(true)
    }
  })

  it('sends every call to the daemon-resolved override', async () => {
    setAccountDbBaseLoader(async () => 'https://db.example.test/')
    expect(await accountDbBase()).toBe('https://db.example.test')
    await callEverything()
    expect(fetchMock).toHaveBeenCalledTimes(7)
    for (const [url] of fetchMock.mock.calls as [string][]) {
      expect(url.startsWith('https://db.example.test/')).toBe(true)
      expect(url).not.toContain('supabase.co')
    }
  })

  it('an older daemon with no answer means the default', async () => {
    setAccountDbBaseLoader(async () => null)
    expect(await accountDbBase()).toBe(DEFAULT_ACCOUNT_DB_URL)
  })

  it('a failed lookup is an error and nothing is sent anywhere', async () => {
    setAccountDbBaseLoader(async () => {
      throw new Error('daemon down')
    })
    await expect(signIn('r@k2.dev', 'pw')).rejects.toThrow(/account server address/)
    await expect(refreshSession('r')).rejects.toThrow(/account server address/)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('a 401 from the server is reported, never retried on another host', async () => {
    setAccountDbBaseLoader(async () => 'https://db.example.test')
    fetchMock.mockResolvedValue(respond(401, { error_description: 'Invalid login credentials' }))
    await expect(signIn('r@k2.dev', 'pw')).rejects.toThrow('Invalid login credentials')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})
