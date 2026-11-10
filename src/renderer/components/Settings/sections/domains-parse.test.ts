// Settings → Custom Domains: the shape guards at the daemon boundary. A
// missing optional field takes its default; a wrong shape throws (the page
// shows it as a failed read, never a crash).

import { describe, it, expect } from 'vitest'
import { DomainsParseError, parseDomainAnswer, parseDomainList, parseFirewallStatus } from './domains-parse'

describe('parseDomainList', () => {
  it('an answer with no list of domains throws (the empty `{}` that crashed the page)', () => {
    expect(() => parseDomainList({})).toThrow(DomainsParseError)
    expect(() => parseDomainList({})).toThrow('the answer has no list of domains')
    expect(() => parseDomainList(null)).toThrow('the answer is not an object')
    expect(() => parseDomainList([])).toThrow(DomainsParseError)
    expect(() => parseDomainList({ domains: 'example.com' })).toThrow(DomainsParseError)
  })

  it('an empty list is an empty list', () => {
    expect(parseDomainList({ ok: true, domains: [] })).toEqual([])
  })

  it('a domain with only its name gets every default', () => {
    expect(parseDomainList({ domains: [{ apex: 'example.com' }] })).toEqual([
      {
        apex: 'example.com',
        zoneId: null,
        dnsWrite: false,
        status: null,
        nameservers: [],
        created: false,
        readOnly: false,
        zoneMissing: false,
        checkError: null,
        names: [],
      },
    ])
  })

  it('a domain with no name, or names that are not a list, throws', () => {
    expect(() => parseDomainList({ domains: [{ zoneId: 'z' }] })).toThrow('domains[0] has no domain name')
    expect(() => parseDomainList({ domains: ['example.com'] })).toThrow('domains[0] is not an object')
    expect(() => parseDomainList({ domains: [{ apex: 'example.com', names: {} }] })).toThrow('example.com: its names are not a list')
    expect(() => parseDomainList({ domains: [{ apex: 'example.com', names: [{ role: 'mail' }] }] })).toThrow('has no hostname')
  })

  it('a name defaults its role and certificate, and reads unix or ISO expiry', () => {
    const [d] = parseDomainList({
      domains: [
        {
          apex: 'example.com',
          names: [
            { hostname: 'example.com' },
            { hostname: 'mail.example.com', role: 'mail', cert: { state: 'issued', issuer: 'R11', expiresAt: 1798502400 } },
            { hostname: 'www.example.com', cert: { state: 'expired', expiresAt: '2026-09-21T00:00:00Z' }, renewal: { state: 'retrying', lastError: 'dns' } },
          ],
        },
      ],
    })
    expect(d.names[0]).toEqual({ hostname: 'example.com', role: 'other', cert: { state: 'missing', issuer: null, expiresAt: null }, renewal: null, app: null })
    expect(d.names[1].cert).toEqual({ state: 'issued', issuer: 'R11', expiresAt: 1798502400 })
    expect(d.names[2].cert.expiresAt).toBe(Date.parse('2026-09-21T00:00:00Z') / 1000)
    expect(d.names[2].renewal).toEqual({ state: 'retrying', lastError: 'dns' })
  })

  it('an App name keeps its records and drops broken ones; an app block without an App throws', () => {
    const [d] = parseDomainList({
      domains: [
        {
          apex: 'example.com',
          names: [
            {
              hostname: 'shop.example.com',
              role: 'app:shop',
              app: { app: 'shop', state: 'waiting_dns', records: [{ type: 'A', name: 'shop.example.com', value: '203.0.113.10' }, { type: 'TXT' }], recordsPath: 'direct' },
            },
          ],
        },
      ],
    })
    const app = d.names[0].app!
    expect(app.records).toEqual([{ type: 'A', name: 'shop.example.com', value: '203.0.113.10' }])
    expect(app.notServedYet).toBe(true)
    expect(app.recordsPath).toBe('direct')
    expect(app.pathB).toBeNull()
    expect(() =>
      parseDomainList({ domains: [{ apex: 'example.com', names: [{ hostname: 'a.example.com', app: { state: 'live' } }] }] }),
    ).toThrow('has no App name')
  })

  it('parseDomainAnswer reads an attach answer, or null', () => {
    expect(parseDomainAnswer({ ok: true, domain: { apex: 'example.dev', status: 'pending_ns' } })?.apex).toBe('example.dev')
    expect(parseDomainAnswer({ ok: true })).toBeNull()
    expect(parseDomainAnswer({ domain: { zoneId: 'z' } })).toBeNull()
  })
})

describe('parseFirewallStatus', () => {
  it('an answer with no port list throws (the empty `{}` that crashed the page)', () => {
    expect(() => parseFirewallStatus({})).toThrow('the answer has no list of open ports')
    expect(() => parseFirewallStatus({ view: {} })).toThrow(DomainsParseError)
    expect(() => parseFirewallStatus('nope')).toThrow('the answer is not an object')
    expect(() => parseFirewallStatus({ view: { public: [{ proto: 'tcp' }] } })).toThrow('ports[0] has no port number')
  })

  it('a bare port list gets every default', () => {
    expect(parseFirewallStatus({ view: { public: [] } })).toEqual({
      ports: [],
      behindNat: false,
      managedBy: 'owner',
      ssh: { port: 22, allowFrom: [] },
      guard: null,
      front: null,
    })
  })

  it('reads ports, SSH, the guard and the front; "any" means anyone', () => {
    const st = parseFirewallStatus({
      view: {
        public: [
          { port: 22, proto: 'tcp', source: 'ssh', allowFrom: ['any'], why: 'SSH' },
          { port: 6000, proto: 'udp', source: 'owner', allowFrom: ['198.51.100.0/24'], why: 'owner: game' },
        ],
        box: { behindNat: true },
        upstream: { managedBy: 'k2' },
      },
      ssh: { port: 2222, allowFrom: ['198.51.100.7'] },
      guard: { settings: { enabled: true, connsPerIp: 64, writesPerMin: 120 }, counters: { perIpRefused: 3, guardLimited: 12 } },
      front: { enabled: true, http: { on: true, serving: false } },
    })
    expect(st.ports[0].allowFrom).toEqual([])
    expect(st.ports[1]).toEqual({ port: 6000, proto: 'udp', source: 'owner', allowFrom: ['198.51.100.0/24'], why: 'owner: game' })
    expect(st.behindNat).toBe(true)
    expect(st.managedBy).toBe('k2')
    expect(st.ssh).toEqual({ port: 2222, allowFrom: ['198.51.100.7'] })
    expect(st.guard).toEqual({ enabled: true, connsPerIp: 64, writesPerMin: 120, refused: 3, limited: 12 })
    expect(st.front).toEqual({ enabled: true, httpOn: true, httpServing: false, relayNames: 0 })
  })
})
