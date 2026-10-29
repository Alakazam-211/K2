// @vitest-environment jsdom
// Settings → Access → Vault (prd-k2-vault-v1 §16.1, KV39).

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

import { primaryScope } from '@/kessel/server-scope'
import { VaultSection, groupSecrets, originLabel, REVEAL_SHOW_MS } from './VaultSection'
import { requestVaultTab, type HandoffView } from './handoff-api'
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
  const r = render(<VaultSection scope={primaryScope()} />)
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
    expect(banner.textContent).not.toContain('coming later')
    expect(within(banner).getByTestId('vault-recovery-start').textContent).toBe('Set up recovery key')
    expect(document.body.textContent).not.toMatch(/agents can.t read/i)
  })

  it('sets up the recovery key from the app: Mac confirm, words shown once, three typed back', async () => {
    const words = Array.from({ length: 24 }, (_, i) => `w${i + 1}`)
    let confirmed = false
    daemonCliGet.mockImplementation(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
      if (route === 'vault/status')
        return confirmed ? { ...STATUS, recovery: 'confirmed', recoveryConfirmedAt: '2026-10-11T00:00:00Z', recoveryHint: undefined } : STATUS
      if (route === 'vault/list') return params?.system ? systemCall() : { secrets: userRows }
      if (route === 'vault/presets') return { presets: [] }
      if (route === 'vault/requests') return { requests: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    postHandler = (route, body) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-r', expiresIn: 60 }
      if (route === 'vault/recovery/show-once')
        return { id: 'rc-1', phrase: words.join(' '), checkWords: [3, 11, 20], recipient: 'age1x' }
      if (route === 'vault/recovery/confirm') {
        expect(body).toEqual({ id: 'rc-1', words: ['w3', 'w11', 'w20'] })
        confirmed = true
        return { ok: true, recovery: 'confirmed', recipient: 'age1x' }
      }
      throw new Error(`unexpected POST ${route}`)
    }
    const { container } = await renderPage()
    fireEvent.click(screen.getByTestId('vault-recovery-start'))
    const shown = await screen.findByTestId('vault-recovery-phrase')
    expect(within(shown).getAllByTestId('vault-recovery-word')).toHaveLength(24)
    expect(shown.textContent).toContain('w24')
    // No copy button: the clipboard is readable by agents.
    expect(within(shown).queryByRole('button', { name: /copy/i })).toBeNull()
    expect(posts.slice(0, 3)).toEqual([
      { route: 'vault/confirm/start', body: { action: 'recovery-setup' } },
      { route: 'vault/confirm/finish', body: { action: 'recovery-setup', method: 'local-auth', label: 'recovery key' } },
      { route: 'vault/recovery/show-once', body: { confirmToken: 'tok-r' } },
    ])
    fireEvent.click(screen.getByTestId('vault-recovery-saved'))
    // The words are gone once the check starts.
    expect(container.textContent).not.toContain('w24')
    fireEvent.change(screen.getByTestId('vault-recovery-word-3'), { target: { value: 'w3' } })
    fireEvent.change(screen.getByTestId('vault-recovery-word-11'), { target: { value: ' W11 ' } })
    fireEvent.change(screen.getByTestId('vault-recovery-word-20'), { target: { value: 'w20' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm recovery key' }))
    await waitFor(() => expect(screen.queryByTestId('vault-recovery-banner')).toBeNull())
    expect(posts.map((p) => p.route)).toContain('vault/recovery/confirm')
  })

  it('recovery setup asks for the vault password when the Mac sheet is not offered, and restarts on wrong words', async () => {
    postHandler = (route) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-p', expiresIn: 60 }
      if (route === 'vault/recovery/show-once') return { id: 'rc-2', phrase: 'a b c d e f g h i j k l m n o p q r s t u v w x', checkWords: [3, 11, 20], recipient: 'age1y' }
      if (route === 'vault/recovery/confirm') throw new Error("those words don't match; start again with show-once")
      throw new Error(`unexpected POST ${route}`)
    }
    await renderPage()
    fireEvent.click(screen.getByTestId('vault-recovery-start'))
    const pw = (await screen.findByTestId('vault-recovery-password')) as HTMLInputElement
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.click(within(screen.getByTestId('vault-recovery-setup')).getByRole('button', { name: 'Confirm' }))
    await screen.findByTestId('vault-recovery-phrase')
    expect(posts[1].body).toEqual({ action: 'recovery-setup', method: 'vault-password', label: 'recovery key', password: 'correct horse battery' })
    fireEvent.click(screen.getByTestId('vault-recovery-saved'))
    for (const n of [3, 11, 20]) fireEvent.change(screen.getByTestId(`vault-recovery-word-${n}`), { target: { value: 'nope' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm recovery key' }))
    await screen.findByTestId('vault-recovery-start')
    expect(screen.getByTestId('vault-recovery-setup').textContent).toContain('Start again')
    expect(screen.getByTestId('vault-recovery-banner')).toBeTruthy()
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

  it('agent delivery: offered only for SSH keys, and never warns that agents can see the value', async () => {
    userRows = [secret({ name: 'DEPLOY_KEY', kind: 'ssh_key', hosts: [], hostsDisplay: [], defaultDelivery: 'agent', deliveryCeiling: 'env' }), ...userRows]
    await renderPage()
    fireEvent.click(within(row('DEPLOY_KEY')).getByRole('button', { name: 'Change delivery' }))
    const panel = within(row('DEPLOY_KEY')).getByTestId('vault-delivery-panel')
    const select = within(panel).getByTestId('vault-delivery-select') as HTMLSelectElement
    expect(select.value).toBe('agent')
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['agent', 'file', 'env'])
    expect(panel.textContent).not.toContain('Agents can see this value')
    fireEvent.change(select, { target: { value: 'file' } })
    expect(panel.textContent).toContain('Agents can see this value')
    // An API key has no agent choice.
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Change delivery' }))
    const p2 = within(row('STRIPE_KEY')).getByTestId('vault-delivery-panel')
    expect(Array.from((within(p2).getByTestId('vault-delivery-select') as HTMLSelectElement).options).map((o) => o.value)).not.toContain('agent')
  })

  it('the Share panel shows the delivery it will send, even one outside the usual choices', async () => {
    userRows = [secret({ name: 'API_KEY_X', defaultDelivery: 'api', deliveryCeiling: 'api' }), ...userRows]
    postHandler = (route) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-g', expiresIn: 60 }
      return { ok: true }
    }
    await renderPage()
    fireEvent.click(within(row('API_KEY_X')).getByRole('button', { name: 'Share' }))
    const panel = within(row('API_KEY_X')).getByTestId('vault-share-panel')
    const as = within(panel).getByTestId('vault-share-as') as HTMLSelectElement
    expect(as.value).toBe('api')
    expect(as.options[as.selectedIndex].textContent).toBe('the agent can print it')
    expect(panel.textContent).toContain('They can see this value')
    fireEvent.click(within(panel).getByRole('button', { name: 'Share' }))
    // VC-B2: a share they can read asks for a confirm first, like reveal.
    const pw = (await within(panel).findByTestId('vault-share-password')) as HTMLInputElement
    expect(posts.some((p) => p.route === 'vault/grant')).toBe(false)
    fireEvent.change(pw, { target: { value: 'correct horse' } })
    fireEvent.click(within(panel).getByRole('button', { name: 'Confirm and share' }))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/grant')).toBe(true))
    expect(posts.map((p) => p.route)).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'vault/grant'])
    expect(posts[0].body).toEqual({ action: 'grant-read', secretId: 'id-API_KEY_X' })
    expect(posts[1].body).toMatchObject({ action: 'grant-read', secretId: 'id-API_KEY_X', method: 'vault-password', password: 'correct horse' })
    expect(posts[2].body).toEqual({ id: 'id-API_KEY_X', to: 'ws:*', as: 'api', confirmToken: 'tok-g' })
  })

  it('a share that never shows the value needs no confirm', async () => {
    await renderPage()
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Share' }))
    const panel = within(row('STRIPE_KEY')).getByTestId('vault-share-panel')
    expect((within(panel).getByTestId('vault-share-as') as HTMLSelectElement).value).toBe('fetch')
    expect(panel.textContent).not.toContain('They can see this value')
    fireEvent.click(within(panel).getByRole('button', { name: 'Share' }))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/grant')).toBe(true))
    expect(posts.map((p) => p.route)).toEqual(['vault/grant'])
    expect(posts[0].body).toEqual({ id: 'id-STRIPE_KEY', to: 'ws:*', as: 'fetch' })
  })

  it('an SSH key keeps its line breaks: add and rotate use a multi-line field', async () => {
    const KEY = '-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nk2testline2\n-----END OPENSSH PRIVATE KEY-----\n'
    userRows = [secret({ name: 'DEPLOY_KEY', kind: 'ssh_key', hosts: [], hostsDisplay: [], defaultDelivery: 'agent', deliveryCeiling: 'env' }), ...userRows]
    const { container } = await renderPage()
    // Add.
    fireEvent.click(screen.getByTestId('vault-add'))
    const form = screen.getByTestId('vault-add-form')
    fireEvent.change(within(form).getByPlaceholderText('STRIPE_KEY'), { target: { value: 'NEW_SSH' } })
    fireEvent.change(within(form).getByTestId('vault-add-kind'), { target: { value: 'ssh_key' } })
    const area = within(form).getByTestId('vault-value-multiline') as HTMLTextAreaElement
    expect(area.tagName).toBe('TEXTAREA')
    fireEvent.change(area, { target: { value: KEY } })
    fireEvent.click(within(form).getByTestId('vault-add-save'))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/set')).toBe(true))
    const set = posts.find((p) => p.route === 'vault/set')!.body
    expect(set.valueB64).toBe(toBase64(KEY))
    expect(set.kind).toBe('ssh_key')
    expect(set.delivery).toBe('file')
    // Rotate.
    fireEvent.click(within(row('DEPLOY_KEY')).getByRole('button', { name: 'Rotate' }))
    const panel = within(row('DEPLOY_KEY')).getByTestId('vault-rotate-panel')
    const area2 = within(panel).getByTestId('vault-value-multiline') as HTMLTextAreaElement
    fireEvent.change(area2, { target: { value: KEY } })
    fireEvent.click(within(panel).getByRole('button', { name: 'Rotate' }))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/rotate')).toBe(true))
    expect(posts.find((p) => p.route === 'vault/rotate')!.body.valueB64).toBe(toBase64(KEY))
    expect(container.textContent).not.toContain('k2testline2')
    // An API key still uses a one-line password field.
    fireEvent.click(within(row('STRIPE_KEY')).getByRole('button', { name: 'Rotate' }))
    const p3 = within(row('STRIPE_KEY')).getByTestId('vault-rotate-panel')
    expect(within(p3).queryByTestId('vault-value-multiline')).toBeNull()
  })

  it('add secret: picking SSH key offers agent delivery', async () => {
    await renderPage()
    fireEvent.click(screen.getByTestId('vault-add'))
    const form = screen.getByTestId('vault-add-form')
    const delivery = () => within(form).getByTestId('vault-add-delivery') as HTMLSelectElement
    expect(Array.from(delivery().options).map((o) => o.value)).not.toContain('agent')
    fireEvent.change(within(form).getByTestId('vault-add-kind'), { target: { value: 'ssh_key' } })
    expect(Array.from(delivery().options).map((o) => o.value)).toContain('agent')
    fireEvent.change(delivery(), { target: { value: 'agent' } })
    expect(form.textContent).not.toContain('Agents can see this value')
    fireEvent.change(within(form).getByTestId('vault-add-kind'), { target: { value: 'api_key' } })
    expect(delivery().value).toBe('fetch')
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

describe('originLabel (handoff, prd-agent-secret-handoff-v1 §10.3)', () => {
  it('names the sender, host, day and short delivery id', () => {
    expect(
      originLabel({
        origin: 'handoff:hd_7f3c0000000000000000000000000000',
        createdBy: 'system:handoff',
        handoff: { deliveryId: 'hd_7f3c0000000000000000000000000000', from: 'sales::rosson.k2.dev', at: '2026-10-16T10:00:00Z' },
      }),
    ).toBe('sent by sales on rosson.k2.dev, 16 Oct (hd_7f3c…)')
    expect(
      originLabel({
        origin: 'handoff:hd_7f3c0000000000000000000000000000',
        createdBy: 'system:handoff',
        handoff: { deliveryId: 'hd_7f3c0000000000000000000000000000', from: 'billing', at: null },
      }),
    ).toBe('sent by billing (hd_7f3c…)')
    expect(originLabel({ origin: 'handoff:hd_abcd0000000000000000000000000000', createdBy: 'system:handoff' })).toBe(
      'sent by another agent (hd_abcd…)',
    )
    expect(originLabel({ origin: 'thread:c1', createdBy: 'owner' })).toBe('Thread card')
  })
})

describe('Vault → Incoming / Sent (prd-agent-secret-handoff-v1 §10.3)', () => {
  function hv(over: Partial<HandoffView>): HandoffView {
    return {
      deliveryId: 'hd_1111aaaa000000000000000000000000',
      direction: 'in',
      status: 'pending',
      workspaceId: 'p-ops',
      workspace: 'ops',
      peer: 'sales::rosson.k2.dev',
      cross: true,
      name: 'STRIPE_KEY',
      kind: 'api_key',
      hosts: ['https://api.stripe.com:443'],
      note: 'for the refund job',
      reason: 'not_allowed',
      createdAt: '2026-10-17T09:00:00Z',
      ref: null,
      ...over,
    }
  }

  let incoming: HandoffView[] = []
  let sent: HandoffView[] = []

  beforeEach(() => {
    incoming = [
      hv({}),
      hv({ deliveryId: 'hd_2222bbbb000000000000000000000000', name: 'OLD_KEY', status: 'landed', ref: 'k2v://ws/ops/OLD_KEY', decidedBy: 'owner' }),
    ]
    sent = [hv({ deliveryId: 'hd_3333cccc000000000000000000000000', direction: 'out', workspace: 'sales', peer: 'ops', status: 'landed' })]
    const base = daemonCliGet.getMockImplementation()!
    daemonCliGet.mockImplementation(async (scope: unknown, route: string, params?: Record<string, unknown>) => {
      if (route === 'vault/incoming') return { items: incoming }
      if (route === 'vault/sent') return { items: sent }
      return base(scope, route, params)
    })
  })

  it('lists incoming rows with peer, status and short id; Accept posts decide and reloads', async () => {
    await renderPage()
    fireEvent.click(screen.getByRole('tab', { name: 'Incoming' }))
    const rows = await screen.findAllByTestId('vault-incoming-row')
    expect(rows).toHaveLength(2)
    expect(rows[0]!.textContent).toContain('from sales on rosson.k2.dev to ops')
    expect(rows[0]!.textContent).toContain('waiting for a person')
    expect(rows[0]!.textContent).toContain('hd_1111…')
    expect(rows[1]!.textContent).toContain('k2v://ws/ops/OLD_KEY')
    expect(within(rows[1]!).queryByTestId('vault-incoming-accept')).toBeNull()
    postHandler = (route) => {
      if (route === 'vault/incoming/decide') {
        incoming = [hv({ status: 'landed', ref: 'k2v://ws/ops/STRIPE_KEY' }), incoming[1]!]
        return { ok: true, delivery: incoming[0] }
      }
      return { ok: true }
    }
    fireEvent.click(within(rows[0]!).getByTestId('vault-incoming-accept'))
    await waitFor(() => expect(posts.some((p) => p.route === 'vault/incoming/decide')).toBe(true))
    expect(posts.find((p) => p.route === 'vault/incoming/decide')!.body).toEqual({
      id: 'hd_1111aaaa000000000000000000000000',
      action: 'accept',
    })
    await waitFor(() => expect(screen.getAllByTestId('vault-incoming-row')[0]!.getAttribute('data-status')).toBe('landed'))
  })

  it('name_taken on Accept offers Accept as and Replace', async () => {
    postHandler = (route) => {
      if (route === 'vault/incoming/decide') {
        throw new Error(JSON.stringify({ error: { code: 'name_taken', hint: 'this workspace already has STRIPE_KEY' } }))
      }
      return { ok: true }
    }
    await renderPage()
    fireEvent.click(screen.getByRole('tab', { name: 'Incoming' }))
    const row = (await screen.findAllByTestId('vault-incoming-row'))[0]!
    fireEvent.click(within(row).getByTestId('vault-incoming-accept'))
    await within(row).findByTestId('vault-incoming-replace')
    expect(within(row).getByTestId('vault-incoming-as')).toBeTruthy()
    expect(within(row).getByRole('alert').textContent).toContain('already has STRIPE_KEY')
  })

  it('the Sent tab lists what this server sent, and a requested tab opens first', async () => {
    requestVaultTab('sent')
    render(<VaultSection scope={primaryScope()} />)
    const rows = await screen.findAllByTestId('vault-sent-row')
    expect(rows[0]!.textContent).toContain('from sales to ops')
    expect(rows[0]!.textContent).toContain('in the vault')
  })
})
