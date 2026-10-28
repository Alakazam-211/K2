// @vitest-environment jsdom
// Settings → Notifications → Email notices and Settings → Workspaces →
// "Let this agent email people on this server" (prd-email-notifications-
// part2-v1 §4 N10, V). Both only read the daemon's answers and send the
// person's gestures to the scope they are given.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  address: null as unknown,
  pending: [] as unknown[],
  muted: [] as string[],
  role: 'owner' as string,
  methods: ['local-auth'] as string[],
  agents: [] as unknown[],
  agentsStatus: 200,
  noticeStatus: 200,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  const { DaemonHttpError } = await import('@/lib/daemon-http-error')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'notify/email/address') {
        if (h.noticeStatus !== 200) throw new DaemonHttpError('no route', h.noticeStatus, 'not_found')
        return { ok: true, person: 'owner', address: h.address, pending: h.pending, mutedWorkspaces: h.muted }
      }
      if (route === 'auth/whoami') return { role: h.role, owner: h.role === 'owner' }
      if (route === 'notifications/email/agents') {
        if (h.agentsStatus !== 200) throw new DaemonHttpError('owner_only', h.agentsStatus, 'owner_only')
        return { ok: true, agents: h.agents }
      }
      if (route === 'notifications/email') {
        return {
          ok: true,
          senders: [
            { address: 'system@notify.brand.test', owner: 'system', app: null, state: 'ready' },
            { address: 'shop@notify.brand.test', owner: 'app:svc', app: 'svc', state: 'ready' },
            { address: 'notices@notify.brand.test', owner: null, app: null, state: 'ready' },
            { address: 'taken@notify.brand.test', owner: null, app: null, state: 'ready' },
          ],
        }
      }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route === 'notify/email/address/set') {
        h.pending = [{ id: 'eva_1', address: body.address, state: 'pending', createdAt: 1, expiresAt: 1_900_000_000, verifiedAt: null }]
        return { ok: true, sent: true }
      }
      if (route === 'notify/email/address/remove') {
        h.address = null
        return { ok: true, removed: 1 }
      }
      if (route === 'notify/email/mute') {
        h.muted = body.muted ? [String(body.workspace)] : []
        return { ok: true }
      }
      if (route === 'vault/confirm/start') return { methods: h.methods, expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'notifications/email/agent/on') {
        h.agents = [{ workspace: body.workspace, sender: body.sender, on: true, offReason: null, grantedAt: 1 }]
        return { ok: true }
      }
      if (route === 'notifications/email/agent/off') {
        h.agents = [{ workspace: body.workspace, sender: 'notices@notify.brand.test', on: false, offReason: null, grantedAt: 1 }]
        return { ok: true }
      }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { primaryScope } from '@/kessel/server-scope'
import { EmailNoticePrefs } from './EmailNoticePrefs'
import { AgentEmailNoticesRow, agentNoticeLine } from './AgentEmailNoticesRow'
import { agentOnBinding, parseMyEmailNotices } from './email-notice-api'

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.address = { id: 'eva_0', address: 'me@home.test', state: 'verified', createdAt: 1, expiresAt: null, verifiedAt: 2 }
  h.pending = []
  h.muted = []
  h.role = 'owner'
  h.methods = ['local-auth']
  h.agents = [{ workspace: 'ws-other', sender: 'taken@notify.brand.test', on: true, offReason: null, grantedAt: 1 }]
  h.agentsStatus = 200
  h.noticeStatus = 200
})
afterEach(() => cleanup())

const workspaces = [
  { projectId: 'ws-sales', name: 'Sales' },
  { projectId: 'ws-ops', name: 'Ops' },
]

