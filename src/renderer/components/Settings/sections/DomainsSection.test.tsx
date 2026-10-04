// @vitest-environment jsdom

// Settings → K2 Server → Domains: custom-domain attach A8.1 states
// (pending nameservers, auto-added, Check again, 409 owned elsewhere).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ token: 'tok', port: 1 })),
  daemonHttpBase: () => 'http://daemon.test',
}))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'local' }))

import { DomainsSection, OWNED_ELSEWHERE_COPY, type DomainRow } from './DomainsSection'

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
