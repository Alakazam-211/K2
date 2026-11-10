// @vitest-environment jsdom

// Settings → Network → Custom Domains: reads that fail or answer the wrong
// shape, adding a domain, its status and the nameservers to set (A8.1),
// names and their records (App names, Path B), and the firewall.

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

import { DomainsSection, OWNED_ELSEWHERE_COPY, ZONE_MISSING_COPY } from './DomainsSection'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

type Row = Record<string, unknown>
const PENDING: Row = {
  apex: 'pending.example',
  zoneId: 'z-pend',
  dnsWrite: false,
  status: 'pending_ns',
  nameservers: ['ns1.k2.dev', 'ns2.k2.dev'],
  created: true,
  names: [],
}
const ACTIVE: Row = { ...PENDING, status: 'active', dnsWrite: true }
const BYO: Row = { apex: 'byo.example', zoneId: null, dnsWrite: false, status: null, nameservers: [], created: false, names: [] }

type Call = { method: string; path: string; body: unknown }
let calls: Call[] = []
let listGet: { status: number; body: unknown } = { status: 200, body: { ok: true, domains: [] } }
let postHandler: (path: string, body: Record<string, unknown>) => { status: number; body: unknown }
let firewallGet: { status: number; body: unknown }
const confirm = vi.fn(async () => true)

const rows = (r: Row[]): void => {
  listGet = { status: 200, body: { ok: true, domains: r } }
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  cleanup()
  calls = []
  rows([])
  domainsChangedHandlers.clear()
  // A firewall with no open ports, so its own card never adds an alert.
  firewallGet = { status: 200, body: { ok: true, view: { public: [] } } }
  postHandler = () => ({ status: 404, body: { ok: false, error: { code: 'not_found', hint: 'nope' } } })
  confirm.mockClear()
  confirm.mockImplementation(async () => true)
  useConfirmDialogStore.setState({ confirm })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const path = new URL(url).pathname
      const method = init?.method ?? 'GET'
      const body = init?.body ? JSON.parse(String(init.body)) : undefined
      calls.push({ method, path, body })
      if (method === 'GET' && path === '/cli/domains') return jsonResponse(listGet.status, listGet.body)
      if (method === 'GET' && path === '/cli/firewall') return jsonResponse(firewallGet.status, firewallGet.body)
      const r = postHandler(path, (body ?? {}) as Record<string, unknown>)
      return jsonResponse(r.status, r.body)
    }),
  )
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: vi.fn(async () => undefined) }, configurable: true })
})

const posts = (path: string): Call[] => calls.filter((c) => c.method === 'POST' && c.path === path)

describe('DomainsSection — reads that fail', () => {
  it('an answer of the wrong shape is a failed read in plain words, never a crash', async () => {
    listGet = { status: 200, body: {} }
    firewallGet = { status: 200, body: {} }
    render(<DomainsSection />)
    const card = await screen.findByTestId('domains-load-error')
    expect(within(card).getByText('K2 couldn’t read your domains from this server')).toBeTruthy()
    expect(card.textContent).toContain('wasn’t a list of domains K2 can read')
    expect(card.textContent).not.toContain('The server said')
    const fw = await screen.findByTestId('firewall-load-error')
    expect(within(fw).getByText('K2 couldn’t read the firewall from this server')).toBeTruthy()
  })

  it('a failed read says what failed, then the server’s reason, and Try again reads again', async () => {
    listGet = { status: 500, body: { ok: false, error: { code: 'db', hint: 'database is locked' } } }
    render(<DomainsSection />)
    const card = await screen.findByTestId('domains-load-error')
    expect(card.textContent).toContain('The server said: database is locked.')
    rows([ACTIVE])
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByTestId('domain-pending.example')).toBeTruthy()
    expect(screen.queryByTestId('domains-load-error')).toBeNull()
  })

  it('no domains yet: says so and offers Add domain', async () => {
    render(<DomainsSection />)
    const empty = await screen.findByTestId('domains-empty')
    expect(within(empty).getByText('No domains yet')).toBeTruthy()
    fireEvent.click(within(empty).getByRole('button', { name: 'Add domain' }))
    expect(await screen.findByTestId('add-domain-form')).toBeTruthy()
  })
})

