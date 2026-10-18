// @vitest-environment jsdom
// Settings → Projects → the agent: "Secrets from other agents"
// (prd-agent-secret-handoff-v1 §7.3, §10.4, H19).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const setSection = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))
vi.mock('@/stores/settings', () => ({ useSettingsStore: { getState: () => ({ setSection }) } }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) =>
    sel({
      projects: [
        { id: 'p-ops', name: 'ops', handle: 'ops', path: '/w/ops' },
        { id: 'p-sales', name: 'sales', handle: 'sales', path: '/w/sales' },
        { id: 'p-billing', name: 'billing', handle: 'billing', path: '/w/billing' },
      ],
    }),
}))

import { AgentHandoffAcceptFrom, typedSenderOk } from './HandoffAcceptFrom'
import { takeRequestedVaultTab, type AcceptFromEntry } from './handoff-api'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const OPS = { id: 'p-ops', name: 'ops', handle: 'ops', path: '/w/ops' } as unknown as ProjectWithWorkspaces

let list: AcceptFromEntry[] = []
let posts: { route: string; body: Record<string, unknown> }[] = []

beforeEach(() => {
  cleanup()
  posts = []
  setSection.mockReset()
  list = [{ workspaceId: 'p-ops', sender: 'ws:p-sales', addedBy: 'owner', addedAt: '2026-10-17T09:00:00Z' }]
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockImplementation(async (_s: unknown, route: string, params?: Record<string, unknown>) => {
    expect(params).toEqual({ workspace: 'p-ops' })
    if (route === 'vault/accept-from') return { entries: list }
    if (route === 'vault/incoming')
      return {
        items: [
          { deliveryId: 'hd_a', status: 'pending', name: 'K1' },
          { deliveryId: 'hd_b', status: 'pending', name: 'K2' },
          { deliveryId: 'hd_c', status: 'landed', name: 'K3' },
        ],
      }
    throw new Error(`unexpected GET ${route}`)
  })
  daemonCliPost.mockImplementation(async (_s: unknown, route: string, body: Record<string, unknown>) => {
    posts.push({ route, body: { ...body } })
    if (body.action === 'add') list = [...list, { workspaceId: 'p-ops', sender: String(body.sender), addedBy: 'owner', addedAt: 'now' }]
    else list = list.filter((e) => e.sender !== body.sender)
    return { ok: true, entries: list }
  })
})

describe('AgentHandoffAcceptFrom', () => {
  it('lists senders by name and the pending count, which opens Vault → Incoming', async () => {
    render(<AgentHandoffAcceptFrom project={OPS} />)
    const entry = await screen.findByTestId('handoff-accept-entry')
    expect(entry.textContent).toContain('sales (this server)')
    const review = await screen.findByTestId('handoff-pending-review')
    expect(review.textContent).toBe('Pending: 2 · Review')
    fireEvent.click(review)
    expect(setSection).toHaveBeenCalledWith('vault')
    expect(takeRequestedVaultTab()).toBe('incoming')
  })

  it('adds a local agent and a federated address through /cli/vault/accept-from, and removes', async () => {
    render(<AgentHandoffAcceptFrom project={OPS} />)
    await screen.findByTestId('handoff-accept-entry')
    // The picker offers other agents not already listed (not ops itself, not sales).
    const pick = screen.getByTestId('handoff-accept-pick') as HTMLSelectElement
    expect([...pick.options].map((o) => o.value)).toEqual(['', 'ws:p-billing'])
    fireEvent.change(pick, { target: { value: 'ws:p-billing' } })
    fireEvent.click(screen.getByTestId('handoff-accept-pick-add'))
    await waitFor(() => expect(screen.getAllByTestId('handoff-accept-entry')).toHaveLength(2))
    const typedAdd = screen.getByTestId('handoff-accept-typed-add') as HTMLButtonElement
    fireEvent.change(screen.getByTestId('handoff-accept-typed'), { target: { value: '*' } })
    expect(typedAdd.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('handoff-accept-typed'), { target: { value: 'ci::luzz.k2.dev' } })
    fireEvent.click(typedAdd)
    await waitFor(() => expect(screen.getAllByTestId('handoff-accept-entry')).toHaveLength(3))
    const sales = screen.getAllByTestId('handoff-accept-entry').find((e) => e.getAttribute('data-sender') === 'ws:p-sales')!
    fireEvent.click(within(sales).getByRole('button', { name: 'Stop accepting from ws:p-sales' }))
    await waitFor(() => expect(screen.getAllByTestId('handoff-accept-entry')).toHaveLength(2))
    expect(posts).toEqual([
      { route: 'vault/accept-from', body: { workspace: 'p-ops', sender: 'ws:p-billing', action: 'add' } },
      { route: 'vault/accept-from', body: { workspace: 'p-ops', sender: 'ci::luzz.k2.dev', action: 'add' } },
      { route: 'vault/accept-from', body: { workspace: 'p-ops', sender: 'ws:p-sales', action: 'remove' } },
    ])
    // Never the agent-access toggle route.
    expect(daemonCliPost.mock.calls.some((c) => c[1] === 'agent-access/set')).toBe(false)
  })

  it('read-only shows the list without edit controls', async () => {
    render(<AgentHandoffAcceptFrom project={OPS} readOnly />)
    await screen.findByTestId('handoff-accept-entry')
    expect(screen.queryByTestId('handoff-accept-pick')).toBeNull()
    expect(screen.queryByRole('button', { name: /Stop accepting/ })).toBeNull()
  })

  it('a refusal shows the daemon hint', async () => {
    daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'handoff_person_only', hint: 'only a person on this server edits who an agent accepts secrets from' } }))
    })
    render(<AgentHandoffAcceptFrom project={OPS} />)
    await screen.findByTestId('handoff-accept-entry')
    fireEvent.change(screen.getByTestId('handoff-accept-typed'), { target: { value: 'team:g1' } })
    fireEvent.click(screen.getByTestId('handoff-accept-typed-add'))
    expect((await screen.findByRole('alert')).textContent).toContain('only a person')
  })
})

describe('typedSenderOk', () => {
  it('takes agent::host, agent@host, team: and ws:, never a wildcard', () => {
    expect(typedSenderOk('sales::rosson.k2.dev')).toBe(true)
    expect(typedSenderOk('sales@rosson.k2.dev')).toBe(true)
    expect(typedSenderOk('team:g-1')).toBe(true)
    expect(typedSenderOk('ws:abc')).toBe(true)
    expect(typedSenderOk('*')).toBe(false)
    expect(typedSenderOk('*::host')).toBe(false)
    expect(typedSenderOk('sales')).toBe(false)
  })
})
