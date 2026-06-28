// Unit tests for the per-host op helpers (Settings → Connections tiles).
// Pure logic only — the fetch wrappers are exercised in the app; here we lock
// the credential resolution + the update-check → display-string mapping + the
// federation badge copy, the bits that must never drift from the daemon shape.

import { describe, it, expect, vi, afterEach } from 'vitest'
import {
  remoteCreds,
  summarizeCheck,
  federationBadgeText,
  hostOpPost,
  hostOpGet,
  type HostCreds,
} from './host-ops'
import type { UpdateCheckResult } from '@/components/Settings/sections/update-host'

/** A minimal Response stand-in covering exactly what host-ops `parse` reads. */
function fakeRes({ status = 200, body = '' }: { status?: number; body?: string } = {}): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => body,
  } as unknown as Response
}

const CREDS: HostCreds = { base: 'https://rosson.k2.dev', token: 'tok' }

describe('remoteCreds — per-host base/token resolution', () => {
  it('builds an https base and omits port 443 for a secure host', () => {
    expect(
      remoteCreds({ hostname: 'rosson.k2.dev', port: 443, secure: true, token: 'tok' }),
    ).toEqual({ base: 'https://rosson.k2.dev', token: 'tok' })
  })

  it('keeps a non-443 port even when secure', () => {
    expect(
      remoteCreds({ hostname: 'rosson.k2.dev', port: 8443, secure: true, token: 'tok' }),
    ).toEqual({ base: 'https://rosson.k2.dev:8443', token: 'tok' })
  })

  it('uses http + explicit port for a plain LAN host', () => {
    expect(
      remoteCreds({ hostname: '192.168.1.5', port: 4622, secure: false, token: 't2' }),
    ).toEqual({ base: 'http://192.168.1.5:4622', token: 't2' })
  })

  it('yields an empty token (signed out) when the host has none', () => {
    expect(
      remoteCreds({ hostname: 'h', port: 443, secure: true, token: '' }).token,
    ).toBe('')
  })
})

describe('summarizeCheck — update-check response → display', () => {
  const base: UpdateCheckResult = { current: '0.40.10', latest: '0.40.10', available: false }

  it('reports up-to-date with the current version', () => {
    const s = summarizeCheck('Hetzner box', base)
    expect(s.kind).toBe('up-to-date')
    expect(s.text).toContain('Hetzner box')
    expect(s.text).toContain('v0.40.10')
  })

  it('reports an available update with the version delta + latest', () => {
    const s = summarizeCheck('Hetzner box', {
      current: '0.40.10',
      latest: '0.40.12',
      available: true,
    })
    expect(s.kind).toBe('available')
    if (s.kind === 'available') {
      expect(s.latest).toBe('0.40.12')
      expect(s.text).toContain('0.40.10')
      expect(s.text).toContain('0.40.12')
    }
  })

  it('reports newer-no-artifact distinctly from up-to-date', () => {
    const s = summarizeCheck('Hetzner box', {
      current: '0.40.10',
      latest: '0.40.12',
      available: false,
      newerNoArtifact: true,
      platform: 'linux-aarch64',
    })
    expect(s.kind).toBe('newer-no-artifact')
    expect(s.text).toContain('linux-aarch64')
    expect(s.text.toLowerCase()).not.toContain('up to date')
  })
})

describe('federationBadgeText', () => {
  it('maps each state to its label', () => {
    expect(federationBadgeText('loading')).toBe('Federation: …')
    expect(federationBadgeText('on')).toBe('Federation: on')
    expect(federationBadgeText('off')).toBe('Federation: off')
    expect(federationBadgeText('unknown')).toBe('Federation: —')
  })
})

// The tile fix: a remote restart/update leaves a DEAD pooled WKWebView socket
// so the next fetch throws at the network layer. withRemoteRetry must retry
// (the throw evicts the socket; the retry opens a fresh one), but must NOT
// retry an authoritative non-2xx (401 stale token / 403 not-owner).
describe('hostOpPost / hostOpGet — survive a remote restart (retry-on-network-error)', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('hostOpPost: a connection error then a 200 RESOLVES (retry happened)', async () => {
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('Load failed')) // dead pooled socket
      .mockResolvedValueOnce(fakeRes({ body: JSON.stringify({ ok: true }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(hostOpPost(CREDS, 'daemon/restart')).resolves.toEqual({ ok: true })
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it('hostOpGet: a connection error then a 200 RESOLVES (retry happened)', async () => {
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('Failed to fetch'))
      .mockResolvedValueOnce(fakeRes({ body: JSON.stringify({ federationEnabled: true }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(hostOpGet(CREDS, 'settings/get')).resolves.toEqual({ federationEnabled: true })
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it('hostOpPost: a 401 is NOT retried — surfaces immediately (single fetch)', async () => {
    const fetchMock = vi.fn(async () =>
      fakeRes({ status: 401, body: JSON.stringify({ error: 'session expired' }) }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await expect(hostOpPost(CREDS, 'daemon/restart')).rejects.toThrow('session expired')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('hostOpGet: a 500 app error is NOT retried (single fetch)', async () => {
    const fetchMock = vi.fn(async () =>
      fakeRes({ status: 500, body: JSON.stringify({ error: 'boom' }) }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await expect(hostOpGet(CREDS, 'settings/get')).rejects.toThrow('boom')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})
