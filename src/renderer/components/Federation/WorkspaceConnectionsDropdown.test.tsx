// @vitest-environment jsdom
//
// Federation V1 (Phase 5) cross-server agent picker — role-gate + fail-closed
// behavior. `prd-cross-server-agent-comms.md`.
//
// The federation client (`@/lib/federation`) is mocked so the component graph
// is hermetic (no daemon-cli / Tauri). Tests assert:
//   - a MEMBER (and unauthenticated) sees NOTHING and never calls the daemon
//     (the renderer-side role-gate the task requires);
//   - an OWNER with trusted peers + a roster sees selectable agent options and
//     selecting one sets the cross-server target;
//   - when federation is unavailable (daemon flag off → client reports
//     `available:false`), an owner still sees nothing (fail-closed default).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  listFederationPeers: vi.fn(),
  fetchPeerRoster: vi.fn(),
}))

vi.mock('@/lib/federation', async () => {
  const actual = await vi.importActual<typeof import('@/lib/federation')>('@/lib/federation')
  return {
    ...actual, // keep real trustedPeers / crossServerTarget
    listFederationPeers: h.listFederationPeers,
    fetchPeerRoster: h.fetchPeerRoster,
  }
})

import WorkspaceConnectionsDropdown from './WorkspaceConnectionsDropdown'
import { useFederationTargetStore, __resetFederationTargetForTests } from '@/stores/federation-target'
import type { FederationPeer, RosterAgent } from '@/lib/federation'

const PEER: FederationPeer = {
  fingerprint: 'fp-rosson-laptop',
  label: 'rosson@laptop',
  subdomain: 'rosson',
  trust: 'trusted',
  capabilities: ['inbound', 'roster'],
}

const AGENT: RosterAgent = {
  workspace_id: 'ws-uuid-1',
  workspace_name: 'Backend',
  agent: 'scout',
  address: 'ws-uuid-1::scout',
}

beforeEach(() => {
  cleanup()
  __resetFederationTargetForTests()
  h.listFederationPeers.mockReset()
  h.fetchPeerRoster.mockReset()
})

describe('WorkspaceConnectionsDropdown role-gate', () => {
  it('renders NOTHING for a member and never queries the daemon', async () => {
    const { container } = render(<WorkspaceConnectionsDropdown role="member" />)
    // Give any (incorrect) async effect a chance to run.
    await Promise.resolve()
    expect(container.firstChild).toBeNull()
    expect(h.listFederationPeers).not.toHaveBeenCalled()
    expect(h.fetchPeerRoster).not.toHaveBeenCalled()
  })

  it('renders NOTHING when role is null (unauthenticated)', async () => {
    const { container } = render(<WorkspaceConnectionsDropdown role={null} />)
    await Promise.resolve()
    expect(container.firstChild).toBeNull()
    expect(h.listFederationPeers).not.toHaveBeenCalled()
  })

  it('lists a trusted peer’s agents for an owner and sets the cross-server target on pick', async () => {
    h.listFederationPeers.mockResolvedValue({ available: true, data: [PEER] })
    h.fetchPeerRoster.mockResolvedValue({ available: true, data: [AGENT] })

    const onSelect = vi.fn()
    render(<WorkspaceConnectionsDropdown role="owner" onSelectTarget={onSelect} />)

    const option = await screen.findByTestId('federation-agent-option')
    expect(option.textContent).toContain('Backend · scout')

    fireEvent.click(option)
    // Target = <peer-fingerprint>::<workspace-uuid>::<agent>.
    const expected = 'fp-rosson-laptop::ws-uuid-1::scout'
    expect(onSelect).toHaveBeenCalledWith(expected, 'rosson@laptop · scout')
    expect(useFederationTargetStore.getState().target).toBe(expected)
  })

  it('renders NOTHING for an owner when federation is unavailable (flag off)', async () => {
    h.listFederationPeers.mockResolvedValue({ available: false })
    const { container } = render(<WorkspaceConnectionsDropdown role="owner" />)
    await waitFor(() => expect(h.listFederationPeers).toHaveBeenCalled())
    expect(container.firstChild).toBeNull()
    expect(h.fetchPeerRoster).not.toHaveBeenCalled()
  })

  it('filters out non-trusted peers (no pending/blocked agents surface)', async () => {
    const pending: FederationPeer = { ...PEER, fingerprint: 'fp-pending', trust: 'pending' }
    h.listFederationPeers.mockResolvedValue({ available: true, data: [pending] })
    const { container } = render(<WorkspaceConnectionsDropdown role="admin" />)
    await waitFor(() => expect(h.listFederationPeers).toHaveBeenCalled())
    expect(container.firstChild).toBeNull()
    // A pending peer must never be queried for its roster.
    expect(h.fetchPeerRoster).not.toHaveBeenCalled()
  })
})
