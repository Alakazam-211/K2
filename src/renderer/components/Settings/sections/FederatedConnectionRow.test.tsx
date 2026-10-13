// @vitest-environment jsdom
// 0159: a federated connection whose workspace left its server is muted,
// says why, and offers Re-add to an owner or admin only.
import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

import { FederatedConnectionRow } from './FederatedConnectionRow'
import type { RemoteConnectionEntry } from '@/lib/federation'
import { canReAddGoneConnection, remoteConnectionGoneNote } from '@/lib/federation-gone'

const live: RemoteConnectionEntry = { remote: true, address: 'ai::rpm.k2.dev', host: 'rpm.k2.dev', agent: 'ai', gone: false }
const gone: RemoteConnectionEntry = { ...live, gone: true }

afterEach(cleanup)

describe('FederatedConnectionRow', () => {
  it('a live row looks like today: federated, no note, no Re-add', () => {
    const { container } = render(
      <FederatedConnectionRow entry={live} canReAdd busy={false} onRemove={() => {}} onReAdd={() => {}} />,
    )
    const row = container.querySelector('[data-federated-connection="ai::rpm.k2.dev"]')
    expect(row).not.toBeNull()
    expect(row?.getAttribute('data-gone')).toBeNull()
    expect(screen.getByText('federated')).toBeTruthy()
    expect(screen.queryByText(/no longer exists/)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Re-add' })).toBeNull()
  })

  it('a gone row is marked, says why, and an owner/admin can re-add it', () => {
    const onReAdd = vi.fn()
    const onRemove = vi.fn()
    const { container } = render(
      <FederatedConnectionRow entry={gone} canReAdd busy={false} onRemove={onRemove} onReAdd={onReAdd} />,
    )
    const row = container.querySelector('[data-federated-connection="ai::rpm.k2.dev"]')
    expect(row?.getAttribute('data-gone')).toBe('true')
    expect(screen.getByText('gone')).toBeTruthy()
    expect(screen.getByText('Workspace no longer exists on rpm.k2.dev. Re-add to reconnect.')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Re-add' }))
    expect(onReAdd).toHaveBeenCalledTimes(1)
    expect(onRemove).not.toHaveBeenCalled()
    fireEvent.click(screen.getByTitle('Remove federated connection'))
    expect(onRemove).toHaveBeenCalledTimes(1)
  })

  it('a member sees the gone note but no Re-add', () => {
    render(<FederatedConnectionRow entry={gone} canReAdd={false} busy={false} onRemove={() => {}} onReAdd={() => {}} />)
    expect(
      screen.getByText(
        /Workspace no longer exists on rpm\.k2\.dev\. Re-add to reconnect\. Only an owner or admin can re-add it\./,
      ),
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Re-add' })).toBeNull()
  })

  it('shows progress and blocks a second click while re-adding', () => {
    render(<FederatedConnectionRow entry={gone} canReAdd busy onRemove={() => {}} onReAdd={() => {}} />)
    const b = screen.getByRole('button', { name: 'Re-adding…' }) as HTMLButtonElement
    expect(b.disabled).toBe(true)
  })
})

describe('gone helpers', () => {
  it('only owner and admin may re-add', () => {
    expect(canReAddGoneConnection('owner')).toBe(true)
    expect(canReAddGoneConnection('admin')).toBe(true)
    expect(canReAddGoneConnection('member')).toBe(false)
    expect(canReAddGoneConnection(null)).toBe(false)
  })

  it('names the server, or says that server', () => {
    expect(remoteConnectionGoneNote('rpm.k2.dev')).toBe('Workspace no longer exists on rpm.k2.dev. Re-add to reconnect.')
    expect(remoteConnectionGoneNote('')).toBe('Workspace no longer exists on that server. Re-add to reconnect.')
  })
})
