// @vitest-environment jsdom

// Settings → Compute, package requests (prd-compute-self-serve-environments-v1
// §6, §12.3): pending rows group into one card per batch; another server's
// ask says the install is host-wide; Allow once / Allow and add / Deny send
// the batch id to the decide route; a dry-run refusal shows as a warning;
// recent decisions sit below, collapsed. Daemon truth is mocked.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('./compute-packages-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-packages-api')>()
  return {
    ...orig,
    fetchPkgRequests: vi.fn(),
    fetchNodePackages: vi.fn(),
    decidePkgRequest: vi.fn(),
    onComputeSync: vi.fn(() => () => {}),
  }
})

import * as api from './compute-packages-api'
import type { PkgRequestRow } from './compute-packages-api'
import type { ComputeNode, LocalStatus } from './compute-api'
import { HOST_WIDE_NOTE, LocalPackagesSummary, NodePackagesSummary, PackageRequestsPanel } from './ComputePackagesParts'

const m = vi.mocked(api)
const now = Math.floor(Date.now() / 1000)

function row(over: Partial<PkgRequestRow> = {}): PkgRequestRow {
  return {
    id: 'pr_1',
    batchId: 'pb_1',
    origin: 'own',
    node: 'k2n7',
    nodeRequest: null,
    server: 'own',
    workspaceId: 'ws-tests',
    jobId: 'job-1',
    askedBy: 'tests',
    why: 'the Tauri build needs GTK',
    names: ['libgtk-3-dev'],
    dry: { installs: ['libgtk-3-dev', 'libgtk-3-0', 'libatk1.0-dev'], size_mb: 120 },
    host: 'Ubuntu 24.04.1 LTS · apt',
    state: 'pending',
    addToList: false,
    decidedBy: null,
    note: null,
    createdAt: now - 60,
    expiresAt: now + 86000,
    ...over,
  }
}

function node(over: Partial<ComputeNode> = {}): ComputeNode {
  return {
    id: 'n-7',
    name: 'k2n7',
    fingerprint: 'fp-7',
    os: 'linux',
    arch: 'x86_64',
    labels: {},
    state: 'active',
    enrolledVia: 'code',
    enrolledAt: 1,
    confirmedBy: 'owner',
    controllerPause: null,
    lastSeen: now,
    lastRoute: null,
    protocol: 7,
    nodeVersion: '0.46.9',
    routes: [],
    online: true,
    liveRoute: null,
    queued: 0,
    running: 0,
    ...over,
  }
}

beforeEach(() => {
  cleanup()
  vi.clearAllMocks()
  m.onComputeSync.mockImplementation(() => () => {})
})

afterEach(() => {
  cleanup()
})

describe('groupPkgBatches', () => {
  it('one batch per batchId, names once each, every node with its host line, dependencies and size summed', () => {
    const batches = api.groupPkgBatches([
      row(),
      row({ id: 'pr_2', node: 'k2n8', host: 'Debian 13 · apt', names: ['libgtk-3-dev', 'libwebkit2gtk-4.1-dev'], dry: { installs: ['a', 'b'], size_mb: 60 } }),
      row({ id: 'pr_3', batchId: 'pb_2', names: ['cmake'], dry: null }),
    ])
    expect(batches.map((b) => b.batchId)).toEqual(['pb_1', 'pb_2'])
    const [first, second] = batches
    expect(first.names).toEqual(['libgtk-3-dev', 'libwebkit2gtk-4.1-dev'])
    expect(first.nodes).toEqual([
      { node: 'k2n7', host: 'Ubuntu 24.04.1 LTS · apt' },
      { node: 'k2n8', host: 'Debian 13 · apt' },
    ])
    expect(first.deps).toBe(5)
    expect(first.sizeMb).toBe(180)
    expect(first.who).toBe('agent tests')
    expect(first.otherServer).toBe(false)
    expect(api.pkgDepsLine(first)).toBe('5 packages with dependencies, 180 MB')
    expect(second.deps).toBe(0)
    expect(api.pkgDepsLine(second)).toBe('')
  })

  it("another server's ask names that server and marks the batch host-wide", () => {
    const [b] = api.groupPkgBatches([row({ server: 'acme.k2.dev', origin: 'fwd' })])
    expect(b.who).toBe("acme.k2.dev's tests")
    expect(b.otherServer).toBe(true)
  })
})

