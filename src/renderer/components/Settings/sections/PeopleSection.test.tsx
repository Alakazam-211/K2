// @vitest-environment jsdom
//
// Settings → K2 Server → User Access. Principals (username + full name).
// Not Admin Access's ConnectTab 'people', not skin-access.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

import { PeopleSection, PEOPLE_MANIFEST, parsePeople } from './PeopleSection'
import { UserTemplatesSection } from './UserTemplatesSection'
import { SECTION_LABELS } from '../searchManifest'

const dir = dirname(fileURLToPath(import.meta.url))

function allowRoster(users: unknown): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'skin/users') return { users }
    if (route === 'skin/grants') return { grants: [] }
    if (route === 'skin/roles') return { roles: [] }
    if (route === 'skin/templates') return { templates: [] }
    throw new Error(`unexpected GET ${route}`)
  })
}

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})

describe('PEOPLE_MANIFEST', () => {
  it('is K2 Server User Access, not skin-access and not Admin Access', () => {
    expect(PEOPLE_MANIFEST.every((e) => e.section === 'people')).toBe(true)
    expect(PEOPLE_MANIFEST.some((e) => e.section === 'skin-access')).toBe(false)
    expect(SECTION_LABELS.people).toBe('User Access')
    expect(SECTION_LABELS['user-templates']).toBe('User Templates')
    expect(SECTION_LABELS.domains).toBe('Custom Domains')
    expect(SECTION_LABELS['k2-access']).toBe('Admin Access')
    expect(SECTION_LABELS['skin-access']).toBe('Apps')
  })
})

