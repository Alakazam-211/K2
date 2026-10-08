// @vitest-environment jsdom
// Settings → K2 Server → Vault (prd-k2-vault-v1 §16.1, KV39).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within, act } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) =>
    sel({ projects: [{ id: 'p-sales', name: 'sales', handle: 'sales', path: '/w/sales' }] }),
}))

import { VaultSection, groupSecrets, REVEAL_SHOW_MS } from './VaultSection'
import { toBase64, type VaultSecret, type VaultStatus } from './vault-api'

const VALUE = 'k2test_REVEALED_789'

function secret(over: Partial<VaultSecret>): VaultSecret {
  return {
    id: 'id-' + (over.name ?? 'X'),
    namespace: 'user',
    scope: { kind: 'workspace', id: 'p-sales' },
    workspace: 'sales',
    name: 'STRIPE_KEY',
    kind: 'api_key',
    hosts: ['https://api.stripe.com:443'],
    hostsDisplay: [{ origin: 'https://api.stripe.com:443', unicode: 'api.stripe.com', mixedScript: false }],
    defaultDelivery: 'fetch',
    deliveryCeiling: 'env',
    origin: 'thread:c1',
    createdBy: 'owner',
    humanGiven: true,
    latestVersion: 2,
    status: 'active',
    ref: 'k2v://ws/sales/STRIPE_KEY',
    agentCanSee: false,
    ...over,
  }
}

const STATUS: VaultStatus = {
  open: true,
  keyHome: 'file',
  keyHomeLabel: 'A file on this server',
  rollbackGuard: 'on',
  rollback: false,
  tamper: 'ok',
  recovery: 'unconfirmed',
  confirmPassword: true,
  honest: 'Secrets are encrypted and never shown to guests, apps or other servers.',
  recoveryHint:
    "Only you should do this, from your own SSH login or a terminal outside K2: k2 vault recovery show-once. Agents: don't run it; tell your owner.",
  adopted: [],
}

let userRows: VaultSecret[] = []
let systemCall: () => Promise<unknown> = async () => ({ secrets: [] })
let posts: { route: string; body: Record<string, unknown> }[] = []
let postHandler: (route: string, body: Record<string, unknown>) => unknown = () => ({ ok: true })