describe('findLocalNode', () => {
  const status = (n: Partial<NonNullable<LocalStatus['node']>>): LocalStatus => ({
    installed: true,
    home: '/var/k2node',
    configDir: '/etc/k2-node',
    node: { state: 'online', ...n },
    control: null,
    policy: null,
  })

  it('matches by the node key fingerprint first, then the same name under the same controller', () => {
    const nodes = [node(), node({ id: 'n-8', name: 'mini-1', fingerprint: 'fp-mini' })]
    expect(api.findLocalNode(status({ node_fp: 'fp-mini', name: 'other' }), nodes, 'rosson.k2.dev')?.name).toBe('mini-1')
    expect(
      api.findLocalNode(status({ name: 'k2n7', controller: 'https://rosson.k2.dev' }), nodes, 'rosson.k2.dev')?.name,
    ).toBe('k2n7')
    expect(api.findLocalNode(status({ name: 'k2n7', controller: 'https://acme.k2.dev' }), nodes, 'rosson.k2.dev')).toBeNull()
    expect(api.findLocalNode(null, nodes, 'rosson.k2.dev')).toBeNull()
  })
})

describe('PackageRequestsPanel', () => {
  function serve(pending: PkgRequestRow[], recent: PkgRequestRow[] = [], canDecide = true): void {
    m.fetchPkgRequests.mockImplementation(async (state) =>
      state === 'pending' ? { requests: pending, canDecide } : { requests: [...pending, ...recent], canDecide },
    )
  }

  it('one card per batch with what, where, for whom, why and the dry-run size', async () => {
    serve([row(), row({ id: 'pr_2', node: 'k2n8', host: 'Debian 13 · apt' }), row({ id: 'pr_3', batchId: 'pb_2', names: ['cmake'] })])
    render(<PackageRequestsPanel />)
    const card = await screen.findByTestId('compute-pkg-batch-pb_1')
    expect(screen.getByTestId('compute-pkg-batch-pb_2')).toBeTruthy()
    expect(screen.getByTestId('compute-pkg-requests').textContent).toContain('Package requests (2)')
    expect(card.textContent).toContain('libgtk-3-dev')
    const nodes = within(card).getAllByTestId('compute-pkg-batch-node').map((n) => n.textContent)
    expect(nodes).toEqual(['on k2n7 (Ubuntu 24.04.1 LTS · apt)', 'on k2n8 (Debian 13 · apt)'])
    expect(card.textContent).toContain('for agent tests')
    expect(card.textContent).toContain('Why: the Tauri build needs GTK')
    expect(within(card).getByTestId('compute-pkg-batch-deps').textContent).toBe('6 packages with dependencies, 240 MB')
    // Our own agent: no host-wide line.
    expect(within(card).queryByTestId('compute-pkg-batch-hostwide')).toBeNull()
  })

  it("another server's ask carries the 'installed for every job' line", async () => {
    serve([row({ server: 'acme.k2.dev', origin: 'fwd' })])
    render(<PackageRequestsPanel />)
    const card = await screen.findByTestId('compute-pkg-batch-pb_1')
    expect(card.textContent).toContain("for acme.k2.dev's tests")
    expect(within(card).getByTestId('compute-pkg-batch-hostwide').textContent).toBe(HOST_WIDE_NOTE)
    expect(HOST_WIDE_NOTE).toBe('This is installed for every job on that machine.')
  })

  it('shows a dry-run refusal as a warning', async () => {
    serve([row({ dry: { installs: ['sudo'], refusal: 'package_denied: sudo' } })])
    render(<PackageRequestsPanel />)
    const card = await screen.findByTestId('compute-pkg-batch-pb_1')
    expect(card.textContent).toContain('The dry run would refuse this: package_denied: sudo')
  })

  it('Allow once sends approve without add, for the batch', async () => {
    serve([row()])
    m.decidePkgRequest.mockResolvedValue({ request: 'pb_1', rows: [{ request: 'pr_1', node: 'k2n7', state: 'approved', note: 'on its way to the node' }] })
    render(<PackageRequestsPanel />)
    fireEvent.click(await screen.findByTestId('compute-pkg-allow-pb_1'))
    await waitFor(() => expect(m.decidePkgRequest).toHaveBeenCalledWith('pb_1', true, { add: false, note: undefined }))
    expect(await screen.findByText('on its way to the node')).toBeTruthy()
  })

  it('Allow and add sends add: true', async () => {
    serve([row()])
    m.decidePkgRequest.mockResolvedValue({ request: 'pb_1', rows: [] })
    render(<PackageRequestsPanel />)
    fireEvent.click(await screen.findByTestId('compute-pkg-allow-add-pb_1'))
    await waitFor(() => expect(m.decidePkgRequest).toHaveBeenCalledWith('pb_1', true, { add: true, note: undefined }))
  })

  it('Deny asks for an optional note, then sends approve: false with it', async () => {
    serve([row()])
    m.decidePkgRequest.mockResolvedValue({ request: 'pb_1', rows: [{ request: 'pr_1', node: 'k2n7', state: 'denied' }] })
    render(<PackageRequestsPanel />)
    fireEvent.click(await screen.findByTestId('compute-pkg-deny-pb_1'))
    expect(m.decidePkgRequest).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText('Note for the agent'), { target: { value: 'use the container image' } })
    fireEvent.click(screen.getByTestId('compute-pkg-deny-confirm-pb_1'))
    await waitFor(() =>
      expect(m.decidePkgRequest).toHaveBeenCalledWith('pb_1', false, { add: false, note: 'use the container image' }),
    )
  })

  it('a member who cannot decide sees no buttons', async () => {
    serve([row()], [], false)
    render(<PackageRequestsPanel />)
    const card = await screen.findByTestId('compute-pkg-batch-pb_1')
    expect(within(card).queryByRole('button')).toBeNull()
    expect(screen.getByTestId('compute-pkg-requests').textContent).toContain("Only this server's owner or an Admin decides these.")
  })

  it('recent decisions sit below, collapsed, with state and note', async () => {
    serve([], [row({ id: 'pr_9', batchId: 'pb_9', state: 'denied', note: 'not on this box', decidedBy: 'owner' })])
    render(<PackageRequestsPanel />)
    const recent = await screen.findByTestId('compute-pkg-recent')
    expect((recent as HTMLDetailsElement).open).toBe(false)
    const line = within(recent).getByTestId('compute-pkg-recent-pr_9')
    expect(line.textContent).toContain('denied')
    expect(line.textContent).toContain('not on this box')
    expect(screen.getByTestId('compute-pkg-requests').textContent).toContain('Nothing is waiting.')
  })

  it('re-reads on the daemon pkg_request signal', async () => {
    let fire: ((kind: string) => void) | null = null
    m.onComputeSync.mockImplementation((fn) => {
      fire = fn
      return () => {}
    })
    serve([])
    render(<PackageRequestsPanel />)
    await screen.findByTestId('compute-pkg-requests')
    await waitFor(() => expect(m.fetchPkgRequests).toHaveBeenCalledTimes(2))
    serve([row()])
    expect(fire).not.toBeNull()
    fire!('pkg_request')
    expect(await screen.findByTestId('compute-pkg-batch-pb_1')).toBeTruthy()
  })

  it('an older daemon without the route renders nothing', async () => {
    m.fetchPkgRequests.mockRejectedValue(new Error(JSON.stringify({ error: { code: 'not_found', hint: 'no route' } })))
    const { container } = render(<PackageRequestsPanel />)
    await waitFor(() => expect(m.fetchPkgRequests).toHaveBeenCalled())
    await waitFor(() => expect(container.innerHTML).toBe(''))
  })
})

