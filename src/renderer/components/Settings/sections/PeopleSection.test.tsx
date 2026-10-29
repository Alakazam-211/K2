// @vitest-environment jsdom
//
// Settings → Access → User Access. Principals (username + full name).
// Not Admin Access's ConnectTab 'people', not skin-access.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
}))

vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => h.role) }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

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
  h.role = 'owner'
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
    expect(await screen.findByRole('button', { name: 'Add user' })).toBeTruthy()
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
      email: null,
      tiedTo: null,
      pendingReset: false,
      tiedReasons: [],
      storedEmailRefused: null,
      lockedUntil: null,
    }])
  })

  // agent-app-users T6.1: a tied, pending person shows the badge and the
  // reset action; an untied person shows none. A tied person's stored
  // email is never offered (verification review 2026-10-26, item 1).
  it('shows Waiting for reset with its action only for a tied, pending person', async () => {
    allowRoster([
      { username: 'tina', pendingReset: true, tiedTo: 'sales', email: 'tina@example.test', storedEmailRefused: 'tied' },
      { username: 'owen', pendingReset: false, tiedTo: null },
    ])
    h.daemonCliPost.mockResolvedValue({ ok: true, delivered: 'sent', sentTo: 'tina@home.test', from: 'owner@box.test', ttlSecs: 86400 })
    render(<PeopleSection />)
    await waitFor(() => expect(screen.getByLabelText('Open tina')).toBeTruthy())
    expect(screen.getByLabelText('Open tina').textContent).toContain('Waiting for reset')
    expect(screen.getByLabelText('Open owen').textContent).not.toContain('Waiting for reset')

    fireEvent.click(screen.getByLabelText('Open owen'))
    expect(screen.queryByRole('button', { name: 'Reset password…' })).toBeNull()

    fireEvent.click(screen.getByLabelText('Open tina'))
    expect(screen.getByText(/so a reset never goes there/)).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: 'Reset password…' }))
    const dialog = screen.getByTestId('reset-password-dialog')
    expect(dialog.textContent).toContain('sales’s agent set tina up')
    fireEvent.click(screen.getByRole('radio', { name: /Email them the link/ }))
    const to = screen.getByLabelText('Email for tina') as HTMLInputElement
    expect(to.value).toBe('')
    expect(dialog.textContent).toContain('K2 won’t use the email on file (tina@example.test)')
    fireEvent.change(to, { target: { value: 'tina@home.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send link' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/reset-link', { username: 'tina', to: 'tina@home.test' })
    })
    expect(await screen.findByText(/Reset link sent to tina@home.test from owner@box.test/)).toBeTruthy()
  })

  // Security review item 3 + verification item 1: whatever the tie's
  // reason, the stored email is never a reset target.
  it('never offers the stored email for any tie reason', async () => {
    for (const reasons of [['created'], ['invite'], ['app_key'], ['created', 'email']]) {
      allowRoster([{ username: 'tess', pendingReset: true, tiedTo: 'sales', tiedReasons: reasons, email: 'tess@agent.test' }])
      const view = render(<PeopleSection />)
      fireEvent.click(await screen.findByRole('button', { name: 'Password actions for tess' }))
      fireEvent.click(screen.getByRole('menuitem', { name: 'Reset password…' }))
      fireEvent.click(screen.getByRole('radio', { name: /Email them the link/ }))
      expect((screen.getByLabelText('Email for tess') as HTMLInputElement).value).toBe('')
      view.unmount()
    }
  })

  // 0.46.6: the daemon lets only the Owner manage App users, so only the
  // Owner sees the row actions and Add user.
  it('shows the row actions and Add user only to the Owner', async () => {
    for (const role of ['admin', 'member', null] as const) {
      h.role = role
      allowRoster([{ username: 'ada', lockedUntil: 4102444800 }])
      const view = render(<PeopleSection />)
      await screen.findByLabelText('Open ada')
      // Let the role read settle before checking nothing appeared.
      await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('skin/roles'))
      await new Promise((r) => setTimeout(r, 0))
      expect(screen.queryByRole('button', { name: 'Password actions for ada' }), String(role)).toBeNull()
      expect(screen.queryByRole('button', { name: 'Add user' }), String(role)).toBeNull()
      view.unmount()
    }
    h.role = 'owner'
    allowRoster([{ username: 'ada' }])
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for ada' }))
    expect(screen.getByRole('menuitem', { name: 'Reset password…' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Set password…' })).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Unlock' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Add user' })).toBeTruthy()
  })

  it('shows a one-time reset link once, and closing the sheet drops it', async () => {
    allowRoster([{ username: 'ada', email: 'ada@clinic.test' }])
    const link = 'https://box.example/cli/skin/password/set#k2inv_test-only'
    h.daemonCliPost.mockResolvedValue({ ok: true, delivered: 'returned', link, expiresAt: 4102444800, ttlSecs: 86400 })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Reset password…' }))
    expect(screen.getByRole('radio', { name: /Get a one-time link/ })).toHaveProperty('checked', true)
    fireEvent.click(screen.getByRole('button', { name: 'Get link' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/reset-link', { username: 'ada', code: true })
    })
    expect((await screen.findByTestId('one-time-secret')).textContent).toBe(link)
    expect(screen.getByText('It expires in 24 hours.')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Copy reset link' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByTestId('reset-password-dialog')).toBeNull()
    expect(document.body.textContent).not.toContain('k2inv_test-only')
    // Opening it again starts over: no link until asked again.
    fireEvent.click(screen.getByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Reset password…' }))
    expect(screen.queryByTestId('one-time-secret')).toBeNull()
    expect(document.body.textContent).not.toContain('k2inv_test-only')
    // Not an untied person's stored email when emailing: it's offered.
    fireEvent.click(screen.getByRole('radio', { name: /Email them the link/ }))
    expect((screen.getByLabelText('Email for ada') as HTMLInputElement).value).toBe('ada@clinic.test')
  })

  it('renders the daemon’s refusal text in the sheet', async () => {
    allowRoster([{ username: 'ada' }])
    h.daemonCliPost.mockRejectedValue(
      new Error(JSON.stringify({ error: { code: 'front_door_unsafe', hint: 'This server’s web address is not safe for a reset page.' } })),
    )
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Reset password…' }))
    fireEvent.click(screen.getByRole('button', { name: 'Get link' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('This server’s web address is not safe for a reset page.')
    expect(screen.queryByTestId('one-time-secret')).toBeNull()
  })

  it('Set password: typed, or generated by the daemon and shown once', async () => {
    allowRoster([{ username: 'ada' }])
    h.daemonCliPost.mockResolvedValue({ username: 'ada', hasPassword: true })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Set password…' }))
    const dialog = screen.getByTestId('set-password-dialog')
    expect(dialog.textContent).toContain('signs ada out everywhere')
    const field = screen.getByLabelText('New password for ada') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.click(screen.getByRole('button', { name: 'Show' }))
    expect(field.type).toBe('text')
    fireEvent.change(field, { target: { value: 'typed-test-password' } })
    fireEvent.click(screen.getByRole('button', { name: 'Set password and sign them out' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/password', { username: 'ada', password: 'typed-test-password' })
    })
    expect(await screen.findByText('Password set. ada is signed out everywhere.')).toBeTruthy()
    expect(screen.queryByTestId('one-time-secret')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))

    h.daemonCliPost.mockResolvedValue({ username: 'ada', hasPassword: true, generatedPassword: 'Test1-Only2-Fake3-Value' })
    fireEvent.click(screen.getByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Set password…' }))
    fireEvent.click(screen.getByRole('button', { name: 'Generate one and sign them out' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/password', { username: 'ada', generate: true })
    })
    expect((await screen.findByTestId('one-time-secret')).textContent).toBe('Test1-Only2-Fake3-Value')
    expect(screen.getAllByText('Test1-Only2-Fake3-Value')).toHaveLength(1)
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(document.body.textContent).not.toContain('Test1-Only2-Fake3-Value')
    fireEvent.click(screen.getByRole('button', { name: 'Password actions for ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Set password…' }))
    expect((screen.getByLabelText('New password for ada') as HTMLInputElement).value).toBe('')
    expect(document.body.textContent).not.toContain('Test1-Only2-Fake3-Value')
  })

  it('Set password for an agent-set-up person says it keeps the agent’s hold and offers Reset', async () => {
    allowRoster([{ username: 'tina', tiedTo: 'sales', pendingReset: true }])
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for tina' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Set password…' }))
    expect(screen.getByTestId('set-password-dialog').textContent).toContain('doesn’t remove that agent’s hold')
    fireEvent.click(screen.getByRole('button', { name: 'Reset password instead…' }))
    expect(screen.queryByTestId('set-password-dialog')).toBeNull()
    expect(screen.getByTestId('reset-password-dialog')).toBeTruthy()
  })

  it('offers Unlock only for a locked login and clears it', async () => {
    allowRoster([{ username: 'lena', lockedUntil: 4102444800 }, { username: 'otto' }])
    h.daemonCliPost.mockResolvedValue({ ok: true, username: 'lena', cleared: true })
    render(<PeopleSection />)
    await screen.findByLabelText('Open lena')
    expect(screen.getByLabelText('Open lena').textContent).toContain('Locked')
    expect(screen.getByLabelText('Open otto').textContent).not.toContain('Locked')
    fireEvent.click(await screen.findByRole('button', { name: 'Password actions for otto' }))
    expect(screen.queryByRole('menuitem', { name: 'Unlock' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Password actions for otto' }))
    fireEvent.click(screen.getByRole('button', { name: 'Password actions for lena' }))
    allowRoster([{ username: 'lena' }, { username: 'otto' }])
    fireEvent.click(screen.getByRole('menuitem', { name: 'Unlock' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/unlock', { username: 'lena' })
    })
    expect(await screen.findByText('lena is unlocked and can sign in again.')).toBeTruthy()
    await waitFor(() => expect(screen.getByLabelText('Open lena').textContent).not.toContain('Locked'))
  })

  it('Add user opens a sheet: a typed password, with full name and email', async () => {
    allowRoster([])
    h.daemonCliPost.mockResolvedValue({ username: 'carol' })
    render(<PeopleSection />)
    expect(screen.queryByLabelText('New user’s username')).toBeNull()
    fireEvent.click(await screen.findByRole('button', { name: 'Add user' }))
    const sheet = screen.getByTestId('add-user-dialog')
    const submit = (): HTMLButtonElement =>
      Array.from(sheet.querySelectorAll('button[type="submit"]'))[0] as HTMLButtonElement
    expect(submit().disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('New user’s username'), { target: { value: 'Carol' } })
    fireEvent.change(screen.getByLabelText('New user’s full name'), { target: { value: 'Carol Danvers' } })
    expect(submit().disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('New user’s password'), { target: { value: 'typed-test-password' } })
    fireEvent.change(screen.getByLabelText('New user’s email'), { target: { value: 'carol@clinic.test' } })
    allowRoster([{ username: 'carol', fullName: 'Carol Danvers' }])
    fireEvent.click(submit())
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users', {
        username: 'carol',
        password: 'typed-test-password',
        email: 'carol@clinic.test',
      })
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', { username: 'carol', fullName: 'Carol Danvers' })
    await waitFor(() => expect(screen.queryByTestId('add-user-dialog')).toBeNull())
    expect(screen.queryByTestId('added-user-dialog')).toBeNull()
    expect(await screen.findByLabelText('Open carol')).toBeTruthy()
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'skin/templates')).toBe(true)
  })

  it('Add user with a generated password closes the sheet and shows it once', async () => {
    allowRoster([])
    h.daemonCliPost.mockResolvedValue({ username: 'dan', generatedPassword: 'Test4-Only5-Fake6-Value' })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Add user' }))
    fireEvent.change(screen.getByLabelText('New user’s username'), { target: { value: 'dan' } })
    fireEvent.click(screen.getByRole('radio', { name: /Let K2 generate one/ }))
    expect(screen.queryByLabelText('New user’s password')).toBeNull()
    allowRoster([{ username: 'dan' }])
    fireEvent.click(within(screen.getByTestId('add-user-dialog')).getByRole('button', { name: 'Add user' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users', { username: 'dan', generate: true }))
    expect(screen.queryByTestId('add-user-dialog')).toBeNull()
    expect((await screen.findByTestId('one-time-secret')).textContent).toBe('Test4-Only5-Fake6-Value')
    expect(screen.getByRole('button', { name: 'Copy password' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(document.body.textContent).not.toContain('Test4-Only5-Fake6-Value')
    expect(screen.getByLabelText('Open dan')).toBeTruthy()
  })

  it('Add user can send an invite link only with an email and an App', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/users') return { users: [] }
      if (route === 'skin/grants') return { grants: [] }
      if (route === 'skin/roles') return { roles: [{ id: 'role-staff', name: 'staff', appId: 'svc-shop' }, { id: 'role-other', name: 'other', appId: 'svc-x' }] }
      if (route === 'projects/list') return [{ id: 'ws-sales' }]
      if (route === 'publish/list') return { services: [{ id: 'svc-shop', name: 'shop', kind: 'cmd' }] }
      throw new Error(`unexpected GET ${route}`)
    })
    h.daemonCliPost.mockResolvedValue({ ok: true, delivered: 'sent', sentTo: 'eve@clinic.test', ttlSecs: 259200 })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Add user' }))
    const invite = screen.getByRole('radio', { name: /Send them a link to set their own/ }) as HTMLInputElement
    expect(invite.disabled).toBe(true)
    expect(screen.getByTestId('add-user-dialog').textContent).toContain('Needs an email.')
    fireEvent.change(screen.getByLabelText('New user’s username'), { target: { value: 'eve' } })
    fireEvent.change(screen.getByLabelText('New user’s email'), { target: { value: 'eve@clinic.test' } })
    expect(invite.disabled).toBe(false)
    fireEvent.click(invite)
    await waitFor(() => expect(screen.getByRole('option', { name: 'shop (cmd)' })).toBeTruthy())
    const submit = screen.getByRole('button', { name: 'Add and send link' }) as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    fireEvent.change(screen.getByLabelText('Invite to App'), { target: { value: 'svc-shop' } })
    expect(screen.queryByRole('option', { name: 'other' })).toBeNull()
    fireEvent.change(screen.getByLabelText('Role in the App'), { target: { value: 'role-staff' } })
    fireEvent.click(submit)
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/invite', {
        username: 'eve',
        app: 'svc-shop',
        email: 'eve@clinic.test',
        role: 'role-staff',
      })
    })
    expect(h.daemonCliPost.mock.calls.every((c) => c[0] !== 'skin/users')).toBe(true)
    expect(await screen.findByText(/Invite sent to eve@clinic.test/)).toBeTruthy()
    // Clearing the email takes the invite option away again.
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    fireEvent.click(screen.getByRole('button', { name: 'Add user' }))
    fireEvent.change(screen.getByLabelText('New user’s email'), { target: { value: 'x@clinic.test' } })
    fireEvent.click(screen.getByRole('radio', { name: /Send them a link to set their own/ }))
    fireEvent.change(screen.getByLabelText('New user’s email'), { target: { value: '' } })
    await waitFor(() => expect(screen.getByRole('radio', { name: /Type one/ })).toHaveProperty('checked', true))
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
