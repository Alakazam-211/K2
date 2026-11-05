// @vitest-environment jsdom

// Settings → Network → Custom Domains: custom-domain attach A8.1 states
// (pending nameservers, auto-added, Check again, 409 owned elsewhere).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ token: 'tok', port: 1 })),
  daemonHttpBase: () => 'http://daemon.test',
}))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'local' }))

// DN12e: capture the `domains_changed` subscription so a test can fire it.
const domainsChangedHandlers = new Set<(e: unknown) => void>()
vi.mock('@/stores/session-events', () => ({
  onDomainsChanged: vi.fn((_scope: unknown, fn: (e: unknown) => void) => {
    domainsChangedHandlers.add(fn)
    return () => {
      domainsChangedHandlers.delete(fn)
    }
  }),
}))

import {
  DomainsSection,
  OWNED_ELSEWHERE_COPY,
  ZONE_MISSING_COPY,
  type DomainRow,
} from './DomainsSection'

const PENDING: DomainRow = {
  apex: 'pending.example',
  zoneId: 'z-pend',
  dnsWrite: false,
  status: 'pending_ns',
  nameservers: ['ns1.k2.dev', 'ns2.k2.dev'],
  created: true,
  names: [],
}
const ACTIVE: DomainRow = { ...PENDING, status: 'active', dnsWrite: true }
const BYO: DomainRow = {
  apex: 'byo.example',
  zoneId: null,
  dnsWrite: false,
  status: null,
  nameservers: [],
  created: false,
  names: [],
}

type Call = { method: string; path: string; body: unknown }
let calls: Call[] = []
let listRows: DomainRow[] = []
let postHandler: (path: string, body: Record<string, string>) => { status: number; body: unknown }
// 0.46.6: the Firewall panel's GET /cli/firewall (its own answer, so the
// Domains tests' error hints never show twice).
let firewallGet: { status: number; body: unknown } = { status: 404, body: { ok: false, error: { code: 'off', hint: 'no firewall in this test' } } }

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

beforeEach(() => {
  cleanup()
  calls = []
  listRows = []
  firewallGet = { status: 404, body: { ok: false, error: { code: 'off', hint: 'no firewall in this test' } } }
  postHandler = () => ({ status: 404, body: { ok: false, error: { code: 'not_found', hint: 'nope' } } })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const path = new URL(url).pathname
      const method = init?.method ?? 'GET'
      const body = init?.body ? JSON.parse(String(init.body)) : undefined
      calls.push({ method, path, body })
      if (method === 'GET' && path === '/cli/domains') {
        return jsonResponse(200, { ok: true, domains: listRows })
      }
      if (method === 'GET' && path === '/cli/firewall') {
        return jsonResponse(firewallGet.status, firewallGet.body)
      }
      const r = postHandler(path, (body ?? {}) as Record<string, string>)
      return jsonResponse(r.status, r.body)
    }),
  )
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText: vi.fn(async () => undefined) },
    configurable: true,
  })
})

