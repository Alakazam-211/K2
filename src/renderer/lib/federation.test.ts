// GAP #3 — owner-driven cross-server auto-pair + add-remote-connection.
//
// These lock the WIRE behaviour of the renderer half: which daemon each call
// targets (active "local" vs a saved remote host), the auto-pair SEQUENCE
// (mutual trust, SAS read back programmatically), the subdomain each side
// gets pinned with (so the daemon send-gate's reconstructed
// `<agent>@<subdomain>.k2.dev` matches the recorded `<agent>@<host>`),
// idempotency (skip when already mutually trusted), and fail-loud on an
// unknown/signed-out remote host.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// Active ("local") daemon = a fixed loopback base + owner token.
const LOCAL_BASE = 'http://127.0.0.1:9000'
const LOCAL_TOKEN = 'LOCALTOK'
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({
    port: 9000,
    token: LOCAL_TOKEN,
    host: '127.0.0.1',
    secure: false,
  })),
  daemonHttpBase: vi.fn(() => LOCAL_BASE),
}))

// Saved remote address book — `rpm.k2.dev` is signed in (has a token).
let hosts: Array<Record<string, unknown>> = []
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => ({ hosts }) },
}))

import {
  parseAgentAtHost,
  autoPairWithHost,
  addRemoteConnection,
  listRemoteConnections,
  getPubkey,
} from '@/lib/federation'

const REMOTE_BASE = 'https://rpm.k2.dev' // secure + port 443 → no :port

function signedInRemote() {
  return [
    {
      id: 'h1',
      label: 'RPM',
      hostname: 'rpm.k2.dev',
      port: 443,
      secure: true,
      token: 'REMOTETOK',
      remember: true,
      lastConnectedAt: null,
    },
  ]
}

interface Recorded {
  url: string
  method: string
  body: unknown
}

/** Install a fetch stub that answers each daemon route. `peers` controls the
 *  idempotency state per base. Records every call for assertion. */
function installFetch(opts: {
  localPeers?: unknown[]
  remotePeers?: unknown[]
}): Recorded[] {
  const recorded: Recorded[] = []
  const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
    const u = new URL(url)
    const isLocal = u.host === '127.0.0.1:9000'
    const method = init?.method ?? 'GET'
    const body = init?.body ? JSON.parse(String(init.body)) : undefined
    recorded.push({ url, method, body })

    const ok = (data: unknown) => ({
      ok: true,
      status: 200,
      text: async () => JSON.stringify(data),
    })

    const path = u.pathname
    if (path === '/cli/federation/pubkey') {
      return ok(
        isLocal
          ? { public_key_pem: 'LOCALPEM', fingerprint: 'LOCALFP', subdomain: 'mybox' }
          : { public_key_pem: 'REMOTEPEM', fingerprint: 'REMOTEFP', subdomain: 'rpm' },
      )
    }
    if (path === '/cli/federation/peers') {
      return ok({ peers: isLocal ? opts.localPeers ?? [] : opts.remotePeers ?? [] })
    }
    if (path === '/cli/federation/pair/request') {
      return ok({ fingerprint: 'PFP', sas: '424242' })
    }
    if (path === '/cli/federation/pair/confirm') {
      return ok({ ok: true })
    }
    if (path === '/cli/connections') {
      return ok({ success: true })
    }
    return { ok: false, status: 404, text: async () => '{"error":"not found"}' }
  })
  vi.stubGlobal('fetch', fetchMock)
  return recorded
}

describe('parseAgentAtHost', () => {
  it('splits a valid agent@host on the last @', () => {
    expect(parseAgentAtHost('ai@rpm.k2.dev')).toEqual({ agent: 'ai', host: 'rpm.k2.dev' })
    expect(parseAgentAtHost('a@b@host.example')).toEqual({ agent: 'a@b', host: 'host.example' })
  })
  it('rejects non-addresses (no @, empty side, whitespace-only)', () => {
    expect(parseAgentAtHost('local-workspace')).toBeNull()
    expect(parseAgentAtHost('@host')).toBeNull()
    expect(parseAgentAtHost('agent@')).toBeNull()
    expect(parseAgentAtHost('   ')).toBeNull()
  })
})