describe('NodePackagesSummary', () => {
  it('host line, manager, policy, open requests and the reboot badge', () => {
    render(
      <NodePackagesSummary
        node={node({
          host: { line: 'Ubuntu 24.04.1 LTS · apt · x86_64', rebootRequired: true },
          rebootPending: true,
          packages: { manager: 'apt', policy: 'official, ≤ 2048 MB per request', openRequests: 2 },
        })}
      />,
    )
    const el = screen.getByTestId('compute-node-packages-k2n7')
    expect(screen.getByTestId('compute-node-host').textContent).toBe('Ubuntu 24.04.1 LTS · apt · x86_64')
    expect(screen.getByTestId('compute-node-reboot').textContent).toBe('reboot pending')
    expect(el.textContent).toContain('Packages: apt')
    expect(el.textContent).toContain('policy official, ≤ 2048 MB per request')
    expect(screen.getByTestId('compute-node-open-requests').textContent).toBe('2 open requests')
  })

  it('an older node with none of it renders nothing', () => {
    const { container } = render(<NodePackagesSummary node={node()} />)
    expect(container.innerHTML).toBe('')
  })
})

describe('LocalPackagesSummary', () => {
  it("this machine's policy and its open requests", async () => {
    m.fetchNodePackages.mockResolvedValue({
      node: 'k2n7',
      host: null,
      hostLine: null,
      packages: {
        manager: 'apt',
        level: 'profiles',
        profiles: ['tauri-linux'],
        maxInstallMb: 2048,
        policy: 'profiles (tauri-linux), ≤ 2048 MB per request',
        approved: [],
        installed: [],
        indexAgeSecs: null,
        openRequests: 1,
      },
      takesRequests: true,
      owner: 'this server',
      requests: [row(), row({ id: 'pr_old', state: 'installed' })],
      note: null,
    })
    render(<LocalPackagesSummary node={node()} />)
    const el = await screen.findByTestId('compute-local-pkg-pr_1')
    expect(el.textContent).toContain('libgtk-3-dev for agent tests')
    const part = screen.getByTestId('compute-local-packages')
    expect(part.textContent).toContain('Package policy for k2n7: profiles (tauri-linux), ≤ 2048 MB per request')
    expect(part.textContent).toContain('1 open package request')
    expect(screen.queryByTestId('compute-local-pkg-pr_old')).toBeNull()
    expect(m.fetchNodePackages).toHaveBeenCalledWith('k2n7')
  })
})