describe('DomainsSection — A8.1 pending nameservers', () => {
  it('a pending_ns row shows Pending nameservers, the NS list with copy buttons, and the auto-added note', async () => {
    listRows = [PENDING]
    render(<DomainsSection />)
    const block = await screen.findByTestId('pending-ns-pending.example')
    expect(screen.getByText(/Pending nameservers/)).toBeTruthy()
    expect(within(block).getByText('ns1.k2.dev')).toBeTruthy()
    expect(within(block).getByText('ns2.k2.dev')).toBeTruthy()
    expect(within(block).getByText('Auto-added to your k2.dev account')).toBeTruthy()
    expect(within(block).getByRole('button', { name: 'Check again' })).toBeTruthy()

    fireEvent.click(within(block).getByRole('button', { name: 'Copy ns2.k2.dev' }))
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('ns2.k2.dev')
    expect(await within(block).findByText('Copied')).toBeTruthy()
  })

  it('no auto-added note when created is false', async () => {
    listRows = [{ ...PENDING, created: false }]
    render(<DomainsSection />)
    const block = await screen.findByTestId('pending-ns-pending.example')
    expect(within(block).queryByText('Auto-added to your k2.dev account')).toBeNull()
  })

  it('Check again POSTs /cli/domains/refresh and the row flips to active', async () => {
    listRows = [PENDING]
    postHandler = (path, body) => {
      if (path === '/cli/domains/refresh' && body.apex === 'pending.example') {
        listRows = [ACTIVE]
        return { status: 200, body: { ok: true, checked: true, domains: [ACTIVE], domain: ACTIVE } }
      }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    const block = await screen.findByTestId('pending-ns-pending.example')
    fireEvent.click(within(block).getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(screen.queryByTestId('pending-ns-pending.example')).toBeNull())
    expect(screen.getByText(/K2-hosted DNS \(writable\)/)).toBeTruthy()
    expect(calls.some((c) => c.method === 'POST' && c.path === '/cli/domains/refresh')).toBe(true)
  })

  it('active and BYO rows look as before (no pending block)', async () => {
    listRows = [ACTIVE, BYO]
    render(<DomainsSection />)
    expect(await screen.findByText(/K2-hosted DNS \(writable\)/)).toBeTruthy()
    expect(screen.getByText(/BYO DNS \(inventory only\)/)).toBeTruthy()
    expect(screen.queryByText(/Pending nameservers/)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Check again' })).toBeNull()
  })

  it('a 409 attach shows "This domain belongs to another k2.dev account."', async () => {
    postHandler = (path) => {
      if (path === '/cli/domains') {
        return {
          status: 409,
          body: {
            ok: false,
            error: { code: 'zone_owned_elsewhere', hint: 'owned.example is in account 42' },
          },
        }
      }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    await waitFor(() => expect(calls.some((c) => c.path === '/cli/domains')).toBe(true))
    fireEvent.change(screen.getByPlaceholderText('example.com'), {
      target: { value: 'owned.example' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Attach' }))
    expect(await screen.findByText(OWNED_ELSEWHERE_COPY)).toBeTruthy()
    expect(OWNED_ELSEWHERE_COPY).toBe('This domain belongs to another k2.dev account.')
  })

  it('API-missing attach shows the daemon hint', async () => {
    postHandler = () => ({
      status: 503,
      body: {
        ok: false,
        error: {
          code: 'bind_api_unavailable',
          hint: "Domain linking isn't live on k2.dev yet (POST /api/dns/zones/bind answered HTTP 405 with an empty body).",
        },
      },
    })
    render(<DomainsSection />)
    fireEvent.change(screen.getByPlaceholderText('example.com'), {
      target: { value: 'missing.example' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Attach' }))
    expect(await screen.findByText(/Domain linking isn't live on k2.dev yet/)).toBeTruthy()
  })
})

describe('DomainsSection — the daemon re-checks pending domains (prd-dns-pending P1, DN12)', () => {
  it('a domains_changed frame refetches and the pending row flips with no click', async () => {
    listRows = [PENDING]
    render(<DomainsSection />)
    await screen.findByTestId('pending-ns-pending.example')
    expect(domainsChangedHandlers.size).toBe(1)
    const gets = () => calls.filter((c) => c.method === 'GET' && c.path === '/cli/domains').length
    const before = gets()

    // The daemon's own check saw k2.dev flip the zone.
    listRows = [ACTIVE]
    for (const fn of domainsChangedHandlers) fn({ kind: 'domains_changed', reason: 'status_changed', apex: 'pending.example' })
    await waitFor(() => expect(screen.queryByTestId('pending-ns-pending.example')).toBeNull())
    expect(screen.getByText(/K2-hosted DNS \(writable\)/)).toBeTruthy()
    expect(gets()).toBe(before + 1)
    expect(calls.some((c) => c.method === 'POST')).toBe(false)
  })

  it('unsubscribes on unmount', async () => {
    listRows = [ACTIVE]
    const { unmount } = render(<DomainsSection />)
    await screen.findByText(/K2-hosted DNS \(writable\)/)
    expect(domainsChangedHandlers.size).toBe(1)
    unmount()
    expect(domainsChangedHandlers.size).toBe(0)
  })

  it('a zone missing from k2.dev shows the not-found note with Check again', async () => {
    listRows = [{ ...ACTIVE, zoneMissing: true }]
    render(<DomainsSection />)
    const block = await screen.findByTestId('zone-missing-pending.example')
    expect(within(block).getByText(ZONE_MISSING_COPY)).toBeTruthy()
    expect(within(block).getByRole('button', { name: 'Check again' })).toBeTruthy()
  })

  it('a pending row missing from k2.dev shows the note once and one Check again', async () => {
    listRows = [{ ...PENDING, zoneMissing: true }]
    render(<DomainsSection />)
    await screen.findByTestId('zone-missing-pending.example')
    expect(screen.getAllByRole('button', { name: 'Check again' })).toHaveLength(1)
  })

  it('a winddown zone reads as read-only', async () => {
    listRows = [{ ...ACTIVE, status: 'winddown', dnsWrite: false, readOnly: true }]
    render(<DomainsSection />)
    expect(await screen.findByText(/K2-hosted DNS \(read-only: winddown on k2.dev\)/)).toBeTruthy()
    expect(screen.queryByTestId('zone-missing-pending.example')).toBeNull()
  })
})

// 0.46.6 (prd-app-custom-domains-v1 §9.2, vs-live CD-A5): an App name says
// "not served yet" until it is live, shows the TXT to set, and Check now
// re-runs the daemon's DNS check.
describe('DomainsSection — custom App names', () => {
  const WAITING: DomainRow = {
    ...ACTIVE,
    apex: 'agentacme.com',
    names: [
      {
        hostname: 'acme.agentacme.com',
        role: 'app:agentacme',
        cert: { state: 'issued', issuer: "Let's Encrypt" },
        app: {
          app: 'agentacme',
          state: 'waiting_dns',
          live: false,
          notServedYet: true,
          words: 'not served yet: waiting for the DNS records',
          lastError: 'TXT _k2-domain.acme.agentacme.com "k2-verify=ab12" is not visible yet',
          txt: { type: 'TXT', name: '_k2-domain.acme.agentacme.com', value: 'k2-verify=ab12' },
          served: 'elsewhere',
        },
      },
    ],
  }

  it('a bound App name that nothing serves reads "not served yet", never just its cert', async () => {
    listRows = [WAITING]
    render(<DomainsSection />)
    const chip = await screen.findByTestId('app-state-acme.agentacme.com')
    expect(chip.textContent).toBe('not served yet · waiting dns')
    expect(screen.getByText(/TXT _k2-domain\.acme\.agentacme\.com "k2-verify=ab12" \(DNS-only\)/)).toBeTruthy()
  })

  it('Check now POSTs /cli/app/domain/check with the App and host', async () => {
    listRows = [WAITING]
    postHandler = (path, body) => {
      if (path === '/cli/app/domain/check') return { status: 200, body: { ok: true, app: body.app } }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Check now' }))
    await waitFor(() =>
      expect(calls.find((c) => c.path === '/cli/app/domain/check')?.body).toEqual({
        app: 'agentacme',
        host: 'acme.agentacme.com',
      }),
    )
  })

  it('a grandfathered name reads served by your own web server, with no TXT to set', async () => {
    const ext = structuredClone(WAITING)
    ext.names[0].app = {
      ...ext.names[0].app!,
      state: 'external',
      external: true,
      live: false,
      notServedYet: false,
      primary: true,
      lastError: null,
    }
    listRows = [ext]
    render(<DomainsSection />)
    const chip = await screen.findByTestId('app-state-acme.agentacme.com')
    expect(chip.textContent).toBe('served by your own web server (not by K2) · primary')
    expect(screen.queryByText(/\(DNS-only\)/)).toBeNull()
  })

  it('a live primary name reads live · primary', async () => {
    const live = structuredClone(WAITING)
    live.names[0].app = { ...live.names[0].app!, state: 'live', live: true, notServedYet: false, primary: true }
    listRows = [live]
    render(<DomainsSection />)
    const chip = await screen.findByTestId('app-state-acme.agentacme.com')
    expect(chip.textContent).toBe('live · primary')
  })
})

// 0.46.8 (custom domains Path B, PB-10): the badge, the record table, the
// 20-name meter, the upgrade link.
describe('DomainsSection — custom App names through K2 Connect', () => {
  const pathB = (over: Record<string, unknown>) => ({
    available: true,
    why: null,
    meter: '3 of 20 names through K2\'s relays',
    upgrade: null,
    ...over,
  })
  const row = (app: Record<string, unknown>): DomainRow => ({
    ...BYO,
    apex: 'agentacme.com',
    names: [
      {
        hostname: 'acme.agentacme.com',
        role: 'app:agentacme',
        cert: { state: 'missing' },
        app: { app: 'agentacme', state: 'waiting_dns', live: false, notServedYet: true, words: 'not served yet: waiting for the DNS records', ...app },
      },
    ],
  } as DomainRow)

  it('shows the badge, the CNAME and TXT, and the meter when Path B is open', async () => {
    listRows = [row({
      recordsPath: 'connect',
      records: [
        { type: 'CNAME', name: 'acme.agentacme.com', value: 'agentacme.box.k2.dev' },
        { type: 'TXT', name: '_k2-domain.acme.agentacme.com', value: 'k2-verify=t' },
      ],
      pathB: pathB({}),
    })]
    render(<DomainsSection />)
    expect((await screen.findByTestId('app-path-b-acme.agentacme.com')).textContent).toBe('Through K2 Connect (Pro)')
    expect(screen.getByTestId('app-record-CNAME').textContent).toContain('agentacme.box.k2.dev')
    expect(screen.getByTestId('app-record-TXT').textContent).toContain('"k2-verify=t"')
    expect(screen.getByTestId('app-path-b-meter-acme.agentacme.com').textContent).toContain('3 of 20')
  })

  it('says why and links the upgrade when the box is not on Pro; no badge on Path A', async () => {
    listRows = [row({
      recordsPath: 'direct',
      records: [{ type: 'A', name: 'acme.agentacme.com', value: '203.0.113.10' }],
      pathB: pathB({ available: false, why: 'needs K2 Connect Pro', upgrade: 'https://k2.dev/dashboard' }),
    })]
    render(<DomainsSection />)
    const link = await screen.findByTestId('app-path-b-upgrade-acme.agentacme.com')
    expect(link.getAttribute('href')).toBe('https://k2.dev/dashboard')
    expect(screen.queryByTestId('app-path-b-acme.agentacme.com')).toBeNull()
    expect(screen.getByTestId('app-record-A').textContent).toContain('203.0.113.10')
  })

  it('before a claim succeeds, the CNAME shows alone and says why there is no TXT', async () => {
    listRows = [row({
      recordsPath: 'connect',
      records: [{ type: 'CNAME', name: 'acme.agentacme.com', value: 'agentacme.box.k2.dev' }],
      txtPending: 'No TXT record to set yet: K2 Connect gives the value once it accepts the claim.',
      pathB: pathB({}),
    })]
    render(<DomainsSection />)
    expect((await screen.findByTestId('app-txt-pending-acme.agentacme.com')).textContent).toContain('No TXT record to set yet')
    expect(screen.queryByTestId('app-record-TXT')).toBeNull()
  })

  it('a live name K2 Connect routes keeps its badge', async () => {
    listRows = [row({ state: 'live', live: true, notServedYet: false, viaRelay: true, recordsPath: 'connect' })]
    render(<DomainsSection />)
    await screen.findByTestId('app-path-b-acme.agentacme.com')
    expect(screen.queryByTestId('app-record-CNAME')).toBeNull()
  })
})

describe('Firewall panel (0.46.6)', () => {
  it('the :80 switch shows with the front and POSTs /cli/front/http (0.46.8)', async () => {
    firewallGet = {
      status: 200,
      body: {
        ok: true,
        view: { hash: 'h', public: [{ port: 80, proto: 'tcp', source: 'front80', allowFrom: ['any'], why: 'the :80 front' }], knownNotPublic: [], box: { label: 'box', behindNat: false }, upstream: { managedBy: 'owner', applied: null } },
        ssh: { port: 22, allowFrom: [] },
        applies: 'x',
        guard: { settings: { enabled: true, connsPerIp: 64, writesPerMin: 120 }, counters: { perIpRefused: 0, guardLimited: 0 } },
        front: { enabled: true, http: { on: true, serving: true } },
      },
    }
    postHandler = () => ({ status: 200, body: { ok: true } })
    render(<DomainsSection />)
    const row = await screen.findByTestId('fw-http')
    expect(row.textContent).toContain('on')
    expect(within(screen.getByTestId('fw-row-80-tcp')).getByText('K2: the :80 front')).toBeTruthy()
    fireEvent.click(within(row).getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(calls.find((c) => c.method === 'POST' && c.path === '/cli/front/http')?.body).toEqual({ on: false }))
  })

  const STATUS = {
    ok: true,
    view: {
      hash: 'sha256:' + 'a'.repeat(64),
      public: [
        { port: 22, proto: 'tcp', source: 'ssh', allowFrom: ['any'], why: 'SSH (owner setting)' },
        { port: 443, proto: 'tcp', source: 'front443', allowFrom: ['any'], why: 'the :443 front' },
        { port: 6000, proto: 'udp', source: 'owner', allowFrom: ['any'], why: 'owner: game' },
      ],
      knownNotPublic: [],
      box: { label: 'box', behindNat: false },
      upstream: { managedBy: 'owner', applied: null },
    },
    ssh: { port: 22, allowFrom: [] },
    applies: 'Nobody applies this list for you: copy the rules (k2 firewall rules --format nft|ufw).',
    guard: { settings: { enabled: true, connsPerIp: 64, writesPerMin: 120 }, counters: { perIpRefused: 0, guardLimited: 0 } },
  }

  it('lists the public ports, K2 rows read-only, owner rows removable', async () => {
    firewallGet = { status: 200, body: STATUS }
    postHandler = () => ({ status: 200, body: { ok: true } })
    render(<DomainsSection />)
    const owner = await screen.findByTestId('fw-row-6000-udp')
    expect(within(owner).getByRole('button', { name: 'Remove' })).toBeTruthy()
    const front = screen.getByTestId('fw-row-443-tcp')
    expect(within(front).getByText('K2: the :443 front')).toBeTruthy()
    expect(within(front).getByText('read-only')).toBeTruthy()
    expect(within(screen.getByTestId('fw-row-22-tcp')).queryByRole('button', { name: 'Remove' })).toBeNull()
    expect(screen.getByText(/Nobody applies this list for you/)).toBeTruthy()
    expect(screen.getByTestId('fw-guard').textContent).toContain('64 connections')
    fireEvent.click(within(owner).getByRole('button', { name: 'Remove' }))
    await waitFor(() =>
      expect(calls.find((c) => c.method === 'POST' && c.path === '/cli/firewall/close')?.body).toEqual({ port: '6000/udp' }),
    )
  })
})
