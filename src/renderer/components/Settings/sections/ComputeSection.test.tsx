// @vitest-environment jsdom

// Settings → Compute: the page renders daemon truth (mocked compute-api) and
// sends the owner's gestures. Controller part on the focused server; "This
// computer" part on the local daemon (desktop only).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const features = vi.hoisted(() => ({ computeLocal: true }) as Record<string, unknown>)
vi.mock('@/web/features', async (importOriginal) => {
  const orig = await importOriginal<typeof import('@/web/features')>()
  Object.assign(features, orig.webFeatures, { computeLocal: features.computeLocal })
  return { ...orig, webFeatures: features }
})

vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return {
    ...orig,
    fetchComputeNodes: vi.fn(),
    fetchComputeGrants: vi.fn(),
    fetchComputeJobs: vi.fn(),
    fetchComputeEvents: vi.fn(),
    confirmComputeNode: vi.fn(),
    pauseComputeNode: vi.fn(),
    renameComputeNode: vi.fn(),
    removeComputeNode: vi.fn(),
    mintEnrollCode: vi.fn(),
    setComputeGrant: vi.fn(),
    revokeComputeGrant: vi.fn(),
    setWorkspaceComputeSwitch: vi.fn(),
    fetchLocalStatus: vi.fn(),
    pauseLocalNode: vi.fn(),
    drainLocalNode: vi.fn(),
    resumeLocalNode: vi.fn(),
    setLocalPolicy: vi.fn(),
    enableLocalNode: vi.fn(),
    disableLocalNode: vi.fn(),
    setLocalRemoteAdmin: vi.fn(),
    retireComputeNode: vi.fn(),
  }
})

vi.mock('./K2ConnectSection', () => ({ getAccountAccessToken: vi.fn(async () => null) }))
// Node admin parts read their own routes; this page's tests don't cover them
// (ComputeAdminParts.test.tsx does).
vi.mock('./compute-admin-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-admin-api')>()
  return { ...orig, fetchAdminRequests: vi.fn(async () => ({ requests: [], canApprove: true })), fetchNodeHealth: vi.fn() }
})
vi.mock('../lib/compute-account', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../lib/compute-account')>()
  return {
    ...orig,
    listAccountServers: vi.fn(async () => []),
    requestEnrollTicket: vi.fn(),
    listComputeMachines: vi.fn(async () => ({ ok: true, machines: [] })),
    requestEnrollOffer: vi.fn(),
  }
})

import * as api from './compute-api'
import type { ComputeGrant, ComputeNode, LocalStatus } from './compute-api'
import { COMPUTE_MANIFEST, ComputeSection } from './ComputeSection'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { getAccountAccessToken } from './K2ConnectSection'
import { listAccountServers, requestEnrollTicket } from '../lib/compute-account'

const m = vi.mocked(api)

const DISABLED = new Error(JSON.stringify({ error: { code: 'compute_disabled', hint: 'compute nodes are a preview' } }))

const PENDING: ComputeNode = {
  id: 'n-pend',
  name: 'mini-1',
  fingerprint: 'fp1',
  os: 'linux',
  arch: 'x86_64',
  labels: {},
  state: 'pending',
  enrolledVia: 'code',
  enrolledAt: 1,
  confirmedBy: null,
  controllerPause: null,
  lastSeen: null,
  lastRoute: null,
  protocol: 1,
  nodeVersion: '0.1.0',
  routes: [],
  online: true,
  liveRoute: 'relay',
  queued: 0,
  running: 0,
  sas: '123 456',
}

const ONLINE: ComputeNode = {
  ...PENDING,
  id: 'n-on',
  name: 'mini-2',
  state: 'active',
  sas: undefined,
  confirmedBy: 'owner',
  running: 1,
  queued: 2,
  lastSeen: Math.floor(Date.now() / 1000),
  offer: {
    control: 'active',
    available: true,
    unavailable: [],
    caps: { cpu_millis: 8000, mem_bytes: 16 * 2 ** 30, disk_budget_bytes: 100 * 2 ** 30, hard: true },
    free: { cpu_millis: 4000, mem_bytes: 8 * 2 ** 30, disk_free_bytes: 50 * 2 ** 30 },
    parallelMax: 2,
    tools: { cargo: '1.95.0', node: '22.1.0' },
  },
}