describe('autoPairWithHost', () => {
  beforeEach(() => {
    hosts = signedInRemote()
    vi.unstubAllGlobals()
  })

  it('establishes MUTUAL trust: pins local on remote and remote on local with the right subdomains', async () => {
    const rec = installFetch({ localPeers: [], remotePeers: [] })
    await autoPairWithHost('rpm.k2.dev')

    const pairReqs = rec.filter((r) => r.url.includes('/cli/federation/pair/request'))
    expect(pairReqs).toHaveLength(2)

    // (a) REMOTE side pins LOCAL's key under LOCAL's own subdomain.
    const onRemote = pairReqs.find((r) => r.url.startsWith(REMOTE_BASE))
    expect(onRemote?.body).toMatchObject({ public_key_pem: 'LOCALPEM', subdomain: 'mybox' })

    // (b) LOCAL side pins REMOTE's key under the host-derived subdomain
    //     (`rpm.k2.dev` → `rpm`) so the daemon send-gate matches.
    const onLocal = pairReqs.find((r) => r.url.startsWith(LOCAL_BASE))
    expect(onLocal?.body).toMatchObject({ public_key_pem: 'REMOTEPEM', subdomain: 'rpm' })

    // Each pair/request is followed by an owner pair/confirm echoing the SAS.
    const confirms = rec.filter((r) => r.url.includes('/cli/federation/pair/confirm'))
    expect(confirms).toHaveLength(2)
    for (const c of confirms) {
      expect(c.body).toMatchObject({ fingerprint: 'PFP', sas: '424242' })
    }
  })

  it('is idempotent — no pairing when both directions are already trusted', async () => {
    const rec = installFetch({
      localPeers: [{ fingerprint: 'REMOTEFP', trust: 'trusted' }],
      remotePeers: [{ fingerprint: 'LOCALFP', trust: 'trusted' }],
    })
    await autoPairWithHost('rpm.k2.dev')
    expect(rec.some((r) => r.url.includes('/cli/federation/pair/'))).toBe(false)
  })

  it('fails loud when the host is not a saved/signed-in server', async () => {
    hosts = [] // address book empty
    installFetch({})
    await expect(autoPairWithHost('rpm.k2.dev')).rejects.toThrow(/not a saved server/i)
  })
})

describe('addRemoteConnection', () => {
  beforeEach(() => {
    hosts = signedInRemote()
    vi.unstubAllGlobals()
  })

  it('pairs then records the connection on the LOCAL daemon for the source workspace', async () => {
    const rec = installFetch({ localPeers: [], remotePeers: [] })
    await addRemoteConnection('/Users/me/ws', 'ai@rpm.k2.dev')

    const conn = rec.find((r) => r.url.includes('/cli/connections'))
    expect(conn).toBeDefined()
    expect(conn?.url.startsWith(LOCAL_BASE)).toBe(true)
    const u = new URL(conn!.url)
    expect(u.searchParams.get('project')).toBe('/Users/me/ws')
    expect(u.searchParams.get('action')).toBe('add')
    expect(u.searchParams.get('target')).toBe('ai@rpm.k2.dev')
  })

  it('rejects a malformed target before touching the network', async () => {
    installFetch({})
    await expect(addRemoteConnection('/Users/me/ws', 'not-an-address')).rejects.toThrow(
      /not a valid remote agent address/i,
    )
  })
})

describe('listRemoteConnections', () => {
  beforeEach(() => {
    hosts = signedInRemote()
    vi.unstubAllGlobals()
  })

  it('returns only remote entries from the connections list', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        status: 200,
        text: async () =>
          JSON.stringify({
            connections: [
              { projectName: 'LocalWs', remote: false },
              { remote: true, address: 'ai@rpm.k2.dev', host: 'rpm.k2.dev', agent: 'ai' },
            ],
          }),
      })),
    )
    const out = await listRemoteConnections('/Users/me/ws')
    expect(out).toEqual([
      { remote: true, address: 'ai@rpm.k2.dev', host: 'rpm.k2.dev', agent: 'ai' },
    ])
  })

  it('returns [] for an empty source path (no network call)', async () => {
    const f = vi.fn()
    vi.stubGlobal('fetch', f)
    expect(await listRemoteConnections('')).toEqual([])
    expect(f).not.toHaveBeenCalled()
  })
})

// The federation client (cliGet/cliPost) must survive a remote restart/update:
// the dead pooled WKWebView socket throws, withRemoteRetry evicts+reopens it.
// A non-2xx (404 federation-off) is authoritative and must NOT be retried.
describe('cliGet retry-on-network-error (via getPubkey)', () => {
  beforeEach(() => {
    hosts = signedInRemote()
    vi.unstubAllGlobals()
  })

  it('a connection error then a 200 RESOLVES (retry happened)', async () => {
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('Load failed')) // dead pooled socket
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        text: async () =>
          JSON.stringify({ public_key_pem: 'PEM', fingerprint: 'FP', subdomain: 'mybox' }),
      })
    vi.stubGlobal('fetch', fetchMock)

    await expect(getPubkey()).resolves.toMatchObject({ fingerprint: 'FP' })
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it('a 404 (federation off) is NOT retried — surfaces immediately (single fetch)', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: false,
      status: 404,
      text: async () => JSON.stringify({ error: 'federation disabled' }),
    }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(getPubkey()).rejects.toThrow(/federation disabled/i)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})
