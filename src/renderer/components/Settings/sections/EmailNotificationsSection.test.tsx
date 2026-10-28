// @vitest-environment jsdom
// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8): the page only reads `/cli/notifications/email/*` and sends the
// owner's gestures. A paused sender shows why and resumes after a vault
// confirm bound to its address; the system sender is never "paused";
// suppressions are searched by address and never shown as addresses.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  methods: ['local-auth'] as string[],
  paused: true,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  const shop = {
    id: 'ens_shop',
    address: 'noreply@notify.brand.test',
    domain: 'notify.brand.test',
    path: 'hosted',
    owner: 'app:svc_shop',
    app: 'svc_shop',
    fromName: 'Shop',
    state: 'ready',
    envelope: null,
    paused: null as unknown,
  }
  const sys = { ...shop, id: 'ens_sys', address: 'system@notify.brand.test', owner: 'system', app: null, paused: null }
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'notifications/email') {
        return {
          ok: true,
          boxSwitch: 'on',
          hostedMail: true,
          airgap: false,
          systemSender: sys,
          senders: [
            sys,
            { ...shop, paused: h.paused ? { pausedAt: 1_800_000_000, reason: 'hard_bounces', rate: { sent: 80, hardBounces: 7, complaints: 0 } } : null },
          ],
          relays: [],
          settings: { logRetentionDays: 30, ownerEmail: null },
        }
      }
      if (route === 'notifications/email/agents') return { ok: true, agents: [{ workspace: 'sales', sender: 'notices@notify.brand.test', on: true, offReason: null, grantedAt: 1 }] }
      if (route === 'notifications/email/suppressions') {
        return { ok: true, suppressions: [{ addressSha256: 'ab'.repeat(32), domain: 'x.test', scope: 'box', scopeRef: null, reason: 'hard_bounce', createdAt: 1, createdBy: 'dsn' }] }
      }
      if (route === 'notifications/email/log') return { ok: true, items: [{ id: 'enl_1', to: ['a@x.test'], status: 'submitted', origin: 'app', purpose: null, error: null, createdAt: 1 }] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route === 'vault/confirm/start') return { methods: h.methods, expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'notifications/email/sender/resume') {
        h.paused = false
        return { ok: true, sender: body.sender, paused: false }
      }
      if (route === 'notifications/email/agent/off') return { ok: true }
      if (route === 'notifications/email/suppressions/rm') return { ok: true, removed: 1 }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { EmailNotificationsSection } from './EmailNotificationsSection'
import { primaryScope } from '@/kessel/server-scope'
import { ownerText, pauseText, pickMethod, suppressionScopeText } from './email-notify-api'

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.methods = ['local-auth']
  h.paused = true
})
afterEach(() => cleanup())

describe('EmailNotificationsSection', () => {
  it('shows a paused App sender with why, and resumes it after a confirm bound to its address', async () => {
    render(<EmailNotificationsSection scope={primaryScope()} />)
    await screen.findByTestId('email-notifications')
    expect(screen.getByText(/7 of 80 recipients hard-bounced in 24 h/)).toBeTruthy()
    expect(screen.getByText(/never paused: it carries every password reset/)).toBeTruthy()
    fireEvent.click(screen.getByText('Resume…'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/sender/resume')).toBe(true))
    const start = h.posts.find((p) => p.route === 'vault/confirm/start')
    expect(start?.body).toEqual({ action: 'email-resume', secretId: 'noreply@notify.brand.test' })
    const finish = h.posts.find((p) => p.route === 'vault/confirm/finish')
    expect(finish?.body.method).toBe('local-auth')
    const resume = h.posts.find((p) => p.route === 'notifications/email/sender/resume')
    expect(resume?.body).toEqual({ sender: 'noreply@notify.brand.test', confirmToken: 'tok-1' })
    await waitFor(() => expect(screen.queryByText('Resume…')).toBeNull())
  })

  it('asks for the vault password when the server offers no Touch ID', async () => {
    h.methods = ['vault-password']
    render(<EmailNotificationsSection scope={primaryScope()} />)
    await screen.findByTestId('email-notifications')
    fireEvent.click(screen.getByText('Resume…'))
    const pw = await screen.findByLabelText('Vault password')
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByText('Resume…'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/sender/resume')).toBe(true))
    const finish = h.posts.find((p) => p.route === 'vault/confirm/finish')
    expect(finish?.body).toMatchObject({ method: 'vault-password', password: 'hunter2' })
  })

  it('turns a workspace off and removes a suppression by address, never showing addresses it keeps hashed', async () => {
    render(<EmailNotificationsSection scope={primaryScope()} />)
    await screen.findByTestId('email-notifications')
    expect(screen.getByText('@x.test')).toBeTruthy()
    fireEvent.click(screen.getByText('Turn off'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/agent/off')?.body).toEqual({ workspace: 'sales' }))
    fireEvent.change(screen.getByLabelText('Address'), { target: { value: 'gone@x.test' } })
    fireEvent.click(screen.getByText('Remove'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/suppressions/rm')?.body).toEqual({ address: 'gone@x.test', all: true }))
    expect(await screen.findByText(/mail may go to gone@x.test again/)).toBeTruthy()
  })
})

describe('email-notify-api words', () => {
  it('names owners, pauses, scopes and confirm methods', () => {
    const base = { id: 'a', address: 'a@b', domain: 'b', path: 'hosted', fromName: 'A', state: 'ready', envelope: null, paused: null }
    expect(ownerText({ ...base, owner: 'system', app: null })).toBe('System sender')
    expect(ownerText({ ...base, owner: 'app:svc', app: 'svc' })).toBe('App svc')
    expect(ownerText({ ...base, owner: null, app: null })).toBe('Unassigned')
    expect(pauseText({ pausedAt: 1, reason: 'complaints', rate: { sent: 1000, complaints: 2 } })).toBe('2 of 1000 recipients reported spam in 24 h')
    expect(suppressionScopeText({ addressSha256: 'x', domain: 'd', scope: 'sender', scopeRef: 'ens_1', reason: 'complaint', createdAt: 1, createdBy: 'arf' })).toBe('sender ens_1')
    expect(pickMethod(['vault-password'])).toBeNull()
    expect(pickMethod(['vault-password', 'local-auth'])).toBe('local-auth')
  })
})
