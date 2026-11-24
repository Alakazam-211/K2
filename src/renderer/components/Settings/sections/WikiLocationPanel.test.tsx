// @vitest-environment jsdom
//
// Settings → workspace → Advanced → Wiki location. Mock daemonCli*: the
// row reads GET wiki/location and the owner's Approve sends POST
// wiki/location/approve; nothing shows for a wiki that is a plain folder.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import { WikiLocationPanel } from './WikiLocationPanel'

const base = {
  workspace: '/w/ws',
  linked: true,
  target: '/vaults/notes',
  approved: null,
  lines: ['/w/ws/.k2/wiki is a link to /vaults/notes, not approved'],
  doctor: [],
}

describe('WikiLocationPanel', () => {
  beforeEach(() => {
    cleanup()
    h.daemonCliGet.mockReset()
    h.daemonCliPost.mockReset()
  })

  it('shows nothing for a wiki that is a plain folder', async () => {
    h.daemonCliGet.mockResolvedValue({ ...base, linked: false, target: null, status: 'not_linked' })
    const { container } = render(<WikiLocationPanel projectPath="/w/ws" />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('wiki/location', { project: '/w/ws' }))
    expect(container.querySelector('[data-testid="wiki-location-row"]')).toBeNull()
  })

  it('offers Approve for an unapproved link and sends it', async () => {
    h.daemonCliGet.mockResolvedValue({ ...base, status: 'not_approved' })
    h.daemonCliPost.mockResolvedValue({
      ...base,
      status: 'approved',
      approved: { target: '/vaults/notes', approvedAt: '2026-11-23T00:00:00Z' },
      lines: ['approved'],
    })
    render(<WikiLocationPanel projectPath="/w/ws" />)
    const approve = await screen.findByTestId('wiki-location-approve')
    expect(screen.getByText('/vaults/notes')).toBeTruthy()
    fireEvent.click(approve)
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('wiki/location/approve', { project: '/w/ws' }))
    await waitFor(() => expect(screen.queryByTestId('wiki-location-approve')).toBeNull())
  })

  it('shows an approved link without an Approve button', async () => {
    h.daemonCliGet.mockResolvedValue({
      ...base,
      status: 'approved',
      approved: { target: '/vaults/notes', approvedAt: '2026-11-23T00:00:00Z' },
    })
    render(<WikiLocationPanel projectPath="/w/ws" />)
    await screen.findByTestId('wiki-location-row')
    expect(screen.queryByTestId('wiki-location-approve')).toBeNull()
  })
})