describe('DomainsSection — adding a domain', () => {
  it('adds the domain, then shows it with its nameservers and an Added note', async () => {
    const added = { ...PENDING, apex: 'new.example' }
    postHandler = (path, body) => {
      if (path === '/cli/domains' && body.apex === 'new.example') {
        rows([added])
        return { status: 200, body: { ok: true, domain: added } }
      }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    fireEvent.click(within(await screen.findByTestId('domains-empty')).getByRole('button', { name: 'Add domain' }))
    const form = await screen.findByTestId('add-domain-form')
    const add = within(form).getByRole('button', { name: 'Add domain' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    fireEvent.change(within(form).getByLabelText('Domain name'), { target: { value: 'New.Example' } })
    expect(add.disabled).toBe(false)
    fireEvent.click(add)
    const row = await screen.findByTestId('domain-new.example')
    expect(within(row).getByText('Added')).toBeTruthy()
    expect(within(row).getByText('Waiting for nameservers')).toBeTruthy()
    expect(within(row).getByText('ns1.k2.dev')).toBeTruthy()
    expect(screen.queryByTestId('add-domain-form')).toBeNull()
  })

  it('a 409 says the domain belongs to another k2.dev account and keeps the form', async () => {
    rows([BYO])
    postHandler = (path) => {
      if (path === '/cli/domains') return { status: 409, body: { ok: false, error: { code: 'zone_owned_elsewhere', hint: 'owned.example is in account 42' } } }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Add domain' }))
    const form = await screen.findByTestId('add-domain-form')
    fireEvent.change(within(form).getByLabelText('Domain name'), { target: { value: 'owned.example' } })
    fireEvent.click(within(form).getByRole('button', { name: 'Add domain' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('K2 couldn’t add owned.example.')
    expect(alert.textContent).toContain(OWNED_ELSEWHERE_COPY)
    expect(screen.getByTestId('add-domain-form')).toBeTruthy()
  })

  it('any other refusal shows the server’s reason', async () => {
    rows([BYO])
    postHandler = () => ({
      status: 503,
      body: { ok: false, error: { code: 'bind_api_unavailable', hint: "Domain linking isn't live on k2.dev yet" } },
    })
    render(<DomainsSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Add domain' }))
    fireEvent.change(screen.getByLabelText('Domain name'), { target: { value: 'missing.example' } })
    fireEvent.click(within(screen.getByTestId('add-domain-form')).getByRole('button', { name: 'Add domain' }))
    expect((await screen.findByRole('alert')).textContent).toContain("The server said: Domain linking isn't live on k2.dev yet.")
  })
})

describe('DomainsSection — a domain’s status (A8.1, DN12)', () => {
  it('a pending domain lists the nameservers to set, with copy buttons and the auto-added note', async () => {
    rows([PENDING])
    render(<DomainsSection />)
    const block = await screen.findByTestId('pending-ns-pending.example')
    expect(within(block).getByText('ns1.k2.dev')).toBeTruthy()
    expect(within(block).getByText('K2 also added pending.example to your k2.dev account.')).toBeTruthy()
    fireEvent.click(within(block).getByRole('button', { name: 'Copy ns2.k2.dev' }))
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('ns2.k2.dev')
    expect(await within(block).findByText('Copied')).toBeTruthy()
  })

  it('no auto-added note when created is false', async () => {
    rows([{ ...PENDING, created: false }])
    render(<DomainsSection />)
    const block = await screen.findByTestId('pending-ns-pending.example')
    expect(within(block).queryByText(/also added/)).toBeNull()
  })

  it('Check again POSTs /cli/domains/refresh and the row flips to Working', async () => {
    rows([PENDING])
    postHandler = (path, body) => {
      if (path === '/cli/domains/refresh' && body.apex === 'pending.example') {
        rows([ACTIVE])
        return { status: 200, body: { ok: true, checked: true } }
      }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    const row = await screen.findByTestId('domain-pending.example')
    fireEvent.click(within(row).getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(screen.queryByTestId('pending-ns-pending.example')).toBeNull())
    expect(screen.getByTestId('domain-status-pending.example').textContent).toBe('Working')
  })

  it('a check k2.dev didn’t answer says so', async () => {
    rows([PENDING])
    postHandler = () => ({ status: 200, body: { ok: true, checked: false, hint: 'k2.dev unreachable' } })
    render(<DomainsSection />)
    fireEvent.click(within(await screen.findByTestId('domain-pending.example')).getByRole('button', { name: 'Check again' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('K2 couldn’t check pending.example with k2.dev.')
    expect(alert.textContent).toContain('k2.dev unreachable')
  })

  it('your own DNS and K2’s DNS read differently', async () => {
    rows([ACTIVE, BYO])
    render(<DomainsSection />)
    expect((await screen.findByTestId('domain-status-pending.example')).textContent).toBe('Working')
    expect(screen.getByTestId('domain-status-byo.example').textContent).toBe('Your DNS')
    expect(screen.queryByRole('button', { name: 'Check again' })).toBeNull()
  })

  it('a zone missing from k2.dev shows the note and one Check again', async () => {
    rows([{ ...PENDING, zoneMissing: true }])
    render(<DomainsSection />)
    expect((await screen.findByTestId('zone-missing-pending.example')).textContent).toBe(ZONE_MISSING_COPY)
    expect(screen.getByTestId('domain-status-pending.example').textContent).toBe('Not found')
    expect(screen.getAllByRole('button', { name: 'Check again' })).toHaveLength(1)
  })

  it('a winddown zone is on hold and names the plan state', async () => {
    rows([{ ...ACTIVE, status: 'winddown', dnsWrite: false, readOnly: true }])
    render(<DomainsSection />)
    expect((await screen.findByTestId('domain-status-pending.example')).textContent).toBe('On hold')
    expect(screen.getByText(/your k2\.dev plan is winddown/)).toBeTruthy()
  })

  it('Remove asks first, then POSTs /cli/domains/remove', async () => {
    rows([BYO])
    postHandler = (path) => {
      if (path === '/cli/domains/remove') {
        rows([])
        return { status: 200, body: { ok: true } }
      }
      throw new Error(`unexpected POST ${path}`)
    }
    confirm.mockImplementationOnce(async () => false)
    render(<DomainsSection />)
    const menu = await screen.findByRole('button', { name: 'More actions for byo.example' })
    fireEvent.click(menu)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove…' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(posts('/cli/domains/remove')).toHaveLength(0)
    fireEvent.click(screen.getByRole('button', { name: 'More actions for byo.example' }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove…' }))
    await waitFor(() => expect(posts('/cli/domains/remove')[0]?.body).toEqual({ apex: 'byo.example' }))
    expect(await screen.findByTestId('domains-empty')).toBeTruthy()
  })

  it('a domains_changed frame reads again with no click, and unmount unsubscribes', async () => {
    rows([PENDING])
    const { unmount } = render(<DomainsSection />)
    await screen.findByTestId('pending-ns-pending.example')
    expect(domainsChangedHandlers.size).toBe(1)
    rows([ACTIVE])
    for (const fn of domainsChangedHandlers) fn({ kind: 'domains_changed', reason: 'status_changed', apex: 'pending.example' })
    await waitFor(() => expect(screen.queryByTestId('pending-ns-pending.example')).toBeNull())
    expect(calls.some((c) => c.method === 'POST')).toBe(false)
    unmount()
    expect(domainsChangedHandlers.size).toBe(0)
  })
})

describe('DomainsSection — names', () => {
  const withNames = (names: Row[]): Row[] => [{ ...ACTIVE, apex: 'example.com', names }]

  it('a name says what it’s for and whether its certificate works', async () => {
    rows(withNames([
      { hostname: 'mail.example.com', role: 'mail', cert: { state: 'issued', expiresAt: 1798502400 } },
      { hostname: 'www.example.com', role: 'publish', cert: { state: 'expired', expiresAt: 1790035200 }, renewal: { state: 'retrying', lastError: 'the DNS points somewhere else' } },
      { hostname: 'new.example.com', role: 'other' },
    ]))
    render(<DomainsSection />)
    const mail = await screen.findByTestId('domain-name-mail.example.com')
    expect(within(mail).getByText('Working')).toBeTruthy()
    expect(mail.textContent).toContain('For mail. Certificate until')
    const www = screen.getByTestId('domain-name-www.example.com')
    expect(within(www).getByText('Certificate expired')).toBeTruthy()
    expect(www.textContent).toContain('The server said: the DNS points somewhere else.')
    expect(within(screen.getByTestId('domain-name-new.example.com')).getByText('No certificate')).toBeTruthy()
  })

  it('Renew on an expired name POSTs /cli/certs/renew and says it’s done', async () => {
    rows(withNames([{ hostname: 'www.example.com', role: 'publish', cert: { state: 'expired' } }]))
    postHandler = (path) => {
      if (path === '/cli/certs/renew') return { status: 200, body: { ok: true } }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    fireEvent.click(within(await screen.findByTestId('domain-name-www.example.com')).getByRole('button', { name: 'Renew' }))
    expect(await screen.findByText('Renewed')).toBeTruthy()
    expect(posts('/cli/certs/renew')[0].body).toEqual({ hostname: 'www.example.com' })
  })

  it('Add name checks the name is on one of your domains, then POSTs it with its use', async () => {
    rows(withNames([]))
    postHandler = (path) => {
      if (path === '/cli/domains/names') return { status: 200, body: { ok: true } }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    expect((await screen.findByTestId('names-empty')).textContent).toContain('No names yet')
    fireEvent.click(screen.getByRole('button', { name: 'Add name' }))
    const form = await screen.findByTestId('add-name-form')
    const add = within(form).getByRole('button', { name: 'Add name' }) as HTMLButtonElement
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'mail.other.example' } })
    expect(add.disabled).toBe(true)
    expect(form.textContent).toContain('Use a name on one of your domains: example.com.')
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'mail.example.com' } })
    fireEvent.click(add)
    await waitFor(() => expect(posts('/cli/domains/names')[0]?.body).toEqual({ hostname: 'mail.example.com', role: 'mail' }))
  })

  // 0.46.6 (prd-app-custom-domains-v1 §9.2): an App name is never "Working"
  // until it is live, shows the records to set, and Check now re-runs the check.
  const APP = (app: Row): Row[] =>
    withNames([{ hostname: 'shop.example.com', role: 'app:shop', cert: { state: 'issued' }, app: { app: 'shop', state: 'waiting_dns', live: false, notServedYet: true, ...app } }])

  it('an App name nothing serves yet waits for DNS and lists the records, with Check now', async () => {
    rows(APP({
      lastError: 'TXT _k2.shop.example.com is not visible yet',
      txt: { type: 'TXT', name: '_k2.shop.example.com', value: 'k2-verify=ab12' },
    }))
    postHandler = (path) => {
      if (path === '/cli/app/domain/check') return { status: 200, body: { ok: true } }
      throw new Error(`unexpected POST ${path}`)
    }
    render(<DomainsSection />)
    const row = await screen.findByTestId('domain-name-shop.example.com')
    expect(screen.getByTestId('name-status-shop.example.com').textContent).toBe('Waiting for DNS')
    expect(screen.getByTestId('app-record-TXT').textContent).toContain('k2-verify=ab12')
    expect(row.textContent).toContain('Last check: TXT _k2.shop.example.com is not visible yet.')
    fireEvent.click(within(row).getByRole('button', { name: 'Check now' }))
    await waitFor(() => expect(posts('/cli/app/domain/check')[0]?.body).toEqual({ app: 'shop', host: 'shop.example.com' }))
  })

  it('your own web server, a live main address, and a missing App each read plainly', async () => {
    rows(APP({ state: 'external', external: true, notServedYet: false }))
    const { unmount } = render(<DomainsSection />)
    expect((await screen.findByTestId('name-status-shop.example.com')).textContent).toBe('Your web server')
    expect(screen.queryByTestId('app-records-shop.example.com')).toBeNull()
    unmount()
    rows(APP({ state: 'live', live: true, notServedYet: false, primary: true }))
    const second = render(<DomainsSection />)
    expect((await screen.findByTestId('name-status-shop.example.com')).textContent).toBe('Live')
    expect(screen.getByTestId('domain-name-shop.example.com').textContent).toContain('Its main address.')
    second.unmount()
    rows(APP({ error: 'unknown_app' }))
    render(<DomainsSection />)
    expect((await screen.findByTestId('name-status-shop.example.com')).textContent).toBe('No such App')
  })

  // 0.46.8 (Path B, PB-10): the records for K2 Connect, the meter, the upgrade link.
  it('through K2 Connect: the CNAME and TXT, the meter, or why not and the upgrade link', async () => {
    rows(APP({
      recordsPath: 'connect',
      records: [
        { type: 'CNAME', name: 'shop.example.com', value: 'shop.box.k2.dev' },
        { type: 'TXT', name: '_k2-domain.shop.example.com', value: 'k2-verify=t' },
      ],
      pathB: { available: true, meter: '3 of 20 names through K2 Connect' },
    }))
    const { unmount } = render(<DomainsSection />)
    expect((await screen.findByTestId('app-record-CNAME')).textContent).toContain('shop.box.k2.dev')
    expect(screen.getByTestId('app-path-b-meter-shop.example.com').textContent).toContain('3 of 20')
    unmount()
    rows(APP({
      recordsPath: 'connect',
      records: [{ type: 'CNAME', name: 'shop.example.com', value: 'shop.box.k2.dev' }],
      txtPending: 'No TXT record to set yet: K2 Connect gives the value once it accepts the claim.',
      pathB: { available: false, why: 'needs K2 Connect Pro', upgrade: 'https://k2.dev/dashboard' },
    }))
    render(<DomainsSection />)
    expect((await screen.findByTestId('app-txt-pending-shop.example.com')).textContent).toContain('No TXT record to set yet')
    expect(screen.queryByTestId('app-record-TXT')).toBeNull()
    expect(screen.getByTestId('app-path-b-upgrade-shop.example.com').getAttribute('href')).toBe('https://k2.dev/dashboard')
  })
})

describe('Firewall (0.46.6)', () => {
  const STATUS = {
    ok: true,
    view: {
      public: [
        { port: 22, proto: 'tcp', source: 'ssh', allowFrom: ['any'], why: 'SSH (owner setting)' },
        { port: 443, proto: 'tcp', source: 'front443', allowFrom: ['any'], why: 'the :443 front' },
        { port: 6000, proto: 'udp', source: 'owner', allowFrom: ['any'], why: 'owner: game' },
      ],
      box: { behindNat: false },
      upstream: { managedBy: 'owner', applied: null },
    },
    ssh: { port: 22, allowFrom: [] },
    guard: { settings: { enabled: true, connsPerIp: 64, writesPerMin: 120 }, counters: { perIpRefused: 0, guardLimited: 0 } },
    front: { enabled: true, http: { on: true, serving: true } },
  }

  it('lists the open ports in plain words; K2’s are set by K2, yours close after a confirm', async () => {
    firewallGet = { status: 200, body: STATUS }
    postHandler = () => ({ status: 200, body: { ok: true } })
    render(<DomainsSection />)
    const owner = await screen.findByTestId('fw-row-6000-udp')
    expect(owner.textContent).toContain('Game')
    const front = screen.getByTestId('fw-row-443-tcp')
    expect(within(front).getByText('Set by K2')).toBeTruthy()
    expect(front.textContent).toContain('Secure web')
    expect(within(screen.getByTestId('fw-row-22-tcp')).queryByRole('button')).toBeNull()
    expect(screen.getByText(/Nothing sets this list at your provider/)).toBeTruthy()
    expect(screen.getByTestId('fw-guard').textContent).toContain('64 connections')
    fireEvent.click(within(owner).getByRole('button', { name: 'Close…' }))
    await waitFor(() => expect(posts('/cli/firewall/close')[0]?.body).toEqual({ port: '6000/udp' }))
    expect(confirm).toHaveBeenCalledTimes(1)
  })

  it('the http redirect switch POSTs /cli/front/http (0.46.8)', async () => {
    firewallGet = { status: 200, body: STATUS }
    postHandler = () => ({ status: 200, body: { ok: true } })
    render(<DomainsSection />)
    const sw = await screen.findByRole('switch', { name: 'Send http to https' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    await waitFor(() => expect(posts('/cli/front/http')[0]?.body).toEqual({ on: false }))
  })

  it('opening a port checks the number and POSTs it with what it’s for', async () => {
    firewallGet = { status: 200, body: STATUS }
    postHandler = () => ({ status: 200, body: { ok: true } })
    render(<DomainsSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Open a port' }))
    const open = screen.getByRole('button', { name: 'Open port' }) as HTMLButtonElement
    fireEvent.change(screen.getByLabelText('Port'), { target: { value: '70000' } })
    fireEvent.change(screen.getByLabelText('What it’s for'), { target: { value: 'Game server' } })
    expect(open.disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('Port'), { target: { value: '6001/udp' } })
    fireEvent.click(open)
    await waitFor(() => expect(posts('/cli/firewall/open')[0]?.body).toEqual({ port: '6001/udp', label: 'Game server' }))
  })
})
