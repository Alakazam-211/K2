// Tests for validateHost — the K2 Connect sign-in / "test connection"
// pre-flight that hits a candidate host's /boot-status?token=.

import { describe, it, expect, vi, afterEach } from 'vitest'
import { validateHost } from './connect-validate'

function mockFetch(impl: (url: string) => Partial<Response> & { json?: () => Promise<unknown> }) {
  vi.stubGlobal('fetch', vi.fn(async (url: string) => impl(url) as unknown as Response))
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('validateHost', () => {
  it('accepts a 2xx /boot-status with a compatible protocol', async () => {
    let seen = ''
    mockFetch((url) => {
      seen = url
      return { ok: true, status: 200, json: async () => ({ version: '0.40.0', protocol: 1, phase: 'ready', detail: '' }) }
    })
    const r = await validateHost({ hostname: 'rosson.k2.dev', port: 443, secure: true, token: 'tok' })
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.version).toBe('0.40.0')
    // secure + 443 omits the port; token rides as a query param.
    expect(seen).toBe('https://rosson.k2.dev/boot-status?token=tok')
  })

  it('builds an http URL WITH the port for a non-secure LAN host', async () => {
    let seen = ''
    mockFetch((url) => {
      seen = url
      return { ok: true, status: 200, json: async () => ({ version: '1', protocol: 1, phase: 'ready', detail: '' }) }
    })
    await validateHost({ hostname: '10.0.0.5', port: 47800, secure: false, token: 'x' })
    expect(seen).toBe('http://10.0.0.5:47800/boot-status?token=x')
  })

  it('reports a rejected token on 401', async () => {
    mockFetch(() => ({ ok: false, status: 401 }))
    const r = await validateHost({ hostname: 'h', port: 443, secure: true, token: 'bad' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.reason).toMatch(/rejected/i)
  })

  it('reports unreachable on a network error', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('ECONNREFUSED') }))
    const r = await validateHost({ hostname: 'nope.invalid', port: 443, secure: true, token: 't' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.reason).toMatch(/reach/i)
  })

  it('rejects a too-old protocol', async () => {
    mockFetch(() => ({ ok: true, status: 200, json: async () => ({ version: '1', protocol: 0, phase: 'ready', detail: '' }) }))
    const r = await validateHost({ hostname: 'h', port: 443, secure: true, token: 't' }, 1)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.reason).toMatch(/protocol/i)
  })
})