describe('PeopleSection', () => {
  it('lists username and full name from principals and does not fetch projects', async () => {
    allowRoster([
      { username: 'ada', fullName: 'Ada Lovelace', defaultRoomHandles: ['secret-room'] },
      { username: 'bob', fullName: null },
    ])
    render(<PeopleSection />)
    await waitFor(() => {
      expect(screen.getByText('ada')).toBeTruthy()
    })
    expect(screen.getByRole('heading', { level: 2, name: 'User Access' })).toBeTruthy()
    expect(screen.queryByText('Access templates')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Open template Clinic' })).toBeNull()
    expect(screen.getByLabelText('New skin username')).toBeTruthy()
    expect(screen.getByText('Ada Lovelace')).toBeTruthy()
    expect(screen.queryByText('secret-room')).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Admin Access' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Apps' })).toBeNull()
    expect(screen.queryByLabelText('ada full name')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Open ada' }))
    const ada = screen.getByLabelText('ada full name') as HTMLInputElement
    expect(ada.value).toBe('Ada Lovelace')
    fireEvent.click(screen.getByRole('button', { name: 'Open bob' }))
    const bob = screen.getByLabelText('bob full name') as HTMLInputElement
    expect(bob.value).toBe('')
    expect(h.daemonCliGet).toHaveBeenCalledWith('skin/users')
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'projects/list')).toBe(true)
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'publish/list')).toBe(true)
  })

  it('owner save and clear post skin/users/full-name', async () => {
    allowRoster([{ username: 'ada', fullName: 'Ada Lovelace' }])
    h.daemonCliPost.mockResolvedValue({ username: 'ada', fullName: 'Ada/Lovelace: MD' })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Open ada' }))
    const input = screen.getByLabelText('ada full name')
    fireEvent.change(input, { target: { value: 'Ada/Lovelace: MD' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save ada full name' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', {
        username: 'ada',
        fullName: 'Ada/Lovelace: MD',
      })
    })

    allowRoster([{ username: 'ada', fullName: 'Ada Lovelace' }])
    fireEvent.click(screen.getByRole('button', { name: 'Clear ada full name' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', {
        username: 'ada',
        fullName: '',
      })
    })
  })

  it('grant row uses a switch and kind is a SettingDropdown', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/users') {
        return {
          users: [{
            id: 'p-ada',
            username: 'ada',
            fullName: 'Ada Lovelace',
            roleId: 'role-1',
            roleName: 'reader',
            defaultRoomHandles: ['secret-room'],
          }],
        }
      }
      if (route === 'skin/grants') {
        return {
          grants: [{
            id: 'g1',
            subjectKind: 'principal',
            subjectId: 'p-ada',
            kind: 'app',
            targetId: 'svc-1',
            roleId: 'role-2',
            enabled: true,
          }, {
            id: 'gm',
            subjectKind: 'principal',
            subjectId: 'p-ada',
            kind: 'mailbox',
            targetId: 'box',
            scope: 'inbox',
            enabled: true,
          }],
        }
      }
      if (route === 'skin/roles') {
        return {
          roles: [
            { id: 'role-1', name: 'reader', roomAccess: [{ handle: 'secret-room', caps: ['thread:read'] }] },
            { id: 'role-2', name: 'editor', roomAccess: [{ handle: 'sales', caps: ['files:read'] }] },
          ],
        }
      }
      if (route === 'skin/templates') {
        return { templates: [{ id: 't1', name: 'Clinic', lines: [] }] }
      }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<PeopleSection />)
    await screen.findByRole('button', { name: 'Open ada' })
    expect(screen.queryByText('Access templates')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Open template Clinic' })).toBeNull()
    expect(screen.queryByText('secret-room')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Open ada' }))
    expect(screen.queryByText(/mailbox/)).toBeNull()
    const sw = screen.getByRole('switch', { name: 'Enable app svc-1' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(screen.getByRole('button', { name: 'Show app svc-1' }))
    expect(screen.getByText('editor')).toBeTruthy()
    expect(screen.getByText('sales')).toBeTruthy()
    expect(screen.queryByText('secret-room')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Grant kind' }))
    expect(screen.getByTestId('setting-dropdown-menu')).toBeTruthy()
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'projects/list')).toBe(true)
  })

  it('does not call fetchProjects and is not the Connect people tab', () => {
    const src = readFileSync(resolve(dir, 'PeopleSection.tsx'), 'utf8')
    expect(src).not.toContain('fetchProjects')
    expect(src).not.toContain('skin-access')
    expect(src).not.toContain("section: 'k2-access'")
    const shell = readFileSync(resolve(dir, 'K2ConnectSettingsShell.tsx'), 'utf8')
    expect(shell).toContain("title: 'Admin Access'")
    expect(shell).toContain("if (section === 'k2-access') return 'people'")
    expect(shell).not.toContain('PeopleSection')
    const parsed = parsePeople({
      users: [{ username: 'ada', full_name: 'Ada Lovelace' }, { username: '' }],
    })
    expect(parsed).toEqual([{
      id: null,
      username: 'ada',
      fullName: 'Ada Lovelace',
      roleId: null,
      roleName: null,
      defaultRooms: [],
      defaultRoomHandles: [],
    }])
  })

  it('adds a person with password, full name, and optional email', async () => {
    allowRoster([])
    h.daemonCliPost.mockResolvedValue({ username: 'carol' })
    render(<PeopleSection />)
    fireEvent.change(screen.getByLabelText('New skin username'), { target: { value: 'Carol' } })
    fireEvent.change(screen.getByLabelText('New skin full name'), { target: { value: 'Carol Danvers' } })
    fireEvent.change(screen.getByLabelText('New skin password'), { target: { value: 's3cret-horse' } })
    fireEvent.change(screen.getByLabelText('New skin email'), { target: { value: 'carol@clinic.com' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add user' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users', {
        username: 'carol',
        password: 's3cret-horse',
        email: 'carol@clinic.com',
      })
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', {
      username: 'carol',
      fullName: 'Carol Danvers',
    })
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'skin/templates')).toBe(true)
  })

  it('User Templates renders the access template list', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/templates') return { templates: [{ id: 't1', name: 'Clinic', lines: [] }] }
      if (route === 'skin/roles') return { roles: [] }
      if (route === 'skin/users') return { users: [] }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<UserTemplatesSection />)
    expect(await screen.findByRole('heading', { level: 2, name: 'User Templates' })).toBeTruthy()
    expect(screen.getByText('Access templates')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Open template Clinic' })).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'User Access' })).toBeNull()
  })
})
