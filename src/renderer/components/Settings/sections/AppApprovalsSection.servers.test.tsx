// @vitest-environment jsdom

// 0.46.5 patch-up: App Approvals only showed the window's server. It now
// lists the held requests of every server the person owns (the Home pool's
// servers signed in with role owner), one block per server. Each request is
// confirmed, approved and denied on ITS server (that server's vault confirm
// and login), never relayed through the window's. One server failing to
// answer never hides another's requests.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

type Scope = { id: string; isPrimary: boolean; hostKey: string; label: string }

const h = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
}))

const PRIMARY = vi.hoisted(() => ({ id: 'primary', isPrimary: true, hostKey: 'local', label: 'This computer' }))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => PRIMARY,
  scopeForHost: (hostKey: string) => ({ id: `host:${hostKey}`, isPrimary: false, hostKey, label: `Server ${hostKey}` }),
}))

// Every request names the server it went to.
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (scope: Scope, route: string, params?: unknown) => h.get(scope.hostKey, route, params),
  daemonCliPost: (scope: Scope, route: string, body?: unknown) => h.post(scope.hostKey, route, body),
}))

const pool = vi.hoisted(() => ({
  entries: {} as Record<string, unknown>,
}))
vi.mock('@/lib/host-pool-instance', () => ({
  hostPool: {
    store: { getState: () => ({ entries: pool.entries }), subscribe: () => () => {} },
  },
}))

import type { AppRequest } from './app-approvals-api'
import { AppApprovalsSection } from './AppApprovalsSection'

const now = (): number => Math.floor(Date.now() / 1000)

function req(id: string, appLabel: string): AppRequest {
  return {
    id,
    kind: 'egress',
    appId: `app-${appLabel}`,
    serviceId: `svc-${appLabel}`,
    appLabel,
    workspaceId: 'ws',
    workspaceHandle: 'ops',
    body: {},
    diff: ['+ host api.example.test'],
    summary: `${appLabel} wants api.example.test`,
    reasons: [],
    reason: null,
    requestedBy: 'agent:ops',
    requesterWorkspace: 'ws',
    requestedAt: now() - 60,
    expiresAt: now() + 86400,
    status: 'pending',
    decidedBy: null,
    decidedAt: null,
    confirmVia: null,
    decisionReason: null,
    result: null,
  }
}

const OK = { available: true, methods: ['connect-password'] }

function owner(hostKey: string, over: Record<string, unknown> = {}) {
  return { hostKey, saved: true, auth: 'ok', role: 'owner', ...over }
}

beforeEach(() => {
  h.get.mockReset()
  h.post.mockReset()
  pool.entries = {}
})
afterEach(() => cleanup())

describe('App Approvals across servers', () => {
  it('shows each owned server in its own block; Admin and signed-out servers are never read', async () => {
    pool.entries = {
      'box.k2.dev': owner('box.k2.dev'),
      'admin.k2.dev': owner('admin.k2.dev', { role: 'admin' }),
      'out.k2.dev': owner('out.k2.dev', { auth: 'signin-required' }),
    }
    const lists: Record<string, AppRequest[]> = { local: [req('w_here', 'notes')], 'box.k2.dev': [req('w_box', 'shop')] }
    h.get.mockImplementation(async (host: string, route: string) => {
      if (route !== 'app/approvals' || !(host in lists)) throw new Error(`unexpected GET ${host} ${route}`)
      return { ok: true, requests: lists[host], ownerConfirm: OK }
    })
    render(<AppApprovalsSection />)
    const box = await screen.findByTestId('app-approvals-server-box.k2.dev')
    expect(box.textContent).toContain('Server box.k2.dev')
    expect(box.textContent).toContain('shop wants api.example.test')
    const here = screen.getByTestId('app-approvals-server-local')
    expect(here.textContent).toContain('This computer')
    expect(here.textContent).toContain("this window's server")
    expect(here.textContent).toContain('notes wants api.example.test')
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('App Approvals (2)')
    const read = new Set(h.get.mock.calls.map((c) => c[0]))
    expect(read).toEqual(new Set(['local', 'box.k2.dev']))
  })

  it("approves with THAT server's confirm and denies on that server", async () => {
    pool.entries = { 'box.k2.dev': owner('box.k2.dev') }
    let boxDone = false
    h.get.mockImplementation(async (host: string) => ({
      ok: true,
      requests: host === 'local' ? [req('w_here', 'notes')] : boxDone ? [] : [req('w_box', 'shop')],
      ownerConfirm: OK,
    }))
    h.post.mockImplementation(async (host: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['connect-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: `tok-${host}`, expiresIn: 60 }
      if (route === 'app/approve') {
        boxDone = true
        return { ok: true, status: 'approved', request: req('w_box', 'shop') }
      }
      if (route === 'app/deny') return { ok: true, status: 'denied' }
      throw new Error(`unexpected POST ${host} ${route}`)
    })
    render(<AppApprovalsSection />)
    fireEvent.click(await screen.findByTestId('app-approve-w_box'))
    const pw = await screen.findByTestId('app-approve-password')
    ;(pw as HTMLInputElement).value = 'hunter2'
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.submit(pw.closest('form')!)
    await waitFor(() => expect(h.post.mock.calls.some((c) => c[1] === 'app/approve')).toBe(true))
    const approveCalls = h.post.mock.calls.filter((c) => c[1] !== 'app/deny')
    expect(approveCalls.map((c) => [c[0], c[1]])).toEqual([
      ['box.k2.dev', 'vault/confirm/start'],
      ['box.k2.dev', 'vault/confirm/finish'],
      ['box.k2.dev', 'app/approve'],
    ])
    expect(approveCalls[2][2]).toEqual({ request: 'w_box', confirmToken: 'tok-box.k2.dev' })

    fireEvent.click(screen.getByTestId('app-deny-w_here'))
    fireEvent.click(screen.getByTestId('app-deny-send-w_here'))
    await waitFor(() => expect(h.post.mock.calls.some((c) => c[1] === 'app/deny')).toBe(true))
    const deny = h.post.mock.calls.find((c) => c[1] === 'app/deny')!
    expect([deny[0], deny[2]]).toEqual(['local', { request: 'w_here' }])
  })

  it('a server that does not answer is named; the others still show', async () => {
    pool.entries = { 'down.k2.dev': owner('down.k2.dev') }
    h.get.mockImplementation(async (host: string) => {
      if (host === 'down.k2.dev') throw new Error('server unreachable')
      return { ok: true, requests: [req('w_here', 'notes')], ownerConfirm: OK }
    })
    render(<AppApprovalsSection />)
    const err = await screen.findByTestId('app-approvals-error-down.k2.dev')
    expect(err.textContent).toContain('Server down.k2.dev')
    expect(err.textContent).toContain('server unreachable')
    expect(screen.getByTestId('app-request-w_here')).toBeTruthy()
  })

  it('with only the window server it reads one server and shows no server headings', async () => {
    h.get.mockImplementation(async () => ({ ok: true, requests: [req('w_here', 'notes')], ownerConfirm: OK }))
    render(<AppApprovalsSection />)
    const here = await screen.findByTestId('app-approvals-server-local')
    expect(here.querySelector('h3')).toBeNull()
    expect(new Set(h.get.mock.calls.map((c) => c[0]))).toEqual(new Set(['local']))
  })
})