beforeEach(() => {
  cleanup()
  posts = []
  userRows = [
    secret({ name: 'STRIPE_KEY' }),
    secret({
      name: 'SERVER_TOKEN',
      scope: { kind: 'server' },
      workspace: undefined,
      defaultDelivery: 'env',
      agentCanSee: true,
      ref: 'k2v://server/SERVER_TOKEN',
    }),
    secret({
      name: 'CHAT_KEY',
      scope: { kind: 'session', id: 'conv-1' },
      workspace: undefined,
      hosts: ['https://xn--pple-43d.com:443'],
      hostsDisplay: [{ origin: 'https://xn--pple-43d.com:443', unicode: 'аpple.com', mixedScript: true }],
    }),
  ]
  systemCall = async () => ({ secrets: [] })
  postHandler = () => ({ ok: true })
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    if (route === 'vault/status') return STATUS
    if (route === 'vault/list') return params?.system ? systemCall() : { secrets: userRows }
    if (route === 'vault/presets')
      return { presets: [{ id: 'stripe', label: 'Stripe', hosts: ['https://api.stripe.com:443'], dst: 'header:Authorization', fmt: 'Bearer {}' }] }
    if (route === 'vault/requests') return { requests: [] }
    if (route === 'vault/show') return { secret: userRows[0], grants: [], audit: [] }
    throw new Error(`unexpected GET ${route}`)
  })
  daemonCliPost.mockImplementation(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    posts.push({ route, body: { ...body } })
    return postHandler(route, body)
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function renderPage() {
  const r = render(<VaultSection />)
  await screen.findAllByTestId('vault-secret-row')
  return r
}

function row(name: string): HTMLElement {
  const el = screen.getAllByTestId('vault-secret-row').find((r) => r.getAttribute('data-secret') === name)
  if (!el) throw new Error(`no row ${name}`)
  return el
}

describe('VaultSection', () => {
  it('status card: key home, the honest line, and the person-only recovery banner', async () => {
    await renderPage()
    expect(screen.getByTestId('vault-key-home').textContent).toBe('A file on this server')
    expect(screen.getByTestId('vault-honest').textContent).toBe(STATUS.honest)
    const banner = screen.getByTestId('vault-recovery-banner')
    expect(banner.textContent).toContain('k2 vault recovery show-once')
    expect(banner.textContent).toContain('coming later')
    expect(document.body.textContent).not.toMatch(/agents can.t read/i)
  })

  it('groups Server / each workspace / chats; badges readable delivery; warns on mixed scripts', async () => {
    await renderPage()
    const groups = screen.getAllByTestId('vault-group').map((g) => g.getAttribute('data-group'))
    expect(groups).toEqual(['server', 'ws:sales', 'session'])
    expect(within(row('SERVER_TOKEN')).getByTestId('vault-agent-can-see').textContent).toBe('agent can see this')
    expect(within(row('STRIPE_KEY')).queryByTestId('vault-agent-can-see')).toBeNull()
    const chat = row('CHAT_KEY')
    expect(chat.textContent).toContain('xn--pple-43d.com')
    expect(within(chat).getByTestId('vault-mixed-script').textContent).toContain('mixed scripts')
    expect(row('STRIPE_KEY').textContent).toContain('k2v://ws/sales/STRIPE_KEY')
  })

  it('never renders a value except in the reveal panel after a confirm, and hides it after 30 s', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    postHandler = (route) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'vault/reveal') return { valueB64: toBase64(VALUE), version: 2 }
      return { ok: true }
    }
    const { container } = await renderPage()
    expect(container.textContent).not.toContain(VALUE)
    fireEvent.click(within(row('STRIPE_KEY')).getByTestId('vault-row-reveal'))
    fireEvent.click(screen.getByTestId('vault-reveal-start'))
    const pw = (await screen.findByTestId('vault-reveal-password')) as HTMLInputElement
    fireEvent.change(pw, { target: { value: 'correct horse' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    const panel = await screen.findByTestId('vault-reveal-panel')
    // Masked until Show.
    expect(container.textContent).not.toContain(VALUE)
    fireEvent.click(within(panel).getByRole('button', { name: 'Show' }))
    expect(within(panel).getByTestId('vault-reveal-value').textContent).toBe(VALUE)
    // Only inside the panel.
    const outside = container.cloneNode(true) as HTMLElement
    outside.querySelector('[data-testid="vault-reveal-panel"]')?.remove()
    expect(outside.textContent).not.toContain(VALUE)
    expect(posts.map((p) => p.route)).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'vault/reveal'])
    expect(posts[0].body).toEqual({ action: 'reveal', secretId: 'id-STRIPE_KEY' })
    expect(posts[1].body).toEqual({
      action: 'reveal',
      secretId: 'id-STRIPE_KEY',
      method: 'vault-password',
      label: 'STRIPE_KEY',
      password: 'correct horse',
    })
    expect(posts[2].body).toEqual({ id: 'id-STRIPE_KEY', confirmToken: 'tok-1' })
    act(() => {
      vi.advanceTimersByTime(REVEAL_SHOW_MS)
    })
    expect(screen.queryByTestId('vault-reveal-panel')).toBeNull()
    expect(container.textContent).not.toContain(VALUE)
  })

  it('on a Mac, reveal uses the system sheet (local-auth) with no password field', async () => {
    postHandler = (route) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password', 'local-auth'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-2', expiresIn: 60 }
      if (route === 'vault/reveal') return { valueB64: toBase64(VALUE), version: 2 }
      return { ok: true }
    }
    await renderPage()
    fireEvent.click(within(row('STRIPE_KEY')).getByTestId('vault-row-reveal'))
    fireEvent.click(screen.getByTestId('vault-reveal-start'))
    await screen.findByTestId('vault-reveal-panel')
    expect(screen.queryByTestId('vault-reveal-password')).toBeNull()
    expect(posts[1].body).toEqual({ action: 'reveal', secretId: 'id-STRIPE_KEY', method: 'local-auth', label: 'STRIPE_KEY' })
  })

  it("shows K2's own secrets only when the daemon returns system rows, as metadata", async () => {
    systemCall = async () => ({
      secrets: [...userRows, secret({ id: 'sys-1', name: 'mail.admin', namespace: 'system', scope: { kind: 'server' }, ref: null })],
    })
    await renderPage()
    const sys = await screen.findByTestId('vault-system')
    const rows = within(sys).getAllByTestId('vault-system-row')
    expect(rows).toHaveLength(1)
    expect(rows[0].textContent).toContain('mail.admin')
    expect(within(sys).queryByRole('button')).toBeNull()
  })

  it('no system section for a list without system rows (a Member) or a refused one', async () => {
    systemCall = async () => ({ secrets: userRows })
    await renderPage()
    expect(screen.queryByTestId('vault-system')).toBeNull()
    cleanup()
    systemCall = async () => {
      throw new Error(JSON.stringify({ error: { code: 'role_required', hint: 'needs an Admin' } }))
    }
    await renderPage()
    expect(screen.queryByTestId('vault-system')).toBeNull()
  })

  it('rotate sends base64 in the body and empties the field', async () => {
    const { container } = await renderPage()
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Rotate' }))
    const field = screen.getByLabelText('New value for STRIPE_KEY') as HTMLInputElement
    fireEvent.change(field, { target: { value: VALUE } })
    fireEvent.click(within(screen.getByTestId('vault-rotate-panel')).getByRole('button', { name: 'Rotate' }))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/rotate')).toBe(true))
    expect(posts.find((p) => p.route === 'vault/rotate')!.body).toEqual({ id: 'id-STRIPE_KEY', valueB64: toBase64(VALUE) })
    expect(field.value).toBe('')
    expect(container.textContent).not.toContain(VALUE)
  })

  it('destroy needs the name typed, then posts yes', async () => {
    await renderPage()
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Destroy' }))
    const panel = screen.getByTestId('vault-destroy-panel')
    const go = within(panel).getByRole('button', { name: 'Destroy' }) as HTMLButtonElement
    expect(go.disabled).toBe(true)
    fireEvent.change(within(panel).getByLabelText('Type the name to confirm'), { target: { value: 'STRIPE_KEY' } })
    expect(go.disabled).toBe(false)
    fireEvent.click(go)
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/destroy')).toBe(true))
    expect(posts.find((p) => p.route === 'vault/destroy')!.body).toEqual({ id: 'id-STRIPE_KEY', yes: true })
  })

  it('edit sites removes and adds through vault/policy', async () => {
    await renderPage()
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Edit sites' }))
    const panel = screen.getByTestId('vault-sites-panel')
    fireEvent.click(within(panel).getByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(posts).toHaveLength(1))
    expect(posts[0]).toEqual({ route: 'vault/policy', body: { id: 'id-STRIPE_KEY', removeHosts: ['https://api.stripe.com:443'] } })
  })

  it('add secret with a preset sends the preset, not its sites, and the value as base64', async () => {
    const { container } = await renderPage()
    fireEvent.click(screen.getByTestId('vault-add'))
    const form = screen.getByTestId('vault-add-form')
    fireEvent.change(within(form).getByPlaceholderText('STRIPE_KEY'), { target: { value: 'NEW_KEY' } })
    fireEvent.change(within(form).getByTestId('vault-add-preset'), { target: { value: 'stripe' } })
    expect((within(form).getByTestId('vault-add-sites') as HTMLInputElement).value).toBe('api.stripe.com')
    const value = within(form).getByLabelText('Value') as HTMLInputElement
    fireEvent.change(value, { target: { value: VALUE } })
    fireEvent.click(within(form).getByTestId('vault-add-save'))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/set')).toBe(true))
    expect(posts.find((p) => p.route === 'vault/set')!.body).toEqual({
      name: 'NEW_KEY',
      scope: 'workspace',
      workspace: 'p-sales',
      valueB64: toBase64(VALUE),
      kind: 'api_key',
      delivery: 'fetch',
      preset: 'stripe',
      mode: 'create',
    })
    expect(container.textContent).not.toContain(VALUE)
  })
})

describe('groupSecrets', () => {
  it('drops system rows and orders server, workspaces A–Z, chats, other', () => {
    const g = groupSecrets([
      secret({ name: 'B', workspace: 'zeta' }),
      secret({ name: 'A', workspace: 'alpha' }),
      secret({ name: 'S', scope: { kind: 'server' } }),
      secret({ name: 'C', scope: { kind: 'session', id: 'x' } }),
      secret({ name: 'E', scope: { kind: 'engine', id: 'sew' } }),
      secret({ name: 'K', namespace: 'system', scope: { kind: 'server' } }),
    ])
    expect(g.map((x) => [x.key, x.rows.map((r) => r.name)])).toEqual([
      ['server', ['S']],
      ['ws:alpha', ['A']],
      ['ws:zeta', ['B']],
      ['session', ['C']],
      ['other', ['E']],
    ])
  })
})