describe('EmailNoticePrefs', () => {
  it('shows the confirmed address and turns one workspace off for me', async () => {
    render(<EmailNoticePrefs scope={primaryScope()} workspaces={workspaces} />)
    expect(await screen.findByText('me@home.test')).toBeTruthy()
    expect(screen.getByText(/Confirmed: agents here can email you/)).toBeTruthy()
    fireEvent.click(screen.getByLabelText('Email notices from Ops'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notify/email/mute')?.body).toEqual({ workspace: 'ws-ops', muted: true }))
    await waitFor(() => expect(screen.getByTestId('email-notice-ws-ws-ops').textContent).toContain('Off for you'))
  })

  it('sends a confirm link to a new address, shows it waiting, and removes an address', async () => {
    render(<EmailNoticePrefs scope={primaryScope()} workspaces={workspaces} />)
    await screen.findByText('me@home.test')
    fireEvent.change(screen.getByLabelText('Your address for email notices'), { target: { value: ' new@home.test ' } })
    fireEvent.click(screen.getByText('Send confirm link'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notify/email/address/set')?.body).toEqual({ address: 'new@home.test' }))
    expect(await screen.findByTestId('email-notice-sent')).toBeTruthy()
    expect(await screen.findByTestId('email-notice-address-pending')).toBeTruthy()
    fireEvent.click(screen.getByLabelText('Stop email notices to me@home.test'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notify/email/address/remove')?.body).toEqual({ address: 'me@home.test' }))
  })

  it('hides itself on a server without email notices', async () => {
    h.noticeStatus = 404
    const { container } = render(<EmailNoticePrefs scope={primaryScope()} workspaces={workspaces} />)
    await waitFor(() => expect(h.gets.some((g) => g.route === 'notify/email/address')).toBe(true))
    await waitFor(() => expect(container.innerHTML).toBe(''))
  })

  it('parses only what the daemon gave', () => {
    expect(parseMyEmailNotices({ person: 'gina', address: null, pending: [{ nope: 1 }], mutedWorkspaces: ['a', 2] })).toEqual({
      person: 'gina',
      address: null,
      pending: [],
      mutedWorkspaces: ['a'],
    })
  })
})

describe('AgentEmailNoticesRow', () => {
  it('turns a workspace on with a free sender and a confirm bound to the workspace and the sender', async () => {
    h.agents = [{ workspace: 'ws-other', sender: 'taken@notify.brand.test', on: true, offReason: null, grantedAt: 1 }]
    render(<AgentEmailNoticesRow scope={primaryScope()} projectId="ws-sales" />)
    expect(await screen.findByText(/^Off\. The agent can ask you/)).toBeTruthy()
    fireEvent.click(await screen.findByLabelText('Let this agent email people on this server'))
    const select = (await screen.findByLabelText("Sender for this workspace's notices")) as HTMLSelectElement
    // Only a free sender: never the system sender, an App's or another workspace's.
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['notices@notify.brand.test'])
    fireEvent.click(screen.getByText('Confirm and turn on'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/agent/on')).toBe(true))
    expect(h.posts.find((p) => p.route === 'vault/confirm/start')?.body).toEqual({
      action: 'agent-email-on',
      secretId: 'ws-sales|notices@notify.brand.test',
    })
    expect(h.posts.find((p) => p.route === 'notifications/email/agent/on')?.body).toEqual({
      workspace: 'ws-sales',
      sender: 'notices@notify.brand.test',
      confirmToken: 'tok-1',
    })
    expect(await screen.findByText(/^On: the agent emails people here from notices@notify\.brand\.test/)).toBeTruthy()
    // Off needs no confirm (the same as the CLI).
    fireEvent.click(screen.getByLabelText('Let this agent email people on this server'))
    await waitFor(() => expect(h.posts.find((p) => p.route === 'notifications/email/agent/off')?.body).toEqual({ workspace: 'ws-sales' }))
  })

  it('asks for the vault password when the server offers no Touch ID', async () => {
    h.methods = ['vault-password']
    render(<AgentEmailNoticesRow scope={primaryScope()} projectId="ws-sales" />)
    fireEvent.click(await screen.findByLabelText('Let this agent email people on this server'))
    await screen.findByLabelText("Sender for this workspace's notices")
    fireEvent.click(screen.getByText('Confirm and turn on'))
    const pw = await screen.findByLabelText('Vault password')
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByText('Confirm and turn on'))
    await waitFor(() => expect(h.posts.some((p) => p.route === 'notifications/email/agent/on')).toBe(true))
    expect(h.posts.filter((p) => p.route === 'vault/confirm/finish').pop()?.body).toMatchObject({ method: 'vault-password', password: 'hunter2' })
  })

  it('an Admin sees the state but no switch; a Member sees nothing', async () => {
    h.role = 'admin'
    render(<AgentEmailNoticesRow scope={primaryScope()} projectId="ws-sales" />)
    expect(await screen.findByText('Only the owner turns this on or off.')).toBeTruthy()
    expect(screen.queryByLabelText('Let this agent email people on this server')).toBeNull()
    cleanup()
    h.agentsStatus = 403
    const { container } = render(<AgentEmailNoticesRow scope={primaryScope()} projectId="ws-sales" />)
    await waitFor(() => expect(container.innerHTML).toBe(''))
  })

  it('words', () => {
    expect(agentOnBinding('ws-1', ' Notices@Notify.Brand.test ')).toBe('ws-1|notices@notify.brand.test')
    expect(agentNoticeLine({ workspace: 'w', sender: 'a@b.test', on: false, offReason: 'agent', grantedAt: 1 })).toBe('Off (agent). Sender kept: a@b.test.')
  })
})
