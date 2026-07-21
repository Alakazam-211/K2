// Hosted web cookie-auth helpers (Phase 2b / PRD §2.3).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const isWebMock = vi.hoisted(() => vi.fn(() => false))

vi.mock('@/lib/is-web', () => ({
  isWebClient: () => isWebMock(),
}))

import {
  cliSearchParams,
  withCliTokenQuery,
  withDaemonFetch,
  K2_WEB_CLIENT_HEADER,
  K2_WEB_CLIENT_VALUE,
  logoutWebSession,
} from './session-token'

describe('session-token web auth helpers', () => {
  beforeEach(() => {
    isWebMock.mockReturnValue(false)
  })

  afterEach(() => {
    isWebMock.mockReturnValue(false)
  })

  it('desktop: withCliTokenQuery appends token; withDaemonFetch is a no-op', () => {
    expect(withCliTokenQuery('https://h/cli/x', 'tok')).toBe('https://h/cli/x?token=tok')
    expect(withCliTokenQuery('https://h/cli/x?a=1', 'tok')).toBe('https://h/cli/x?a=1&token=tok')
    const init = withDaemonFetch({ method: 'GET' })
    expect(init).toEqual({ method: 'GET' })
    expect(init.credentials).toBeUndefined()
  })

  it('desktop: cliSearchParams always includes token', () => {
    const s = cliSearchParams('abc', { path: '/x' })
    expect(s.get('token')).toBe('abc')
    expect(s.get('path')).toBe('/x')
  })

  it('web: dual-auth — token query + cookie credentials/CSRF header', () => {
    isWebMock.mockReturnValue(true)
    // Pre-cookie daemons (e.g. 0.40.52) still need ?token=; cookie-capable
    // daemons accept either. Keep both.
    expect(withCliTokenQuery('https://h/cli/x', 'tok')).toBe('https://h/cli/x?token=tok')
    expect(withCliTokenQuery('https://h/cli/x?token=tok', 'tok')).toBe(
      'https://h/cli/x?token=tok',
    )
    const init = withDaemonFetch({
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
    expect(init.credentials).toBe('include')
    const headers = init.headers as Headers
    expect(headers.get(K2_WEB_CLIENT_HEADER)).toBe(K2_WEB_CLIENT_VALUE)
    expect(headers.get('Content-Type')).toBe('application/json')
  })

  it('web: cliSearchParams includes token when provided (dual-auth)', () => {
    isWebMock.mockReturnValue(true)
    const s = cliSearchParams('secret', { path: '/x' })
    expect(s.get('token')).toBe('secret')
    expect(s.get('path')).toBe('/x')
  })

  it('logoutWebSession is a no-op on desktop', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    await logoutWebSession('https://example.com')
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('logoutWebSession POSTs /cli/auth/logout with CSRF + credentials on web', async () => {
    isWebMock.mockReturnValue(true)
    const fetchMock = vi.fn(async () => ({ ok: true, status: 200, text: async () => '{}' }))
    vi.stubGlobal('fetch', fetchMock)
    // sessionStorage may be missing in node — logout still fires fetch
    await logoutWebSession('https://rosson.app.k2.dev')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const call = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    const [url, init] = call
    expect(url).toBe('https://rosson.app.k2.dev/cli/auth/logout')
    expect(init.method).toBe('POST')
    expect(init.credentials).toBe('include')
    const headers = init.headers as Headers
    expect(headers.get(K2_WEB_CLIENT_HEADER)).toBe(K2_WEB_CLIENT_VALUE)
  })
})
