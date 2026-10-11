// @vitest-environment jsdom

// Settings → Compute, node admin (prd-k2-node-admin-v1 S10): Approvals show
// the request in plain words (and any root script in full); Approve always
// runs a fresh owner confirm for THAT request before the daemon signs;
// Deny carries a reason; non-owners never get an Approve button; a node's
// health, pending approvals, update state and offline alert render from
// daemon truth (mocked compute-admin-api).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

vi.mock('./compute-admin-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-admin-api')>()
  return {
    ...orig,
    fetchAdminRequests: vi.fn(),
    approveAdminRequest: vi.fn(),
    denyAdminRequest: vi.fn(),
    startAdminConfirm: vi.fn(),
    finishAdminConfirm: vi.fn(),
    fetchNodeHealth: vi.fn(),
    updateNode: vi.fn(),
    rollbackNode: vi.fn(),
    setNodeOfflineAlert: vi.fn(),
  }
})

import * as api from './compute-admin-api'
import type { AdminRequest, NodeHealthView } from './compute-admin-api'
import { ApprovalsPanel, NodeAdminPanel } from './ComputeAdminParts'

const m = vi.mocked(api)

function req(over: Partial<AdminRequest> = {}): AdminRequest {
  return {
    id: 'car_1',
    nodeId: 'n-1',
    node: 'mini-1',
    kind: 'command',
    class: 'privileged',
    body: { verb: 'script', args: { text: 'rm -rf /var/cache/lima/old\necho done\n', interpreter: '/bin/bash' } },
    digest: 'sha256:abc',
    summary: 'run a root script on mini-1 (2 lines, 37 bytes) — read it in full before approving',
    reason: 'Lima image cache holds 3 old images',
    proposedBy: 'agent:it',
    proposerWorkspace: 'ws-it',
    proposerWorkspaceName: 'it',
    proposedAt: Math.floor(Date.now() / 1000) - 60,
    state: 'pending',
    decidedBy: null,
    decidedVia: null,
    decidedAt: null,
    denyReason: null,
    expiresAt: Math.floor(Date.now() / 1000) + 86000,
    cmdIds: [],
    nextStep: 0,
    result: null,
    ...over,
  }
}

function health(over: Partial<NodeHealthView> = {}): NodeHealthView {
  return {
    node: 'mini-1',
    nodeId: 'n-1',
    online: true,
    lastSeen: 1,
    nodeVersion: '0.45.5',
    at: Math.floor(Date.now() / 1000) - 30,
    facts: {
      uptime_secs: 9 * 86400 + 3600,
      disks: [{ name: 'root', path: '/', total_bytes: 500 * 2 ** 30, free_bytes: 9 * 2 ** 30 }],
      disk_encryption: 'on',
      sleep: { autorestart: 1 },
      executor: { reachable: true, remote_admin: 'full', approval_key_fps: ['ab'] },
    },
    remoteAdmin: 'full',
    approvalKeyFp: 'ab',
    alertAfterSecs: null,
    updateRing: 'normal',
    update: { state: 'rolled_back', version: '0.46.0', reason: 'no hello in 120 s' },
    alertsTo: 'it',
    pendingApprovals: 1,
    features: ['admin', 'health', 'self-update'],
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
})
afterEach(() => cleanup())