const GRANT: ComputeGrant = {
  nodeId: 'n-on',
  workspaceId: 'p-sales',
  scopes: 'run',
  maxJobSecs: 3600,
  maxDiskGb: 20,
  maxParallel: 1,
  maxQueued: 10,
  warmAt: null,
  expiresAt: null,
  grantedBy: 'owner',
  grantedAt: 1,
  node: 'mini-2',
  workspace: 'sales',
}

const LOCAL_OFF: LocalStatus = {
  installed: false,
  home: '/var/k2node',
  configDir: '/etc/k2-node',
  node: null,
  control: null,
  policy: null,
  platform: 'macos',
  homeCheck: { home: '/Users/r', mode: '755', private: false, readable: ['/Users/r/Documents', '/Users/r/.ssh'] },
}

let setComputePreview: ReturnType<typeof vi.fn>

function project(id: string, name: string, extra: Record<string, unknown> = {}): unknown {
  return { id, name, path: `/ws/${name}`, workspaces: [], ...extra }
}

beforeEach(() => {
  cleanup()
  vi.clearAllMocks()
  features.computeLocal = true
  setComputePreview = vi.fn(async () => {})
  useSettingsStore.setState({ computePreview: true, setComputePreview } as never)
  useProjectsStore.setState({
    projects: [project('p-sales', 'sales', { agentsCanUseCompute: 0 }), project('p-ops', 'ops')],
  } as never)
  useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) } as never)
  m.fetchComputeNodes.mockResolvedValue({ nodes: [PENDING, ONLINE], waitingCodes: [], controller: 'rosson.k2.dev' })
  m.fetchComputeGrants.mockResolvedValue([GRANT])
  m.fetchComputeJobs.mockResolvedValue([])
  m.fetchComputeEvents.mockResolvedValue([])
  m.fetchLocalStatus.mockResolvedValue(LOCAL_OFF)
  for (const fn of [
    m.confirmComputeNode,
    m.pauseComputeNode,
    m.removeComputeNode,
    m.revokeComputeGrant,
    m.setComputeGrant,
    m.pauseLocalNode,
    m.setLocalPolicy,
  ]) {
    fn.mockResolvedValue({ ok: true } as never)
  }
  m.setWorkspaceComputeSwitch.mockResolvedValue({ ok: true, agentsCanUseCompute: true })
  vi.mocked(getAccountAccessToken).mockResolvedValue(null)
  vi.mocked(listAccountServers).mockResolvedValue([])
  vi.mocked(requestEnrollTicket).mockReset()
})

afterEach(() => {
  cleanup()
})

describe('agents are not on this page (Rosson 2026-10-11)', () => {
  it('no "Agents that can use compute" section, no per-agent switches; a note points at the Agent tab', async () => {
    render(<ComputeSection />)
    await screen.findByTestId('compute-machines')
    expect(screen.queryByText('Agents that can use compute')).toBeNull()
    expect(screen.queryByTestId('compute-agents')).toBeNull()
    expect(screen.queryByRole('switch', { name: /^Use compute:/ })).toBeNull()
    expect(screen.getByTestId('compute-agents-note').textContent).toContain('Agent → Allow')
    expect(m.setWorkspaceComputeSwitch).not.toHaveBeenCalled()
    expect(COMPUTE_MANIFEST.map((e) => e.id)).not.toContain('compute.agents')
  })
})

