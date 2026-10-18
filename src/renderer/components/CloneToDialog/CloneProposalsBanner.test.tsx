// @vitest-environment jsdom
// 0.46.1: an agent's open clone proposals show on top of its Thread for the
// Owner and Admins. "Review clone…" opens the Clone dialog prefilled from
// the proposal (startCloneProposal with the proposal's target host);
// "Deny" posts the denial. Members, non-workspace addresses and a server
// with nothing open show nothing. The dialog then says who proposed it and
// its button reads "Approve and clone" (CloneToDialog.options.test.tsx).
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, waitFor, cleanup, fireEvent, act } from '@testing-library/react'

const h = vi.hoisted(() => ({
  whoami: { owner: true, role: 'owner' } as Record<string, unknown>,
  proposals: [] as unknown[],
  gets: [] as Array<{ route: string; params: unknown }>,
  posts: [] as Array<{ route: string; body: unknown }>,
  started: [] as unknown[][],
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(
      vi.fn(async (route: string, params?: unknown) => {
        h.gets.push({ route, params })
        if (route === 'auth/whoami') return h.whoami
        if (route === 'clone-proposal/list') return { ok: true, proposals: h.proposals }
        throw new Error(`unexpected GET ${route}`)
      }),
    ),
    daemonCliPost: primaryOnly(
      vi.fn(async (route: string, body?: unknown) => {
        h.posts.push({ route, body })
        if (route === 'clone-proposal/deny') {
          h.proposals = []
          return { ok: true }
        }
        throw new Error(`unexpected POST ${route}`)
      }),
    ),
  }
})

vi.mock('@/lib/start-clone-to', () => ({
  startCloneProposal: (...args: unknown[]) => {
    h.started.push(args)
  },
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))

import { CloneProposalsBanner, hostForTarget, proposalWorkspaceOf } from './CloneProposalsBanner'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

const LUZZ = { id: 'h1', label: 'Luzz', hostname: 'luzz.k2.dev', port: 443, secure: true } as ConnectHost

function row(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'cp_1',
    projectId: 'p1',
    proposedBy: 'agent:sales/s1',
    target: 'luzz.k2.dev',
    options: { threads: true, secrets: { STRIPE_KEY: 'copy' }, allHistory: true },
    why: 'move to the new box',
    status: 'open',
    expiresAt: 9e9,
    workspaceHandle: 'sales',
    workspacePath: '/w/sales',
    summary: 'clone sales to luzz.k2.dev, Thread all, 1 secret copied',
    ...over,
  }
}

beforeEach(() => {
  h.whoami = { owner: true, role: 'owner' }
  h.proposals = [row()]
  h.gets.length = 0
  h.posts.length = 0
  h.started.length = 0
  useConnectHostStore.setState({ hosts: [LUZZ] })
})
afterEach(() => cleanup())

describe('proposal helpers', () => {
  it('a workspace address only; hosts match by name or name:port', () => {
    expect(proposalWorkspaceOf('sales')).toBe('sales')
    expect(proposalWorkspaceOf('sales/reviewer')).toBe('sales')
    expect(proposalWorkspaceOf('import:abc')).toBeNull()
    expect(proposalWorkspaceOf('gb:bot')).toBeNull()
    expect(hostForTarget([LUZZ], 'LUZZ.k2.dev')).toBe(LUZZ)
    expect(hostForTarget([LUZZ], 'luzz.k2.dev:443')).toBe(LUZZ)
    expect(hostForTarget([LUZZ], 'here')).toBeNull()
    expect(hostForTarget([LUZZ], 'other.k2.dev')).toBeNull()
  })
})

describe('Clone proposals on the Thread', () => {
  it('the owner sees the open proposal; Review opens the dialog prefilled for the target host', async () => {
    renderInPrimaryRoom(<CloneProposalsBanner addr="sales/reviewer" />)
    await waitFor(() => expect(screen.getByTestId('clone-proposal-cp_1')).toBeTruthy())
    expect(h.gets).toContainEqual({ route: 'clone-proposal/list', params: { status: 'open', workspace: 'sales' } })
    expect(screen.getByText('sales proposes a clone to luzz.k2.dev')).toBeTruthy()
    expect(screen.getByText(/move to the new box · clone sales to luzz\.k2\.dev/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Review clone…' }))
    await waitFor(() => expect(h.started).toHaveLength(1))
    const [p, path, name, host] = h.started[0] as [{ id: string }, string, string, ConnectHost]
    expect([p.id, path, name, host]).toEqual(['cp_1', '/w/sales', 'sales', LUZZ])
  })

  it('Deny posts the denial and the proposal goes away', async () => {
    renderInPrimaryRoom(<CloneProposalsBanner addr="sales" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'clone-proposal/deny', body: { proposal: 'cp_1' } }]))
    await waitFor(() => expect(screen.queryByTestId('clone-proposals-banner')).toBeNull())
  })

  it('a target this window has no host for (or "here") shows the k2 clone approve hint, no Review', async () => {
    h.proposals = [row({ target: 'here' })]
    renderInPrimaryRoom(<CloneProposalsBanner addr="sales" />)
    expect((await screen.findByTestId('clone-proposal-cli-hint')).textContent).toBe('Run it from a terminal: k2 clone approve cp_1')
    expect(screen.queryByRole('button', { name: 'Review clone…' })).toBeNull()
  })

  it('a Member sees nothing (and never lists); an import address asks nothing', async () => {
    h.whoami = { owner: false, role: 'member' }
    renderInPrimaryRoom(<CloneProposalsBanner addr="sales" />)
    await waitFor(() => expect(h.gets.map((g) => g.route)).toEqual(['auth/whoami']))
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByTestId('clone-proposals-banner')).toBeNull()
    expect(h.gets.some((g) => g.route === 'clone-proposal/list')).toBe(false)
    cleanup()
    h.gets.length = 0
    renderInPrimaryRoom(<CloneProposalsBanner addr="import:abc" />)
    await act(async () => {
      await Promise.resolve()
    })
    expect(h.gets).toEqual([])
  })
})
