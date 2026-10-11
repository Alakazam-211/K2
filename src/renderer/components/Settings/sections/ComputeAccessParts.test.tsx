// @vitest-environment jsdom

// Settings → Compute, "Machines you can use" (Rosson 2026-10-11): the rows
// merge daemon nodes with the account's machines; toggles send one gesture
// each; one-click degrades to "needs a K2 Connect update" while the account
// side is missing. Which agents may use the machines is the Agent tab's
// "Allow compute nodes", not a section here.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return {
    ...orig,
    confirmComputeNode: vi.fn(),
    pauseComputeNode: vi.fn(),
    retireComputeNode: vi.fn(),
  }
})
vi.mock('./K2ConnectSection', () => ({ getAccountAccessToken: vi.fn(async () => 'acc-token') }))
vi.mock('../lib/compute-account', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../lib/compute-account')>()
  return { ...orig, listComputeMachines: vi.fn(), requestEnrollOffer: vi.fn() }
})

import * as api from './compute-api'
import type { ComputeNode, ComputeNodesList } from './compute-api'
import * as parts from './ComputeAccessParts'
import { MachinesPart } from './ComputeAccessParts'
import { getAccountAccessToken } from './K2ConnectSection'
import { listComputeMachines, ONE_CLICK_NEEDS_UPDATE, requestEnrollOffer, type ComputeMachine } from '../lib/compute-account'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useProjectsStore } from '@/stores/projects'

const m = vi.mocked(api)
const listMachines = vi.mocked(listComputeMachines)
const offer = vi.mocked(requestEnrollOffer)

const NODE: ComputeNode = {
  id: 'n-1',
  name: 'mini-1',
  fingerprint: 'aa'.repeat(32),
  os: 'macos',
  arch: 'aarch64',
  labels: {},
  state: 'active',
  enrolledVia: 'code',
  enrolledAt: 1,
  confirmedBy: 'owner',
  controllerPause: null,
  lastSeen: 1,
  lastRoute: 'relay',
  protocol: 1,
  nodeVersion: '0.1.0',
  routes: [],
  online: true,
  liveRoute: 'relay',
  queued: 0,
  running: 2,
  offer: {
    control: 'active',
    available: true,
    unavailable: [],
    caps: { cpu_millis: 10_000, mem_bytes: 32 * 2 ** 30, disk_budget_bytes: 100 * 2 ** 30, hard: true },
    free: { cpu_millis: 10_000, mem_bytes: 16 * 2 ** 30, disk_free_bytes: 200 * 2 ** 30 },
    parallelMax: 2,
    tools: {},
  },
}

function machine(over: Partial<ComputeMachine> = {}): ComputeMachine {
  return {
    sub: 'z13',
    node_fp: 'bb'.repeat(32),
    node_name: 'z13',
    node_state: 'unenrolled',
    controller: null,
    control: 'active',
    sas: null,
    os: 'linux',
    arch: 'x86_64',
    cpu_cores: 16,
    mem_bytes: 64 * 2 ** 30,
    disk_total_bytes: 2000 * 2 ** 30,
    disk_free_bytes: 900 * 2 ** 30,
    node_version: '0.45.4',
    k2_version: '0.45.4',
    updated_at: new Date().toISOString(),
    ...over,
  }
}

function list(nodes: ComputeNode[], ticketSupported = true): ComputeNodesList {
  return { nodes, waitingCodes: [], controller: 'rosson.k2.dev', ticketSupported }
}

let onChanged: ReturnType<typeof vi.fn>

beforeEach(() => {
  cleanup()
  vi.clearAllMocks()
  onChanged = vi.fn(async () => {})
  vi.mocked(getAccountAccessToken).mockResolvedValue('acc-token')
  listMachines.mockResolvedValue({ ok: true, machines: [] })
  useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) } as never)
  useProjectsStore.setState({ projects: [{ id: 'p-sales', name: 'sales', path: '/ws/sales', workspaces: [], agentsCanUseCompute: 0 }] } as never)
  m.retireComputeNode.mockResolvedValue({ ok: true, state: 'draining', running: 2 })
  m.pauseComputeNode.mockResolvedValue({ ok: true, controllerPause: null })
  m.confirmComputeNode.mockResolvedValue({ ok: true, state: 'active' })
})

afterEach(() => cleanup())