describe('preview off', () => {
  it('shows the switch and one paragraph, nothing else for the controller', async () => {
    useSettingsStore.setState({ computePreview: false } as never)
    m.fetchComputeNodes.mockRejectedValue(DISABLED)
    render(<ComputeSection />)
    const sw = await screen.findByRole('switch', { name: 'Compute nodes (preview)' })
    await waitFor(() => expect(m.fetchComputeNodes).toHaveBeenCalled())
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('false'))
    expect(screen.queryByText('Add a node')).toBeNull()
    expect(screen.queryByTestId('compute-node-mini-2')).toBeNull()
    expect(m.fetchComputeGrants).not.toHaveBeenCalled()
    fireEvent.click(sw)
    await waitFor(() => expect(setComputePreview).toHaveBeenCalledWith(true))
  })
})

describe('nodes of this server', () => {
  it('a pending node: the owner types the node code to confirm', async () => {
    render(<ComputeSection />)
    const card = await screen.findByTestId('compute-node-mini-1')
    expect(within(card).getByTestId('node-dot').textContent).toBe('pending')
    expect(card.textContent).toContain('123 456')
    const confirm = within(card).getByRole('button', { name: 'Confirm' }) as HTMLButtonElement
    expect(confirm.disabled).toBe(true)
    const field = within(card).getByLabelText('Code shown on mini-1') as HTMLInputElement
    expect(field.value).toBe('')
    fireEvent.change(field, { target: { value: '123456' } })
    fireEvent.click(confirm)
    await waitFor(() => expect(m.confirmComputeNode).toHaveBeenCalledWith('mini-1', '123456'))
    expect(within(card).getByRole('button', { name: 'Remove' })).toBeTruthy()
  })

  it('an online node: details, pause / drain / resume / remove', async () => {
    render(<ComputeSection />)
    const card = await screen.findByTestId('compute-node-mini-2')
    expect(within(card).getByTestId('node-dot').textContent).toBe('online')
    expect(card.textContent).toContain('8 cores')
    expect(card.textContent).toContain('16 GB RAM')
    expect(card.textContent).toContain('50 GB free')
    expect(card.textContent).toContain('1 running · 2 queued')
    expect(card.textContent).toContain('cargo 1.95.0')

    fireEvent.click(within(card).getByRole('button', { name: 'Pause' }))
    await waitFor(() => expect(m.pauseComputeNode).toHaveBeenCalledWith('mini-2', 'pause'))
    await waitFor(() => expect((within(card).getByRole('button', { name: 'Drain' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(within(card).getByRole('button', { name: 'Drain' }))
    await waitFor(() => expect(m.pauseComputeNode).toHaveBeenCalledWith('mini-2', 'drain'))
    // Not paused, so there is nothing to resume.
    expect((within(card).getByRole('button', { name: 'Resume' }) as HTMLButtonElement).disabled).toBe(true)

    await waitFor(() => expect((within(card).getByRole('button', { name: 'Remove' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(within(card).getByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(m.removeComputeNode).toHaveBeenCalledWith('mini-2'))
    expect(useConfirmDialogStore.getState().confirm).toHaveBeenCalledTimes(1)
  })

  it('a paused node can be resumed', async () => {
    m.fetchComputeNodes.mockResolvedValue({
      nodes: [{ ...ONLINE, controllerPause: 'paused' }],
      waitingCodes: [],
      controller: 'rosson.k2.dev',
    })
    render(<ComputeSection />)
    const card = await screen.findByTestId('compute-node-mini-2')
    expect(within(card).getByTestId('node-dot').textContent).toBe('paused')
    fireEvent.click(within(card).getByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(m.pauseComputeNode).toHaveBeenCalledWith('mini-2', 'resume'))
  })

  it('a grant: revoke; the workspace switch is read-only here (Agent tab owns it)', async () => {
    render(<ComputeSection />)
    const row = await screen.findByTestId('compute-grant-sales')
    expect(row.textContent).toContain('sales')
    expect(within(row).queryByRole('switch')).toBeNull()
    expect(within(row).getByTestId('compute-grant-switch').textContent).toBe(
      'Allow compute nodes: off (set on its Agent tab)',
    )

    fireEvent.click(within(row).getByRole('button', { name: 'Revoke' }))
    await waitFor(() => expect(m.revokeComputeGrant).toHaveBeenCalledWith('mini-2', 'p-sales'))
  })

  it('shows the offer features and a job\'s needs', async () => {
    m.fetchComputeNodes.mockResolvedValue({
      nodes: [{ ...ONLINE, offer: { ...ONLINE.offer!, features: ['needs', 'system-packages'] } }],
      waitingCodes: [],
      controller: 'rosson.k2.dev',
    })
    m.fetchComputeJobs.mockResolvedValue([
      {
        id: 'job-1',
        short: 'j1',
        node: 'mini-2',
        workspace: 'sales',
        argv: ['cargo', 'test'],
        state: 'running',
        reason: null,
        exitCode: null,
        createdAt: Math.floor(Date.now() / 1000),
        needs: { toolchains: ['rust 1.95'], packages: ['libssl-dev'] },
      },
      { id: 'job-2', short: 'j2', node: 'mini-2', workspace: 'sales', argv: ['ls'], state: 'done', reason: null, exitCode: 0, createdAt: 1 },
    ])
    render(<ComputeSection />)
    const features = await screen.findByTestId('compute-node-features')
    expect(features.textContent).toBe('[needs, system-packages]')
    const needs = await screen.findAllByTestId('compute-job-needs')
    expect(needs).toHaveLength(1)
    expect(needs[0].textContent).toBe('needs toolchains: rust 1.95; packages: libssl-dev')
  })

  it('a refusal shows code and hint', async () => {
    m.pauseComputeNode.mockRejectedValue(
      new Error(JSON.stringify({ error: { code: 'owner_only', hint: 'only the owner' } })),
    )
    render(<ComputeSection />)
    const card = await screen.findByTestId('compute-node-mini-2')
    fireEvent.click(within(card).getByRole('button', { name: 'Pause' }))
    await waitFor(() => expect(card.textContent).toContain('only the owner (owner_only)'))
  })

  it('add a node shows the enroll code and install commands', async () => {
    m.mintEnrollCode.mockResolvedValue({
      name: 'mini-3',
      code: 'ABCDE-FGHJK',
      controllerFp: 'ff',
      enroll: 'ABCDE-FGHJK.0123456789abcdef',
      expiresAt: Math.floor(Date.now() / 1000) + 600,
      controllerUrl: 'https://rosson.k2.dev',
      install: { linux: 'sudo linux-install', macos: 'sudo mac-install', enrollOnly: 'k2-node enroll x' },
      confirm: 'k2 compute node confirm mini-3 <code>',
    })
    render(<ComputeSection />)
    const name = await screen.findByLabelText('New node name')
    fireEvent.change(name, { target: { value: 'mini-3' } })
    fireEvent.click(screen.getByRole('button', { name: 'Make enroll code' }))
    const box = await screen.findByTestId('compute-enroll-code')
    expect(m.mintEnrollCode).toHaveBeenCalledWith('mini-3')
    expect(box.textContent).toContain('ABCDE-FGHJK.0123456789abcdef')
    expect(box.textContent).toContain('sudo linux-install')
    expect(box.textContent).toContain('sudo mac-install')
    expect(box.textContent).toContain('k2-node enroll x')
  })
})

describe('this computer', () => {
  async function openSheet(): Promise<HTMLElement> {
    const sw = await screen.findByRole('switch', { name: 'Make this computer a compute node' })
    fireEvent.click(sw)
    return screen.findByTestId('compute-enable-sheet')
  }

  function fillCodeForm(sheet: HTMLElement): void {
    fireEvent.change(within(sheet).getByLabelText('Node name'), { target: { value: 'mac-1' } })
    fireEvent.change(within(sheet).getByLabelText('Server address'), { target: { value: 'https://rosson.k2.dev' } })
    fireEvent.change(within(sheet).getByLabelText('Enroll code'), { target: { value: 'ABCDE-FGHJK.0123456789abcdef' } })
  }

  it('off state: the home check and the private-home choice is sent explicitly', async () => {
    m.enableLocalNode.mockResolvedValue({ ok: true, step: 'enrolling', message: 'Enrolling…' })
    render(<ComputeSection />)
    const part = await screen.findByTestId('compute-this-computer')
    expect(part.textContent).toContain('k2node')
    const sheet = await openSheet()
    const readable = within(sheet).getByTestId('compute-home-readable')
    expect(readable.textContent).toContain('/Users/r/Documents')
    expect(readable.textContent).toContain('/Users/r/.ssh')
    const box = within(sheet).getByLabelText('Make my home folder private (chmod 700)') as HTMLInputElement
    expect(box.checked).toBe(true)
    fillCodeForm(sheet)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    await waitFor(() =>
      expect(m.enableLocalNode).toHaveBeenCalledWith({
        controller: 'https://rosson.k2.dev',
        enroll: 'ABCDE-FGHJK.0123456789abcdef',
        name: 'mac-1',
        privateHome: true,
        remoteAdmin: 'policy',
      }),
    )
  })

  it('an already private home starts unchecked and sends privateHome: false', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      homeCheck: { home: '/Users/r', mode: '700', private: true, readable: [] },
    })
    m.enableLocalNode.mockResolvedValue({ ok: true, step: 'waiting' })
    render(<ComputeSection />)
    const sheet = await openSheet()
    const box = within(sheet).getByLabelText('Make my home folder private (chmod 700)') as HTMLInputElement
    expect(box.checked).toBe(false)
    fillCodeForm(sheet)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    await waitFor(() => expect(m.enableLocalNode).toHaveBeenCalledTimes(1))
    expect(m.enableLocalNode.mock.calls[0][0]).toMatchObject({ privateHome: false })
    expect(Object.prototype.hasOwnProperty.call(m.enableLocalNode.mock.calls[0][0], 'privateHome')).toBe(true)
  })

  it('needs_root shows the admin command to run once', async () => {
    m.enableLocalNode.mockResolvedValue({
      ok: true,
      step: 'needs_root',
      mode: 'command',
      command: 'sudo /Applications/K2.app/k2-node-install --enroll X',
      message: 'This needs an admin once.',
    })
    render(<ComputeSection />)
    const sheet = await openSheet()
    fillCodeForm(sheet)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    const result = await screen.findByTestId('compute-enable-result')
    expect(result.textContent).toContain('sudo /Applications/K2.app/k2-node-install --enroll X')
    expect(result.textContent).toContain('Run this once in a terminal as an admin')
  })

  it('a pending node on this window\'s server offers Confirm with the node code', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'pending', name: 'mac-1', controller: 'https://rosson.k2.dev', sas: '654321', node_fp: 'ab', control: 'active', running: [] },
    })
    m.confirmComputeNode.mockResolvedValue({ ok: true })
    render(<ComputeSection />)
    const sas = await screen.findByTestId('compute-local-sas')
    expect(sas.textContent).toContain('654 321')
    fireEvent.click(within(sas).getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(m.confirmComputeNode).toHaveBeenCalledWith('mac-1', '654321'))
  })

  it('on state: pause after current jobs vs pause now', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'online', name: 'mac-1', controller: 'https://other.k2.dev', control: 'active', running: ['job-a'] },
      policy: { cpu_cap_percent: 60, availability: 'always' },
    })
    render(<ComputeSection />)
    const panel = await screen.findByTestId('compute-local-node')
    expect(panel.textContent).toContain('job-a')
    fireEvent.click(within(panel).getByRole('button', { name: 'Pause after current jobs' }))
    await waitFor(() => expect(m.pauseLocalNode).toHaveBeenCalledWith(false))
    await waitFor(() => expect((within(panel).getByRole('button', { name: 'Pause now' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(within(panel).getByRole('button', { name: 'Pause now' }))
    await waitFor(() => expect(m.pauseLocalNode).toHaveBeenCalledWith(true))
  })

  it('one-click: picks an account server and enrolls with a Connect ticket', async () => {
    const acct = await import('../lib/compute-account')
    const conn = await import('./K2ConnectSection')
    vi.mocked(conn.getAccountAccessToken).mockResolvedValue('acc-tok')
    vi.mocked(acct.listAccountServers).mockResolvedValue([{ label: 'rosson', status: 'active', tunnel_token: 'x' }])
    vi.mocked(acct.requestEnrollTicket).mockResolvedValue({ ok: true, ticket: 'tkt-1' })
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'unenrolled', node_fp: 'fp-local', name: null, control: 'active', running: [] },
    })
    m.enableLocalNode.mockResolvedValue({ ok: true, step: 'enrolling' })
    render(<ComputeSection />)
    const sheet = await openSheet()
    fireEvent.change(within(sheet).getByLabelText('Node name'), { target: { value: 'mac-1' } })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Pick one of your servers' }))
    fireEvent.click(await within(sheet).findByRole('button', { name: 'rosson.k2.dev' }))
    await waitFor(() =>
      expect(acct.requestEnrollTicket).toHaveBeenCalledWith('acc-tok', { controllerSub: 'rosson', nodeFp: 'fp-local', nodeName: 'mac-1' }),
    )
    await waitFor(() =>
      expect(m.enableLocalNode).toHaveBeenCalledWith({
        controller: 'https://rosson.k2.dev',
        ticket: 'tkt-1',
        name: 'mac-1',
        privateHome: true,
        remoteAdmin: 'policy',
      }),
    )
  })

  it('one-click with no node key yet: installs first, then pairs with the key the install returned', async () => {
    vi.mocked(getAccountAccessToken).mockResolvedValue('acc-tok')
    vi.mocked(listAccountServers).mockResolvedValue([{ label: 'rosson', status: 'active', tunnel_token: 'x' }])
    vi.mocked(requestEnrollTicket).mockResolvedValue({ ok: true, ticket: 'tkt-2' })
    m.enableLocalNode
      .mockResolvedValueOnce({ ok: true, step: 'installed', nodeFp: 'fp-new', message: 'installed' })
      .mockResolvedValueOnce({ ok: true, step: 'waiting', requestId: 'r1' })
    render(<ComputeSection />)
    const sheet = await openSheet()
    fireEvent.change(within(sheet).getByLabelText('Node name'), { target: { value: 'mac-1' } })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Pick one of your servers' }))
    fireEvent.click(await within(sheet).findByRole('button', { name: 'rosson.k2.dev' }))
    await waitFor(() => expect(m.enableLocalNode).toHaveBeenCalledTimes(2))
    expect(m.enableLocalNode.mock.calls[0][0]).toEqual({ name: 'mac-1', privateHome: true, remoteAdmin: 'policy' })
    expect(requestEnrollTicket).toHaveBeenCalledTimes(1)
    expect(requestEnrollTicket).toHaveBeenCalledWith('acc-tok', { controllerSub: 'rosson', nodeFp: 'fp-new', nodeName: 'mac-1' })
    expect(m.enableLocalNode.mock.calls[1][0]).toEqual({
      controller: 'https://rosson.k2.dev',
      ticket: 'tkt-2',
      name: 'mac-1',
      privateHome: true,
      remoteAdmin: 'policy',
    })
  })

  // Rosson 2026-10-24 (ticket fd47ac33): two toggles right below the main
  // one; policy on by default and sent with the enable (one root step);
  // full only when asked, and only while policy is on.
  it('remote admin: policy is on by default and rides the enable; full is hidden until policy is on', async () => {
    m.enableLocalNode.mockResolvedValue({ ok: true, step: 'waiting' })
    render(<ComputeSection />)
    const part = await screen.findByTestId('compute-this-computer')
    const policy = await within(part).findByRole('switch', { name: 'Let K2 keep this node updated and healthy (remote admin: policy)' })
    expect(policy.getAttribute('aria-checked')).toBe('true')
    const full = within(part).getByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' })
    expect(full.getAttribute('aria-checked')).toBe('false')
    // Below the main switch, in that order.
    const switches = within(part).getAllByRole('switch').map((s) => s.getAttribute('aria-label'))
    expect(switches.slice(0, 3)).toEqual([
      'Make this computer a compute node',
      'Let K2 keep this node updated and healthy (remote admin: policy)',
      'Also let K2 run admin scripts as root (remote admin: full)',
    ])
    const sheet = await openSheet()
    fillCodeForm(sheet)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    await waitFor(() => expect(m.enableLocalNode).toHaveBeenCalledTimes(1))
    expect(m.enableLocalNode.mock.calls[0][0].remoteAdmin).toBe('policy')
  })

  it('remote admin: full is an opt-in; policy off sends off and clears full', async () => {
    m.enableLocalNode.mockResolvedValue({ ok: true, step: 'waiting' })
    render(<ComputeSection />)
    const part = await screen.findByTestId('compute-this-computer')
    const sheet = await openSheet()
    fillCodeForm(sheet)
    fireEvent.click(within(part).getByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' }))
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    await waitFor(() => expect(m.enableLocalNode).toHaveBeenCalledTimes(1))
    expect(m.enableLocalNode.mock.calls[0][0].remoteAdmin).toBe('full')

    const policy = within(part).getByRole('switch', { name: 'Let K2 keep this node updated and healthy (remote admin: policy)' })
    fireEvent.click(policy)
    expect(within(part).queryByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' })).toBeNull()
    await waitFor(() => expect((within(sheet).getByRole('button', { name: 'Make this computer a node' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(within(sheet).getByRole('button', { name: 'Make this computer a node' }))
    await waitFor(() => expect(m.enableLocalNode).toHaveBeenCalledTimes(2))
    expect(m.enableLocalNode.mock.calls[1][0].remoteAdmin).toBe('off')

    // Policy back on never brings full back by itself.
    fireEvent.click(policy)
    const full = within(part).getByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' })
    expect(full.getAttribute('aria-checked')).toBe('false')
  })

  it("on state, remote admin off: \"K2 can't update this node\" with the command; the switch asks the daemon", async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'online', name: 'mac-1', controller: 'https://other.k2.dev', control: 'active', running: [] },
      remoteAdmin: 'off',
      remoteAdminCommand: "sudo '/Library/Application Support/K2/node/k2-node' admin level policy",
    })
    m.setLocalRemoteAdmin.mockResolvedValue({ ok: true, done: true, remoteAdmin: 'policy', message: 'K2 keeps this node updated and healthy (remote admin: policy).' })
    render(<ComputeSection />)
    const ra = await screen.findByTestId('compute-local-remote-admin')
    const warn = within(ra).getByTestId('compute-cant-update')
    expect(warn.textContent).toContain("K2 can't update this node")
    expect(warn.textContent).toContain('admin level policy')
    expect(within(ra).queryByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' })).toBeNull()
    fireEvent.click(within(ra).getByRole('switch', { name: 'Let K2 keep this node updated and healthy (remote admin: policy)' }))
    await waitFor(() => expect(m.setLocalRemoteAdmin).toHaveBeenCalledWith('policy'))
    expect(await within(ra).findByText('K2 keeps this node updated and healthy (remote admin: policy).')).toBeTruthy()
  })

  it('on state, remote admin policy: full asks the daemon; a command to copy when root needs the owner', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'online', name: 'mac-1', controller: 'https://other.k2.dev', control: 'active', running: [] },
      remoteAdmin: 'policy',
    })
    m.setLocalRemoteAdmin.mockResolvedValue({ ok: true, done: false, rootNeeded: true, command: 'sudo /usr/local/libexec/k2-node admin level full' })
    render(<ComputeSection />)
    const ra = await screen.findByTestId('compute-local-remote-admin')
    expect(within(ra).queryByTestId('compute-cant-update')).toBeNull()
    expect(within(ra).getByRole('switch', { name: 'Let K2 keep this node updated and healthy (remote admin: policy)' }).getAttribute('aria-checked')).toBe('true')
    fireEvent.click(within(ra).getByRole('switch', { name: 'Also let K2 run admin scripts as root (remote admin: full)' }))
    await waitFor(() => expect(m.setLocalRemoteAdmin).toHaveBeenCalledWith('full'))
    expect((await within(ra).findByText('sudo /usr/local/libexec/k2-node admin level full')).textContent).toContain('admin level full')
  })

  it('shows a pending enroll request and a failed enroll', async () => {
    m.fetchLocalStatus.mockResolvedValueOnce({ ...LOCAL_OFF, enrollRequestPending: true })
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      enrollRequestPending: false,
      enrollResult: { id: 'r1', ok: false, message: 'enroll code expired', sas: null, name: 'mac-1', at: Math.floor(Date.now() / 1000) },
    })
    const { unmount } = render(<ComputeSection />)
    expect((await screen.findByTestId('compute-enroll-pending')).textContent).toContain('Waiting for the node')
    unmount()
    render(<ComputeSection />)
    expect((await screen.findByTestId('compute-enroll-failed')).textContent).toContain('enroll code expired')
  })

  it('limits: repo toolchains is sent as a boolean', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'online', name: 'mac-1', controller: 'https://other.k2.dev', control: 'active', running: [] },
      policy: { cpu_cap_percent: 60, availability: 'always' },
    })
    render(<ComputeSection />)
    const panel = await screen.findByTestId('compute-local-node')
    const sw = within(panel).getByRole('switch', { name: "Install the toolchains a job's repo declares" })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    fireEvent.click(within(panel).getByRole('button', { name: 'Save limits' }))
    await waitFor(() => expect(m.setLocalPolicy).toHaveBeenCalledWith({ repo_toolchains: false }))
  })

  it('stop being a node: done, or the admin command', async () => {
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'online', name: 'mac-1', controller: 'https://other.k2.dev', control: 'active', running: [] },
    })
    m.disableLocalNode.mockResolvedValueOnce({ done: false, command: 'sudo k2-node uninstall', message: 'Run this once as an admin' })
    render(<ComputeSection />)
    const panel = await screen.findByTestId('compute-local-node')
    fireEvent.click(within(panel).getByRole('button', { name: 'Stop being a compute node' }))
    await waitFor(() => expect(m.disableLocalNode).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(panel.textContent).toContain('sudo k2-node uninstall'))
  })

  it('one-click falls back to an enroll code when Connect has no ticket route', async () => {
    const acct = await import('../lib/compute-account')
    const conn = await import('./K2ConnectSection')
    vi.mocked(conn.getAccountAccessToken).mockResolvedValue('acc-tok')
    vi.mocked(acct.listAccountServers).mockResolvedValue([{ label: 'rosson', status: 'active', tunnel_token: 'x' }])
    vi.mocked(acct.requestEnrollTicket).mockResolvedValue({
      ok: false,
      unavailable: true,
      message: "One-click isn't available on this account yet; use an enroll code",
    })
    m.fetchLocalStatus.mockResolvedValue({
      ...LOCAL_OFF,
      installed: true,
      node: { state: 'unenrolled', node_fp: 'fp-local', name: null, control: 'active', running: [] },
    })
    render(<ComputeSection />)
    const sheet = await openSheet()
    fireEvent.change(within(sheet).getByLabelText('Node name'), { target: { value: 'mac-1' } })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Pick one of your servers' }))
    fireEvent.click(await within(sheet).findByRole('button', { name: 'rosson.k2.dev' }))
    await waitFor(() => expect(sheet.textContent).toContain("One-click isn't available on this account yet; use an enroll code"))
    expect((within(sheet).getByLabelText('Server address') as HTMLInputElement).value).toBe('https://rosson.k2.dev')
    expect(m.enableLocalNode).not.toHaveBeenCalled()
  })

  it('the web build hides This computer and never reads the local daemon', async () => {
    features.computeLocal = false
    render(<ComputeSection />)
    await screen.findByTestId('compute-node-mini-2')
    expect(screen.queryByTestId('compute-this-computer')).toBeNull()
    expect(screen.queryByRole('switch', { name: 'Make this computer a compute node' })).toBeNull()
    expect(m.fetchLocalStatus).not.toHaveBeenCalled()
  })
})
