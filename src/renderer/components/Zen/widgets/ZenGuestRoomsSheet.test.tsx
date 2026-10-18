// @vitest-environment jsdom
// RS-A15: K2's sheet for a Garden widget's guests.setRooms. K2's words from
// the daemon's change; Approve runs the person's vault confirm bound to the
// change (action garden-guests, its secretId); Cancel is a no.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost, daemonCliGet: vi.fn() }))
vi.mock('@/lib/zen/zen-api', () => ({ zenLocalScope: () => ({ id: 'primary', isPrimary: true }) }))

import { ZenGuestRoomsSheet } from './ZenGuestRoomsSheet'

const ask = {
  secretId: 'guests:abc',
  change: {
    app: 'svc-1',
    appName: 'jobs',
    appHandle: 'nsi/jobs',
    username: 'dana',
    role: null,
    before: ['p-own'],
    beforeHandles: ['nsi'],
    after: ['p-acme'],
    afterHandles: ['acme'],
    widget: 'guestdesk',
    madeBy: 'agent:board-builder',
  },
}

afterEach(() => {
  cleanup()
  daemonCliPost.mockReset()
})

describe('ZenGuestRoomsSheet', () => {
  it('names the change in K2’s words and Cancel is a no', () => {
    const onDone = vi.fn()
    render(<ZenGuestRoomsSheet ask={ask} onDone={onDone} />)
    expect(screen.getByTestId('zen-guest-rooms-user').textContent).toBe('dana')
    expect(screen.getByTestId('zen-guest-rooms-widget').textContent).toBe('guestdesk')
    expect(screen.getByTestId('zen-guest-rooms-before').textContent).toBe('nsi')
    expect(screen.getByTestId('zen-guest-rooms-after').textContent).toBe('acme')
    expect(screen.getByText('Adds acme.')).toBeTruthy()
    expect(screen.getByText('Removes nsi.')).toBeTruthy()
    expect(screen.getByText(/An agent made this widget/)).toBeTruthy()
    fireEvent.click(screen.getByTestId('zen-guest-rooms-cancel'))
    expect(onDone).toHaveBeenCalledWith(null)
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('Approve takes the person’s confirm bound to this change and hands back the token', async () => {
    daemonCliPost.mockImplementation(async (_scope: unknown, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'k2vc_ok' }
      throw new Error(`unexpected ${route}`)
    })
    const onDone = vi.fn()
    render(<ZenGuestRoomsSheet ask={ask} onDone={onDone} />)
    fireEvent.click(screen.getByTestId('zen-guest-rooms-approve'))
    await waitFor(() => expect(onDone).toHaveBeenCalledWith('k2vc_ok'))
    expect(daemonCliPost.mock.calls[0]?.[1]).toBe('vault/confirm/start')
    expect(daemonCliPost.mock.calls[0]?.[2]).toEqual({ action: 'garden-guests', secretId: 'guests:abc' })
    expect(daemonCliPost.mock.calls[1]?.[2]).toMatchObject({ action: 'garden-guests', secretId: 'guests:abc', method: 'local-auth' })
  })
})
