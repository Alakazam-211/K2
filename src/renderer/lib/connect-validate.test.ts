// Tests for validateHost — the K2 Connect sign-in / "test connection"
// pre-flight that hits a candidate host's /boot-status?token=.

import { describe, it, expect, vi, afterEach } from 'vitest'
import { parseServerUrl, validateHost } from './connect-validate'

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

describe('parseServerUrl', () => {
  it('keeps hosted names as https/443', () => {
    const r = parseServerUrl('rosson.k2.dev')
    expect(r.ok).toBe(true)
    if (!r.ok) throw new Error(`expected ok, got ${r.reason}`)
    expect(r.hostname).toBe('rosson.k2.dev')
    expect(r.secure).toBe(true)
    expect(r.port).toBe(443)
  })

  it('treats RFC1918 host:port with no scheme as http', () => {
    const r = parseServerUrl('192.168.1.5:60710')
    expect(r.ok).toBe(true)
    if (!r.ok) throw new Error(`expected ok, got ${r.reason}`)
    expect(r.hostname).toBe('192.168.1.5')
    expect(r.secure).toBe(false)
    expect(r.port).toBe(60710)
  })

  it('requires a port on RFC1918 with no scheme', () => {
    const r = parseServerUrl('192.168.1.5')
    expect(r.ok).toBe(false)
    if (r.ok) throw new Error('expected port-required error')
    expect(r.reason).toMatch(/port/i)
  })

  it('rejects explicit https:// to an RFC1918 address', () => {
    const r = parseServerUrl('https://192.168.1.5')
    expect(r.ok).toBe(false)
    if (r.ok) throw new Error('expected https+LAN teaching error')
    expect(r.reason).toMatch(/HTTP/i)
    const withPort = parseServerUrl('https://192.168.1.5:60710')
    expect(withPort.ok).toBe(false)
    if (withPort.ok) throw new Error('expected https+LAN teaching error even with a port')
  })

  it('accepts explicit http:// RFC1918 with a port', () => {
    const r = parseServerUrl('http://10.0.0.9:60710')
    expect(r.ok).toBe(true)
    if (!r.ok) throw new Error(`expected ok, got ${r.reason}`)
    expect(r.secure).toBe(false)
    expect(r.port).toBe(60710)
  })

  it('treats 172.16/12 and 169.254/16 as LAN http', () => {
    const a = parseServerUrl('172.16.0.1:9')
    expect(a.ok).toBe(true)
    if (!a.ok) throw new Error(`expected ok, got ${a.reason}`)
    expect(a.secure).toBe(false)
    const b = parseServerUrl('169.254.1.1:9')
    expect(b.ok).toBe(true)
    if (!b.ok) throw new Error(`expected ok, got ${b.reason}`)
    expect(b.secure).toBe(false)
  })
})
