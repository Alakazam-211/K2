// @vitest-environment jsdom

// Settings → Apps → <App> → People (app-mobile-access-v1, 0.46.3): the
// person × agent grid draws exactly the daemon's cells; the suggested role
// templates show as chips only while the App has no roles, and a click (not
// opening the page) makes one; Stay signed in, Listed for sign-in and a
// device's Sign out post the owner routes on the window's server.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
    daemonCliPostQuery: vi.fn(),
  }
})

import { AppPeopleAccess } from './AppPeopleAccess'
import { cellSummary, parseAccess, parseTemplates } from './app-people-api'

const ACCESS = {
  ok: true,
  app: 'svc-1',
  agents: [
    { room: 'r-desk', handle: 'desk', displayName: 'Front Desk', color: '#112233' },
    { room: 'r-jobs', handle: 'jobs', displayName: 'Jobs', color: null },
  ],
  people: [
    {
      principalId: 'p1',
      username: 'cleo',
      fullName: 'Cleo Client',
      kind: 'guest',
      enabled: true,
      grantId: 'g1',
      role: 'jobs-client',
      cells: [
        {
          room: 'r-jobs',
          handle: 'jobs',
          caps: ['thread:read', 'thread:post', 'files:read', 'files:write'],
          can: { chat: true, send: true, files: 'write', tickets: 'none', activity: false, members: false, heartbeats: 'none', wiki: false, store: 'none', identity: false },
          folders: { read: ['Shared'], write: ['Inbox'], uploadDir: 'Inbox' },
        },
      ],
    },
    { principalId: 'p2', username: 'olga', fullName: null, kind: 'guest', enabled: false, grantId: 'g2', role: null, cells: [] },
  ],
  devices: [
    { id: 'dev-1', principalId: 'p1', username: 'cleo', app: 'svc-1', deviceId: 'iphone', name: 'Cleo iPhone', createdAt: 1, lastUsedAt: 2, expiresAt: 3, via: 'pairing' },
    { id: 'dev-2', principalId: 'p1', username: 'cleo', app: 'svc-1', deviceId: 'ipad', name: 'Cleo iPad', createdAt: 1, lastUsedAt: 2, expiresAt: 3, via: 'password' },
  ],
  settings: { app: 'svc-1', listed: false, staySignedInDays: 30 },
}

const TEMPLATES = {
  ok: true,
  templates: [
    { id: 'owner', label: 'Owner', description: 'Runs the business.', caps: ['thread:read'], note: 'Approve/sign/send arrives later.' },
    { id: 'field', label: 'Field', description: 'On site.', caps: ['files:write'], note: 'Upload-only arrives later.' },
  ],
}

function routes(): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'skin/access') return ACCESS
    if (route === 'skin/roles/suggest') return TEMPLATES
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockResolvedValue({ ok: true })
}

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  routes()
})
afterEach(() => cleanup())

describe('app-people-api', () => {
  it('parses the grid and summarises a cell in words', () => {
    const a = parseAccess(ACCESS)
    expect(a.agents.map((x) => x.displayName)).toEqual(['Front Desk', 'Jobs'])
    expect(a.people[0].cells[0].folders).toEqual({ read: ['Shared'], write: ['Inbox'], uploadDir: 'Inbox' })
    expect(a.people[1].enabled).toBe(false)
    expect(a.staySignedInDays).toBe(30)
    expect(cellSummary(a.people[0].cells[0])).toBe('chat · files')
    expect(parseTemplates(TEMPLATES).map((t) => t.id)).toEqual(['owner', 'field'])
    expect(parseAccess({}).people).toEqual([])
    // App phone pairing (0.46.5): how a device sign-in started.
    expect(a.devices.map((d) => d.via)).toEqual(['pairing', 'password'])
    expect(parseAccess({ devices: [{ id: 'x' }] }).devices[0].via).toBe('password')
  })
})

describe('AppPeopleAccess', () => {
  it('draws the person × agent grid from the daemon', async () => {
    render(<AppPeopleAccess app="svc-1" hasRoles onRolesChanged={() => {}} />)
    await screen.findByRole('table', { name: 'People and agents' })
    expect(h.daemonCliGet).toHaveBeenCalledWith('skin/access', { app: 'svc-1' })
    expect(screen.getByText('Front Desk')).toBeTruthy()
    expect(screen.getByTestId('cell-cleo-jobs').textContent).toContain('chat · files')
    expect(screen.getByTestId('cell-cleo-jobs').textContent).toContain('read Shared, write Inbox')
    expect(screen.getByTestId('cell-cleo-desk').textContent).toBe('—')
    expect(screen.getByText('Cleo Client')).toBeTruthy()
    expect(screen.getByText('olga')).toBeTruthy()
    expect(screen.getAllByText('off').length).toBe(1)
    // A phone paired by QR carries a Paired badge; a password device doesn't.
    expect(screen.getAllByText('Paired').length).toBe(1)
    // With roles, no suggestions are fetched or shown.
    expect(h.daemonCliGet).not.toHaveBeenCalledWith('skin/roles/suggest')
    expect(screen.queryByLabelText('Suggested roles')).toBeNull()
  })

  it('offers the templates as chips on an App with no roles, and makes one only on a click', async () => {
    const changed = vi.fn()
    render(<AppPeopleAccess app="svc-1" hasRoles={false} onRolesChanged={changed} />)
    const chip = await screen.findByRole('button', { name: '+ Field' })
    expect(chip.getAttribute('title')).toContain('Upload-only arrives later.')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.click(chip)
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/roles/from-template', { app: 'svc-1', template: 'field' }),
    )
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1))
  })

  it('lists for sign-in, signs a device out, and never offers discovery for host', async () => {
    const { unmount } = render(<AppPeopleAccess app="svc-1" hasRoles onRolesChanged={() => {}} />)
    const sw = await screen.findByRole('switch', { name: 'Listed for sign-in' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/access/listed', { app: 'svc-1', listed: true }))
    fireEvent.click(screen.getByRole('button', { name: 'Sign out Cleo iPhone' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/access/devices/revoke', { id: 'dev-1' }))
    unmount()
    render(<AppPeopleAccess app="host" hasRoles onRolesChanged={() => {}} />)
    await screen.findByRole('table', { name: 'People and agents' })
    expect(screen.queryByRole('switch', { name: 'Listed for sign-in' })).toBeNull()
  })

  it('shows a daemon refusal instead of an empty grid', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('owner only'))
    render(<AppPeopleAccess app="svc-1" hasRoles onRolesChanged={() => {}} />)
    expect((await screen.findByRole('alert')).textContent).toContain('owner only')
  })
})
