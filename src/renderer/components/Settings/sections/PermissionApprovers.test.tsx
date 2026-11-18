// @vitest-environment jsdom

// Settings → Access → Permission Cards → Who agents ask (0.46.16): the
// owner's `permissionApprovers`, read from and written to the daemon, one
// write per flip, rolled back on a refusal; an admin sees it read-only.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.get, daemonCliPost: h.post }))

import { WhoAgentsAskRows } from './PermissionCardsSection'
import { parseApproverChoices, permissionApproversSetting } from './permission-cards-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'

let stored: string[]

beforeEach(() => {
  vi.clearAllMocks()
  stored = []
  h.get.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'settings/get') return { permissionCards: true, permissionApprovers: stored }
    if (route === 'users') {
      return {
        users: [
          { username: 'ada', role: 'admin', disabled: false },
          { username: 'rosa', role: 'owner', disabled: false },
          { username: 'mo', role: 'member', disabled: false },
          { username: 'gone', role: 'admin', disabled: true },
        ],
      }
    }
    throw new Error(`unexpected GET ${route}`)
  })
  h.post.mockImplementation(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    if (route !== 'settings/update') throw new Error(`unexpected POST ${route}`)
    const names = body.permissionApprovers as string[]
    if (names.includes('rosa')) throw new DaemonHttpError('rosa is busy', 400, 'bad_request')
    stored = names
    return { permissionApprovers: stored }
  })
})

afterEach(() => cleanup())

describe('Who agents ask', () => {
  it('parses the setting and keeps owner and admin logins that are on', () => {
    expect(permissionApproversSetting({ permissionApprovers: ['ada'] })).toEqual(['ada'])
    expect(permissionApproversSetting({ permissionCards: true })).toBeNull()
    expect(() => permissionApproversSetting({ permissionApprovers: 'ada' })).toThrow()
    expect(
      parseApproverChoices({
        users: [
          { username: 'ada', role: 'admin' },
          { username: 'mo', role: 'member' },
          { username: 'gone', role: 'owner', disabled: true },
        ],
      }),
    ).toEqual([{ login: 'ada', role: 'admin' }])
  })

  it('lists the owner and every owner or admin login, and writes each flip', async () => {
    render(<WhoAgentsAskRows canEdit nonce={0} />)
    await screen.findByTestId('permission-approvers')
    expect(screen.getByTestId('permission-approvers-summary').textContent).toContain('Everyone who can answer')
    expect(screen.getByTestId('permission-approver-owner')).toBeTruthy()
    expect(screen.getByTestId('permission-approver-ada')).toBeTruthy()
    expect(screen.getByTestId('permission-approver-rosa')).toBeTruthy()
    expect(screen.queryByTestId('permission-approver-mo')).toBeNull()
    expect(screen.queryByTestId('permission-approver-gone')).toBeNull()

    fireEvent.click(screen.getByRole('switch', { name: 'Ask ada (admin)' }))
    await waitFor(() => expect(stored).toEqual(['ada']))
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'settings/update', { permissionApprovers: ['ada'] })
    await waitFor(() => expect(screen.getByTestId('permission-approvers-summary').textContent).toContain('Only the people picked'))
  })

  it('rolls a refused write back and says why', async () => {
    render(<WhoAgentsAskRows canEdit nonce={0} />)
    await screen.findByTestId('permission-approvers')
    const rosa = screen.getByRole('switch', { name: 'Ask rosa (owner)' })
    fireEvent.click(rosa)
    await waitFor(() => expect(screen.getByText(/rosa is busy/)).toBeTruthy())
    expect(stored).toEqual([])
    expect(rosa.getAttribute('aria-checked')).toBe('false')
  })

  it('is read-only for an admin', async () => {
    stored = ['ada']
    render(<WhoAgentsAskRows canEdit={false} nonce={0} />)
    await screen.findByTestId('permission-approvers-owner-only')
    const ada = screen.getByRole('switch', { name: 'Ask ada (admin)' })
    expect(ada.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(ada)
    expect(h.post).not.toHaveBeenCalled()
  })
})
