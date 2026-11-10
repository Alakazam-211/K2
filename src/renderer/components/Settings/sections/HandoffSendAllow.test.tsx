// @vitest-environment jsdom
// Settings → Workspaces → the agent → Permissions: "Secrets it may send to
// other servers" (0.46.11): the standing send grants, each with Revoke, and
// Add behind the vault confirm.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))

import { AgentHandoffSendAllow, remoteAgentOk } from './HandoffSendAllow'
import { coerceSendAllow, type SendAllowEntry } from './handoff-api'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const OPS = { id: 'p-ops', name: 'ops', handle: 'ops', path: '/w/ops' } as unknown as ProjectWithWorkspaces

let list: SendAllowEntry[] = []
let posts: { route: string; body: Record<string, unknown> }[] = []

const SECRETS = [
  { id: 's-geo', name: 'FLEET_DATABASE' },
  { id: 's-map', name: 'MAPS_KEY' },
]

beforeEach(() => {
  cleanup()
  posts = []
  list = [
    {
      workspaceId: 'p-ops',
      secretId: 's-geo',
      name: 'FLEET_DATABASE',
      to: 'argus::rpm.k2.dev',
      addedBy: 'connect:maya',
      addedAt: '2026-11-10T09:00:00Z',
    },
  ]
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockImplementation(async (_s: unknown, route: string, params?: Record<string, unknown>) => {
    expect(params).toEqual({ workspace: 'p-ops' })
    if (route === 'vault/send-allow') return { workspace: 'ops', entries: list, secrets: SECRETS }
    throw new Error(`unexpected GET ${route}`)
  })
  daemonCliPost.mockImplementation(async (_s: unknown, route: string, body: Record<string, unknown>) => {
    posts.push({ route, body: { ...body } })
    if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
    if (route === 'vault/confirm/finish') return { token: 'k2vc_test_token' }
    if (route !== 'vault/send-allow') throw new Error(`unexpected POST ${route}`)
    if (body.action === 'add') {
      const s = SECRETS.find((x) => x.id === body.secretId)!
      list = [...list, { workspaceId: 'p-ops', secretId: s.id, name: s.name, to: String(body.to), addedBy: 'owner', addedAt: '2026-11-10T10:00:00Z' }]
    } else {
      list = list.filter((e) => !(e.secretId === body.secretId && e.to === body.to))
    }
    // A POST answers the grants only (no picker).
    return { ok: true, entries: list }
  })
})

describe('AgentHandoffSendAllow', () => {
  it('lists each grant as "May send <secret> to <agent::host>" with who added it', async () => {
    render(<AgentHandoffSendAllow project={OPS} />)
    const entry = await screen.findByTestId('send-allow-entry')
    expect(entry.textContent).toContain('May send FLEET_DATABASE to argus::rpm.k2.dev')
    expect(entry.textContent).toContain('added by connect:maya, 10 Nov')
    expect(within(entry).getByRole('button', { name: 'Stop letting it send FLEET_DATABASE to argus::rpm.k2.dev' })).toBeTruthy()
  })

  it('Revoke removes it through /cli/vault/send-allow, with no confirm', async () => {
    render(<AgentHandoffSendAllow project={OPS} />)
    const entry = await screen.findByTestId('send-allow-entry')
    fireEvent.click(within(entry).getByTestId('send-allow-revoke'))
    expect(await screen.findByTestId('send-allow-empty')).toBeTruthy()
    expect(posts).toEqual([
      { route: 'vault/send-allow', body: { workspace: 'p-ops', secretId: 's-geo', to: 'argus::rpm.k2.dev', action: 'remove' } },
    ])
  })

  it('Add takes a person-given secret and an agent::host, then the handoff-send vault confirm for that secret', async () => {
    render(<AgentHandoffSendAllow project={OPS} />)
    await screen.findByTestId('send-allow-entry')
    const add = screen.getByTestId('send-allow-add') as HTMLButtonElement
    expect(add.disabled).toBe(true)
    fireEvent.click(screen.getByTestId('send-allow-pick'))
    const options = [...screen.getByTestId('setting-dropdown-menu').querySelectorAll('button')]
    expect(options.map((o) => o.textContent)).toEqual(['FLEET_DATABASE', 'MAPS_KEY'])
    fireEvent.click(options[1])
    fireEvent.change(screen.getByTestId('send-allow-to'), { target: { value: 'ws:p-sales' } })
    expect(add.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('send-allow-to'), { target: { value: 'argus::rpm.k2.dev' } })
    expect(add.disabled).toBe(false)
    fireEvent.click(add)
    expect(screen.getByTestId('send-allow-confirm').textContent).toContain('let it send MAPS_KEY to argus::rpm.k2.dev from now on')
    await waitFor(() => expect(screen.getAllByTestId('send-allow-entry')).toHaveLength(2))
    expect(posts.map((p) => p.route)).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'vault/send-allow'])
    expect(posts[0].body).toEqual({ action: 'handoff-send', secretId: 's-map' })
    expect(posts[2].body).toEqual({
      workspace: 'p-ops',
      secretId: 's-map',
      to: 'argus::rpm.k2.dev',
      action: 'add',
      confirmToken: 'k2vc_test_token',
    })
    // The picker survives a POST that answers the grants only.
    fireEvent.click(screen.getByTestId('send-allow-pick'))
    expect([...screen.getByTestId('setting-dropdown-menu').querySelectorAll('button')].map((o) => o.textContent)).toEqual([
      'FLEET_DATABASE',
      'MAPS_KEY',
    ])
  })

  it('read-only shows the grants without Revoke or Add', async () => {
    render(<AgentHandoffSendAllow project={OPS} readOnly />)
    await screen.findByTestId('send-allow-entry')
    expect(screen.queryByTestId('send-allow-revoke')).toBeNull()
    expect(screen.queryByTestId('send-allow-add')).toBeNull()
  })

  it('a refusal shows the daemon hint', async () => {
    daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'handoff_person_only', hint: 'only an Owner or Admin on this server decides' } }))
    })
    render(<AgentHandoffSendAllow project={OPS} />)
    const entry = await screen.findByTestId('send-allow-entry')
    fireEvent.click(within(entry).getByTestId('send-allow-revoke'))
    expect((await screen.findByRole('alert')).textContent).toContain('only an Owner or Admin')
  })
})

describe('remoteAgentOk and coerceSendAllow', () => {
  it('takes an agent on another server only', () => {
    expect(remoteAgentOk('argus::rpm.k2.dev')).toBe(true)
    expect(remoteAgentOk('argus@rpm.k2.dev')).toBe(true)
    for (const bad of ['', 'argus', 'ws:p-1', 'team:g', '*::host', 'a::b::c']) expect(remoteAgentOk(bad)).toBe(false)
  })

  it('drops rows without a secret id or target, never invents a value field', () => {
    const v = coerceSendAllow({
      entries: [{ secretId: 's1', to: 'a::b', name: 'K', addedBy: 'owner', addedAt: 'x', value: 'nope' }, { name: 'no id' }],
      secrets: [{ id: 's1', name: 'K' }, { id: 3 }],
    })
    expect(v.entries).toEqual([{ workspaceId: '', secretId: 's1', name: 'K', to: 'a::b', addedBy: 'owner', addedAt: 'x' }])
    expect(v.secrets).toEqual([{ id: 's1', name: 'K' }])
  })
})