describe('ApprovalsPanel', () => {
  it('shows the request in plain words with the whole script, and approves after a Mac confirm for that request', async () => {
    m.fetchAdminRequests.mockResolvedValueOnce({ requests: [req()], canApprove: true }).mockResolvedValue({ requests: [], canApprove: true })
    m.startAdminConfirm.mockResolvedValue({ methods: ['local-auth', 'vault-password'] })
    m.finishAdminConfirm.mockResolvedValue({ token: 'k2vc_tok' })
    m.approveAdminRequest.mockResolvedValue({ ok: true, request: req({ state: 'sent' }), sent: true, note: 'sent' })
    render(<ApprovalsPanel />)
    await screen.findByTestId('compute-request-car_1')
    expect(screen.getByText(/run a root script on mini-1/)).toBeTruthy()
    expect(screen.getByText(/Lima image cache/)).toBeTruthy()
    expect(screen.getByText(/asked by agent it/)).toBeTruthy()
    expect(screen.getByTestId('compute-request-script').textContent).toBe('rm -rf /var/cache/lima/old\necho done\n')
    fireEvent.click(screen.getByTestId('compute-approve-car_1'))
    await waitFor(() => expect(m.approveAdminRequest).toHaveBeenCalledWith('car_1', 'k2vc_tok'))
    expect(m.startAdminConfirm).toHaveBeenCalledWith('car_1')
    expect(m.finishAdminConfirm).toHaveBeenCalledWith('car_1', 'local-auth', req().summary, undefined)
    await waitFor(() => expect(screen.queryByTestId('compute-request-car_1')).toBeNull())
  })

  it('asks for a password where there is no Touch ID, and never approves without it', async () => {
    m.fetchAdminRequests.mockResolvedValue({ requests: [req()], canApprove: true })
    m.startAdminConfirm.mockResolvedValue({ methods: ['connect-password'] })
    m.finishAdminConfirm.mockResolvedValue({ token: 'k2vc_pw' })
    m.approveAdminRequest.mockResolvedValue({ ok: true, request: req({ state: 'sent' }), sent: true, note: '' })
    render(<ApprovalsPanel />)
    fireEvent.click(await screen.findByTestId('compute-approve-car_1'))
    const pw = (await screen.findByTestId('compute-approve-password')) as HTMLInputElement
    expect(m.approveAdminRequest).not.toHaveBeenCalled()
    expect(screen.getByText('Your K2 Connect password')).toBeTruthy()
    fireEvent.change(pw, { target: { value: 'hunter2-long' } })
    fireEvent.submit(pw.closest('form') as HTMLFormElement)
    await waitFor(() => expect(m.finishAdminConfirm).toHaveBeenCalledWith('car_1', 'connect-password', req().summary, 'hunter2-long'))
    await waitFor(() => expect(m.approveAdminRequest).toHaveBeenCalledWith('car_1', 'k2vc_pw'))
    // The password never stays in the field.
    expect(pw.value).toBe('')
  })

  it('shows the daemon refusal when the approval expired', async () => {
    m.fetchAdminRequests.mockResolvedValue({ requests: [req()], canApprove: true })
    m.startAdminConfirm.mockResolvedValue({ methods: ['local-auth'] })
    m.finishAdminConfirm.mockResolvedValue({ token: 't' })
    m.approveAdminRequest.mockRejectedValue(
      new Error('{"error":{"code":"approval_expired","hint":"this request expired (24 h); ask the agent to propose it again"}}'),
    )
    render(<ApprovalsPanel />)
    fireEvent.click(await screen.findByTestId('compute-approve-car_1'))
    expect(await screen.findByText(/this request expired/)).toBeTruthy()
  })

  it('never shows Approve to someone who is not the owner, but lets them deny with a reason', async () => {
    m.fetchAdminRequests.mockResolvedValue({ requests: [req({ body: { verb: 'power.reboot', args: {} }, summary: 'reboot mini-1' })], canApprove: false })
    m.denyAdminRequest.mockResolvedValue({ ok: true, request: req({ state: 'denied' }) })
    render(<ApprovalsPanel />)
    await screen.findByTestId('compute-request-car_1')
    expect(screen.queryByTestId('compute-approve-car_1')).toBeNull()
    expect(screen.getByText("Only the server's owner approves.")).toBeTruthy()
    expect(screen.queryByTestId('compute-request-script')).toBeNull()
    fireEvent.click(screen.getByTestId('compute-deny-car_1'))
    fireEvent.change(screen.getByLabelText('Why deny'), { target: { value: 'not now' } })
    fireEvent.click(screen.getByText('Deny'))
    await waitFor(() => expect(m.denyAdminRequest).toHaveBeenCalledWith('car_1', 'not now'))
  })

  it('shows approved and running requests, and hides itself on a daemon without node admin', async () => {
    m.fetchAdminRequests.mockResolvedValueOnce({ requests: [req({ state: 'approved', summary: 'reboot mini-1' })], canApprove: true })
    const { unmount } = render(<ApprovalsPanel />)
    expect((await screen.findByTestId('compute-request-inflight-car_1')).textContent).toContain('Approved, waiting for the node')
    unmount()
    m.fetchAdminRequests.mockRejectedValue(new Error('{"error":{"code":"not_found","hint":"no such compute route"}}'))
    render(<ApprovalsPanel />)
    await waitFor(() => expect(m.fetchAdminRequests).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(screen.queryByTestId('compute-approvals')).toBeNull())
  })
})