describe('Machines you can use', () => {
  it('capability missing: says one-click needs a K2 Connect update and still lists enrolled machines', async () => {
    listMachines.mockResolvedValue({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
    render(<MachinesPart list={list([NODE])} onChanged={onChanged} />)
    expect(await screen.findByTestId('compute-oneclick-missing')).toHaveProperty(
      'textContent',
      'One-click needs a K2 Connect update; meanwhile add a machine with an enroll code',
    )
    const row = screen.getByTestId('compute-machine-mini-1')
    const sw = within(row).getByRole('switch', { name: 'Use compute from mini-1' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(within(row).getByTestId('compute-machine-hw').textContent).toBe('10 cores · 32 GB RAM · 200 GB free')
    expect(row.textContent).toContain('online')
    expect(listMachines).toHaveBeenCalledWith('acc-token')
  })

  it('a controller without a ticket key shows the same notice and keeps off machines off', async () => {
    listMachines.mockResolvedValue({ ok: true, machines: [machine()] })
    render(<MachinesPart list={list([], false)} onChanged={onChanged} />)
    await screen.findByTestId('compute-oneclick-missing')
    const sw = within(screen.getByTestId('compute-machine-z13')).getByRole('switch') as HTMLButtonElement
    expect(sw.disabled).toBe(true)
  })

  it('switching an account machine on asks Connect to deliver a ticket for this server', async () => {
    listMachines.mockResolvedValue({ ok: true, machines: [machine()] })
    offer.mockResolvedValue({ ok: true, ticket: 't' })
    render(<MachinesPart list={list([NODE])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-z13')
    expect(within(row).getByTestId('compute-machine-hw').textContent).toBe('16 cores · 64 GB RAM · 900 GB free')
    const sw = within(row).getByRole('switch', { name: 'Use compute from z13' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    await waitFor(() =>
      expect(offer).toHaveBeenCalledWith('acc-token', { controllerSub: 'rosson', nodeSub: 'z13', nodeFp: 'bb'.repeat(32), nodeName: 'z13' }),
    )
    await waitFor(() => expect(within(screen.getByTestId('compute-machine-z13')).getByTestId('compute-machine-status').textContent).toBe('asked to join…'))
    expect(screen.queryByTestId('compute-oneclick-missing')).toBeNull()
  })

  it('a ticket route without delivery flips the page to the needs-update notice', async () => {
    listMachines.mockResolvedValue({ ok: true, machines: [machine()] })
    offer.mockResolvedValue({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
    render(<MachinesPart list={list([])} onChanged={onChanged} />)
    fireEvent.click(within(await screen.findByTestId('compute-machine-z13')).getByRole('switch'))
    await screen.findByTestId('compute-oneclick-missing')
  })

  it('a machine used by another server is shown but cannot be switched on', async () => {
    listMachines.mockResolvedValue({ ok: true, machines: [machine({ node_state: 'online', controller: 'other.k2.dev' })] })
    render(<MachinesPart list={list([])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-z13')
    expect(within(row).getByTestId('compute-machine-status').textContent).toBe('used by other.k2.dev')
    expect((within(row).getByRole('switch') as HTMLButtonElement).disabled).toBe(true)
  })

  it('an offline machine cannot be switched on', async () => {
    listMachines.mockResolvedValue({ ok: true, machines: [machine({ updated_at: '2020-01-01T00:00:00Z' })] })
    render(<MachinesPart list={list([])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-z13')
    expect(row.textContent).toContain('offline')
    expect((within(row).getByRole('switch') as HTMLButtonElement).disabled).toBe(true)
  })

  it('switching an enrolled machine off asks, then retires it (drain, then remove)', async () => {
    render(<MachinesPart list={list([NODE])} onChanged={onChanged} />)
    fireEvent.click(within(await screen.findByTestId('compute-machine-mini-1')).getByRole('switch'))
    await waitFor(() => expect(m.retireComputeNode).toHaveBeenCalledWith('mini-1'))
    expect(useConfirmDialogStore.getState().confirm).toHaveBeenCalled()
    await screen.findByText("mini-1 finishes its jobs, then it's removed.")
    expect(onChanged).toHaveBeenCalled()
  })

  it('a retiring machine shows off; switching it back on resumes it', async () => {
    render(<MachinesPart list={list([{ ...NODE, retiring: true, controllerPause: 'draining' }])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-mini-1')
    expect(within(row).getByTestId('compute-machine-status').textContent).toBe('finishing jobs, then removed')
    const sw = within(row).getByRole('switch')
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    await waitFor(() => expect(m.pauseComputeNode).toHaveBeenCalledWith('mini-1', 'resume'))
  })

  it('a pending machine whose reported code matches is confirmed with one click', async () => {
    const pending = { ...NODE, fingerprint: 'bb'.repeat(32), name: 'z13', state: 'pending' as const, sas: '123 456', offer: undefined }
    listMachines.mockResolvedValue({ ok: true, machines: [machine({ node_state: 'pending', controller: 'rosson.k2.dev', sas: '123456' })] })
    render(<MachinesPart list={list([pending])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-z13')
    fireEvent.click(await within(row).findByRole('button', { name: 'Codes match: confirm' }))
    await waitFor(() => expect(m.confirmComputeNode).toHaveBeenCalledWith('z13', '123456'))
  })

  it('a pending machine reporting a different code is not offered a confirm', async () => {
    const pending = { ...NODE, fingerprint: 'bb'.repeat(32), name: 'z13', state: 'pending' as const, sas: '123 456', offer: undefined }
    listMachines.mockResolvedValue({ ok: true, machines: [machine({ node_state: 'pending', controller: 'rosson.k2.dev', sas: '999999' })] })
    render(<MachinesPart list={list([pending])} onChanged={onChanged} />)
    const row = await screen.findByTestId('compute-machine-z13')
    await waitFor(() => expect(row.textContent).toContain("The codes differ: don't confirm"))
    expect(within(row).queryByRole('button', { name: 'Codes match: confirm' })).toBeNull()
  })

  it('signed out: asks to sign in and still lists machines added with a code', async () => {
    vi.mocked(getAccountAccessToken).mockResolvedValue(null)
    render(<MachinesPart list={list([NODE])} onChanged={onChanged} />)
    await screen.findByText(/Sign in to your k2.dev account/)
    expect(screen.getByTestId('compute-machine-mini-1')).toBeTruthy()
    expect(listMachines).not.toHaveBeenCalled()
  })
})

describe('agent access is not set on this page (Rosson 2026-10-11)', () => {
  it('has no per-agent section; the machines part points at the Agent tab', async () => {
    expect(Object.keys(parts)).toEqual(['MachinesPart'])
    vi.mocked(getAccountAccessToken).mockResolvedValue(null)
    render(<MachinesPart list={list([NODE])} onChanged={onChanged} />)
    expect(screen.getByTestId('compute-agents-note').textContent).toContain(
      'Which agents can use these machines is set per agent: Settings → Workspaces → <agent> → Agent → Allow',
    )
    expect(screen.queryByText('Agents that can use compute')).toBeNull()
  })
})