describe('NodeAdminPanel', () => {
  it('reads health only when opened, then shows facts, pending approvals and the update state', async () => {
    m.fetchNodeHealth.mockResolvedValue(health())
    m.updateNode.mockResolvedValue({ ok: true, update: { state: 'offered', version: '0.46.0' }, note: 'the node verifies, swaps…' })
    m.setNodeOfflineAlert.mockResolvedValue({ ok: true, alertAfterSecs: 600 })
    render(<NodeAdminPanel node="mini-1" />)
    expect(m.fetchNodeHealth).not.toHaveBeenCalled()
    fireEvent.click(screen.getByTestId('compute-node-admin-open-mini-1'))
    const list = await screen.findByTestId('compute-node-health')
    expect(list.textContent).toContain('Up 9d 1h')
    expect(list.textContent).toContain('9 GB free of 500 GB on /')
    expect(list.textContent).toContain('Disk encryption on: after a power cut someone types the password at the machine')
    expect(list.textContent).toContain('Power back on after a cut: yes')
    expect(screen.getByTestId('compute-node-pending').textContent).toContain('1 request(s)')
    expect(screen.getByTestId('compute-node-update').textContent).toContain('rolled_back 0.46.0 (no hello in 120 s)')
    expect(screen.getByText(/Alerts go to it/)).toBeTruthy()
    fireEvent.click(screen.getByText('Update k2-node'))
    await waitFor(() => expect(m.updateNode).toHaveBeenCalledWith('mini-1'))
    expect(await screen.findByText('the node verifies, swaps…')).toBeTruthy()
    fireEvent.click(screen.getByTestId('compute-node-alert-mini-1'))
    await waitFor(() => expect(m.setNodeOfflineAlert).toHaveBeenCalledWith('mini-1', 600))
  })

  it('shows the one-time pin a node needs before admin works', async () => {
    m.fetchNodeHealth.mockResolvedValue(health({ needsPin: true, pinCommand: 'sudo k2-node admin pin' }))
    render(<NodeAdminPanel node="mini-1" />)
    fireEvent.click(screen.getByTestId('compute-node-admin-open-mini-1'))
    expect((await screen.findByTestId('compute-node-needs-pin')).textContent).toContain('sudo k2-node admin pin')
  })

  it('says so when the node cannot self-update and turns an alert off', async () => {
    m.fetchNodeHealth.mockResolvedValue(health({ features: ['admin'], alertAfterSecs: 600, facts: null, update: null }))
    m.setNodeOfflineAlert.mockResolvedValue({ ok: true, alertAfterSecs: null })
    render(<NodeAdminPanel node="mini-1" />)
    fireEvent.click(screen.getByTestId('compute-node-admin-open-mini-1'))
    expect(await screen.findByText('Remote update needs a k2-node with self-update.')).toBeTruthy()
    expect(screen.queryByText('Update k2-node')).toBeNull()
    expect(screen.getByText(/No health report yet/)).toBeTruthy()
    fireEvent.click(screen.getByTestId('compute-node-alert-mini-1'))
    await waitFor(() => expect(m.setNodeOfflineAlert).toHaveBeenCalledWith('mini-1', null))
  })
})
