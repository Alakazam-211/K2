// @vitest-environment jsdom
//
// Settings → Access → User Access: everyone at a glance, then one person's
// access (What they can use) and account (Account). Not Admin Access's
// ConnectTab 'people', not skin-access.

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

import { BOX_LOGIN_REASON, PeopleSection, PEOPLE_MANIFEST, canResetPassword, parsePeople } from './PeopleSection'
import { showPersonNext } from './user-access-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { MUST_CHANGE_NOTE } from './AppUserDialogs'
import { UserTemplatesSection } from './UserTemplatesSection'
import { SECTION_LABELS } from '../searchManifest'
import { menuOptions, pickValue, waitForMenuOptions } from '@/test-utils/setting-dropdown'

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

const confirmSpy = vi.fn(async () => true)

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.role = 'owner'
  confirmSpy.mockReset()
  confirmSpy.mockResolvedValue(true)
  useConfirmDialogStore.setState({ confirm: confirmSpy })
})

/** Open a person from the list, on one of their two tabs. */
async function openPerson(username: string, tab: 'What they can use' | 'Account' = 'What they can use'): Promise<void> {
  fireEvent.click(await screen.findByRole('button', { name: `Open ${username}` }))
  fireEvent.click(screen.getByRole('tab', { name: tab }))
}

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

const ROLES = [
  { id: 'role-1', name: 'reader', roomAccess: [{ handle: 'notes', caps: ['thread:read'] }] },
  { id: 'role-2', name: 'editor', appId: 'svc-1', roomAccess: [{ handle: 'sales', caps: ['files:read'] }] },
]

/** ada: this server's K2 page, the Shop App as editor (off), a mailbox and a
 *  database note; bob: nothing. */
function allowFull(over: { grants?: unknown[] } = {}): void {
  h.daemonCliGet.mockImplementation(async (route: string, q?: Record<string, string>) => {
    if (route === 'skin/users') {
      return {
        users: [
          { id: 'p-ada', username: 'ada', fullName: 'Ada Lovelace', roleId: 'role-1', defaultRoomHandles: ['secret-room'] },
          { id: 'p-bob', username: 'bob' },
        ],
      }
    }
    if (route === 'skin/grants') {
      return {
        grants: over.grants ?? [
          { id: 'gh', subjectKind: 'principal', subjectId: 'p-ada', kind: 'app', targetId: 'host', roleId: 'role-1', enabled: true },
          { id: 'g1', subjectKind: 'principal', subjectId: 'p-ada', kind: 'app', targetId: 'svc-1', roleId: 'role-2', enabled: false },
          { id: 'gm', subjectKind: 'principal', subjectId: 'p-ada', kind: 'mailbox', targetId: 'box', scope: 'inbox', enabled: true },
          { id: 'gd', subjectKind: 'principal', subjectId: 'p-ada', kind: 'database', targetId: 'reports', scope: 'read', enabled: true },
        ],
      }
    }
    if (route === 'skin/roles') return { roles: ROLES }
    if (route === 'projects/list') return [{ id: 'ws-1' }, { id: 'ws-2' }]
    if (route === 'publish/list') return q?.project === 'ws-1' ? { services: [{ id: 'svc-1', name: 'Shop', kind: 'skin' }] } : { services: [] }
    throw new Error(`unexpected GET ${route}`)
  })
}

describe('PeopleSection', () => {
  it('opens on everyone, with what each person can use, and never fetches projects for people with no Apps', async () => {
    allowRoster([
      { username: 'ada', fullName: 'Ada Lovelace', defaultRoomHandles: ['secret-room'] },
      { username: 'bob', fullName: null },
    ])
    render(<PeopleSection />)
    expect(await screen.findByRole('button', { name: 'Open ada' })).toBeTruthy()
    expect(screen.getByRole('heading', { level: 2, name: 'User Access' })).toBeTruthy()
    expect(screen.getByRole('heading', { level: 2, name: 'Everyone' })).toBeTruthy()
    expect(screen.queryByText('Select a person.')).toBeNull()
    expect(screen.getByTestId('summary-ada').textContent).toBe('Nothing yet')
    expect(screen.queryByText('Access templates')).toBeNull()
    expect(await screen.findByRole('button', { name: 'Add person' })).toBeTruthy()
    expect(screen.getAllByText('Ada Lovelace').length).toBeGreaterThan(0)
    expect(screen.queryByText('secret-room')).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Admin Access' })).toBeNull()
    expect(screen.queryByLabelText('ada full name')).toBeNull()
    await openPerson('ada', 'Account')
    expect((screen.getByLabelText('ada full name') as HTMLInputElement).value).toBe('Ada Lovelace')
    await openPerson('bob', 'Account')
    expect((screen.getByLabelText('bob full name') as HTMLInputElement).value).toBe('')
    // Everyone again, from the list's first entry.
    fireEvent.click(screen.getByTestId('people-everyone'))
    expect(screen.getByRole('heading', { level: 2, name: 'Everyone' })).toBeTruthy()
    expect(h.daemonCliGet).toHaveBeenCalledWith('skin/users')
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'projects/list')).toBe(true)
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'publish/list')).toBe(true)
  })

  it('shows loading, then a failed read with its reason and Try again', async () => {
    let fail = true
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (fail) throw new Error('database is locked')
      if (route === 'skin/users') return { users: [{ username: 'ada' }] }
      return route === 'skin/grants' ? { grants: [] } : { roles: [] }
    })
    render(<PeopleSection />)
    expect(screen.getByTestId('people-loading').textContent).toContain('Loading people')
    const err = await screen.findByTestId('people-error')
    expect(err.getAttribute('role')).toBe('alert')
    expect(err.textContent).toContain('Couldn’t load people')
    expect(err.textContent).toContain('The server said: database is locked.')
    fail = false
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByRole('button', { name: 'Open ada' })).toBeTruthy()
  })

  it('with no one yet says how people get here and links to Admin Access', async () => {
    allowRoster([])
    render(<PeopleSection />)
    const empty = await screen.findByTestId('people-empty')
    expect(empty.textContent).toContain('No one can use this server yet')
    expect(empty.textContent).toContain('when you add them')
    fireEvent.click(within(empty).getByRole('button', { name: 'Open Admin Access' }))
    expect(useSettingsStore.getState().activeSection).toBe('k2-access')
    expect(await within(empty).findByRole('button', { name: 'Add person' })).toBeTruthy()
  })

  it('opens on the person User Templates handed over', async () => {
    allowRoster([{ username: 'ada' }, { username: 'bob' }])
    showPersonNext('bob')
    render(<PeopleSection />)
    expect(await screen.findByRole('heading', { level: 2, name: 'bob' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Open bob' }).getAttribute('aria-current')).toBe('true')
  })

  it('save posts skin/users/full-name, says Saved, and an empty name clears it', async () => {
    allowRoster([{ username: 'ada', fullName: 'Ada Lovelace' }])
    h.daemonCliPost.mockResolvedValue({ username: 'ada', fullName: 'Ada/Lovelace: MD' })
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    expect(screen.queryByRole('button', { name: 'Save ada full name' })).toBeNull()
    const input = screen.getByLabelText('ada full name')
    fireEvent.change(input, { target: { value: 'Ada/Lovelace: MD' } })
    allowRoster([{ username: 'ada', fullName: 'Ada/Lovelace: MD' }])
    fireEvent.click(screen.getByRole('button', { name: 'Save ada full name' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', { username: 'ada', fullName: 'Ada/Lovelace: MD' })
    })
    expect(await within(screen.getByTestId('person-profile')).findByText('Saved')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Save ada full name' })).toBeNull()

    fireEvent.change(screen.getByLabelText('ada full name'), { target: { value: '' } })
    fireEvent.keyDown(screen.getByLabelText('ada full name'), { key: 'Enter' })
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/full-name', { username: 'ada', fullName: '' })
    })
  })

  it('a failed save says so under the row, and the field keeps the edit', async () => {
    allowRoster([{ username: 'ada', fullName: 'Ada' }])
    h.daemonCliPost.mockRejectedValue(new Error('name too long'))
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    fireEvent.change(screen.getByLabelText('ada full name'), { target: { value: 'Ada L' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save ada full name' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('Couldn’t save: name too long. Nothing changed; try again.')
    expect((screen.getByLabelText('ada full name') as HTMLInputElement).value).toBe('Ada L')
  })

  it('What they can use: K2 on this server, each App by name and role, noted access, never the mailbox', async () => {
    allowFull()
    render(<PeopleSection />)
    await screen.findByRole('button', { name: 'Open ada' })
    // Everyone names the App once the list of Apps is read.
    await waitFor(() => expect(screen.getByTestId('summary-ada').textContent).toBe('K2 on this server'))
    await openPerson('ada')
    expect(screen.getByRole('switch', { name: 'K2 on this server for ada' }).getAttribute('aria-checked')).toBe('true')
    expect(screen.getByTestId('person-host').textContent).toContain('As reader: talks to the notes agent.')
    const shop = await screen.findByRole('switch', { name: 'Shop for ada' })
    expect(shop.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByTestId('person-app-svc-1').textContent).toContain('Off. As editor: talks to the sales agent when turned on.')
    expect(screen.queryByText(/mailbox/i)).toBeNull()
    expect(screen.queryByText('secret-room')).toBeNull()
    expect(screen.getByTestId('person-noted').textContent).toContain('Database: reports')
    expect(screen.getByTestId('person-noted').textContent).toContain('K2 doesn’t enforce these')
    // Turning an App back on is one switch, and says Saved.
    h.daemonCliPost.mockResolvedValue({})
    fireEvent.click(shop)
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/grants/enabled', { id: 'g1', enabled: true }))
    expect(await within(screen.getByTestId('person-app-svc-1')).findByText('Saved')).toBeTruthy()
  })

  it('turning off K2 on this server and removing an App each confirm first', async () => {
    allowFull()
    h.daemonCliPost.mockResolvedValue({})
    render(<PeopleSection />)
    await openPerson('ada')
    confirmSpy.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByRole('switch', { name: 'K2 on this server for ada' }))
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Turn off K2 on this server for ada?', destructive: true })))
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('switch', { name: 'K2 on this server for ada' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/grants/host', { username: 'ada', present: false }))

    fireEvent.click(await screen.findByRole('button', { name: 'More actions for Shop' }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Remove access…' }))
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Remove ada’s access to Shop?', confirmLabel: 'Remove access', destructive: true })))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/grants/delete', { id: 'g1' }))
  })

  it('Add an App: pick the App, then a role for it, and it posts one App grant', async () => {
    allowFull({ grants: [] })
    h.daemonCliPost.mockResolvedValue({ id: 'g-new' })
    render(<PeopleSection />)
    await openPerson('bob')
    expect(screen.getByRole('switch', { name: 'K2 on this server for bob' }).getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText('No Apps yet')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Add an App' }))
    const add = screen.getByTestId('person-add-app')
    const appPick = within(add).getByLabelText('App to add') as HTMLButtonElement
    await waitFor(() => expect(appPick.disabled).toBe(false))
    await waitForMenuOptions(appPick, (o) => expect(o.map((x) => x.label)).toEqual(['Shop']))
    expect(h.daemonCliGet).toHaveBeenCalledWith('publish/list', { project: 'ws-1' })
    const submit = within(add).getByRole('button', { name: 'Add App' }) as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    pickValue(appPick, 'svc-1')
    expect(menuOptions(within(add).getByLabelText('Role in the App')).map((o) => o.label)).toEqual(['reader', 'editor (Shop only)'])
    pickValue(within(add).getByLabelText('Role in the App'), 'role-2')
    expect(submit.disabled).toBe(false)
    fireEvent.click(submit)
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/grants', { subjectKind: 'principal', subjectId: 'p-bob', kind: 'app', targetId: 'svc-1', roleId: 'role-2' }),
    )
    await waitFor(() => expect(screen.queryByTestId('person-add-app')).toBeNull())
    expect(await within(screen.getByTestId('person-sign-in')).findByText('Saved')).toBeTruthy()
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
      boxLogin: false,
    }])
  })

  // agent-app-users T6.1: a tied, pending person shows the tag and the
  // reset action; an untied person shows none. A tied person's stored
  // email is never offered (verification review 2026-10-26, item 1).
  it('shows Waiting for password with its action only for a tied, pending person', async () => {
    allowRoster([
      { username: 'tina', pendingReset: true, tiedTo: 'sales', email: 'tina@example.test', storedEmailRefused: 'tied' },
      { username: 'owen', pendingReset: false, tiedTo: null },
    ])
    h.daemonCliPost.mockResolvedValue({ ok: true, delivered: 'sent', sentTo: 'tina@home.test', from: 'owner@box.test', ttlSecs: 86400 })
    render(<PeopleSection />)
    await waitFor(() => expect(screen.getByLabelText('Open tina')).toBeTruthy())
    expect(screen.getByLabelText('Open tina').textContent).toContain('Waiting for password')
    expect(screen.getByLabelText('Open owen').textContent).not.toContain('Waiting for password')

    await openPerson('owen')
    expect(screen.queryByTestId('person-tie')).toBeNull()

    await openPerson('tina')
    const tie = screen.getByTestId('person-tie')
    expect(tie.textContent).toContain('The sales agent set tina up')
    expect(tie.textContent).toContain('K2 never sends your reset there')
    fireEvent.click(await within(tie).findByRole('button', { name: 'Reset password…' }))
    const dialog = screen.getByTestId('reset-password-dialog')
    expect(dialog.textContent).toContain('The sales agent set tina up')
    fireEvent.click(screen.getByRole('radio', { name: /Email them the link/ }))
    const to = screen.getByLabelText('Email for tina') as HTMLInputElement
    expect(to.value).toBe('')
    expect(dialog.textContent).toContain('The email on file (tina@example.test) may have been entered by an agent')
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
      await openPerson('tess', 'Account')
      fireEvent.click(await screen.findByRole('button', { name: 'Reset password…' }))
      fireEvent.click(screen.getByRole('radio', { name: /Email them the link/ }))
      expect((screen.getByLabelText('Email for tess') as HTMLInputElement).value).toBe('')
      view.unmount()
    }
  })

  // 0.46.6: the daemon lets the Owner reset anyone and an Admin people who
  // only use Apps (never a box login); adding stays the Owner's. A Member
  // or an unknown role sees no password actions.
  it('shows password actions to the Owner and Admins, and Add person only to the Owner', async () => {
    for (const role of ['member', null] as const) {
      h.role = role
      allowRoster([{ username: 'ada', lockedUntil: 4102444800 }])
      const view = render(<PeopleSection />)
      await screen.findByLabelText('Open ada')
      // Let the role read settle before checking nothing appeared.
      await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('skin/roles'))
      await new Promise((r) => setTimeout(r, 0))
      expect(screen.queryByRole('button', { name: 'Add person' }), String(role)).toBeNull()
      await openPerson('ada', 'Account')
      expect(screen.queryByTestId('person-password'), String(role)).toBeNull()
      view.unmount()
    }
    h.role = 'owner'
    allowRoster([{ username: 'ada', boxLogin: true }])
    const owner = render(<PeopleSection />)
    expect(await screen.findByRole('button', { name: 'Add person' })).toBeTruthy()
    await openPerson('ada', 'Account')
    const card = await screen.findByTestId('person-password')
    expect(within(card).getByRole('button', { name: 'Reset password…' })).toHaveProperty('disabled', false)
    expect(within(card).getByRole('button', { name: 'Set password…' })).toHaveProperty('disabled', false)
    expect(within(card).queryByRole('button', { name: 'Unlock' })).toBeNull()
    expect(card.textContent, 'the owner resets a box login too').not.toContain(BOX_LOGIN_REASON)
    owner.unmount()
  })

  it('an Admin resets App-only users; a box login’s actions are off with the reason', async () => {
    h.role = 'admin'
    allowRoster([
      { username: 'ada', lockedUntil: 4102444800, boxLogin: false },
      { username: 'bo', lockedUntil: 4102444800, boxLogin: true },
    ])
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    let card = await screen.findByTestId('person-password')
    for (const item of ['Reset password…', 'Set password…', 'Unlock']) {
      expect(within(card).getByRole('button', { name: item }), item).toHaveProperty('disabled', false)
    }
    expect(card.textContent).not.toContain(BOX_LOGIN_REASON)
    fireEvent.click(within(card).getByRole('button', { name: 'Set password…' }))
    expect(screen.getByTestId('set-password-dialog')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    await openPerson('bo', 'Account')
    card = screen.getByTestId('person-password')
    for (const item of ['Reset password…', 'Set password…', 'Unlock']) {
      expect(within(card).getByRole('button', { name: item }), item).toHaveProperty('disabled', true)
    }
    expect(card.textContent).toContain(BOX_LOGIN_REASON)
    fireEvent.click(within(card).getByRole('button', { name: 'Reset password…' }))
    expect(screen.queryByTestId('reset-password-dialog'), 'a disabled button opens nothing').toBeNull()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    // Adding a person stays the Owner's (the daemon's POST skin/users floor).
    fireEvent.click(screen.getByTestId('people-everyone'))
    expect(screen.queryByRole('button', { name: 'Add person' })).toBeNull()
  })

  it('canResetPassword mirrors the daemon rule', () => {
    expect(canResetPassword('owner', { boxLogin: true })).toBe(true)
    expect(canResetPassword('owner', { boxLogin: false })).toBe(true)
    expect(canResetPassword('admin', { boxLogin: false })).toBe(true)
    expect(canResetPassword('admin', { boxLogin: true })).toBe(false)
    expect(canResetPassword(null, { boxLogin: false })).toBe(false)
    expect(parsePeople({ users: [{ username: 'x', boxLogin: true }, { username: 'y' }] }).map((p) => p.boxLogin)).toEqual([true, false])
  })

  it('Remove confirms, posts skin/users/remove and goes back to everyone with a note', async () => {
    allowRoster([{ username: 'ada' }, { username: 'bob' }])
    h.daemonCliPost.mockResolvedValue({ ok: true })
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    confirmSpy.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByRole('button', { name: 'Remove ada…' }))
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Remove ada?', destructive: true })))
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    allowRoster([{ username: 'bob' }])
    fireEvent.click(screen.getByRole('button', { name: 'Remove ada…' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/remove', { username: 'ada' }))
    expect(await screen.findByText('ada was removed. They can no longer sign in here.')).toBeTruthy()
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Open ada' })).toBeNull())
  })

  it('shows a one-time reset link once, and closing the sheet drops it', async () => {
    allowRoster([{ username: 'ada', email: 'ada@clinic.test' }])
    const link = 'https://box.example/cli/skin/password/set#k2inv_test-only'
    h.daemonCliPost.mockResolvedValue({ ok: true, delivered: 'returned', link, expiresAt: 4102444800, ttlSecs: 86400 })
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    fireEvent.click(screen.getByRole('button', { name: 'Reset password…' }))
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
    fireEvent.click(screen.getByRole('button', { name: 'Reset password…' }))
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
    await openPerson('ada', 'Account')
    fireEvent.click(screen.getByRole('button', { name: 'Reset password…' }))
    fireEvent.click(screen.getByRole('button', { name: 'Get link' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('This server’s web address is not safe for a reset page.')
    expect(screen.queryByTestId('one-time-secret')).toBeNull()
  })

  it('Set password: typed, or generated by the daemon and shown once', async () => {
    allowRoster([{ username: 'ada' }])
    h.daemonCliPost.mockResolvedValue({ username: 'ada', hasPassword: true, passwordMustChange: true })
    render(<PeopleSection />)
    await openPerson('ada', 'Account')
    fireEvent.click(screen.getByRole('button', { name: 'Set password…' }))
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
    expect((await screen.findByText(/Password set. Give it to ada yourself./)).textContent).toContain(MUST_CHANGE_NOTE)
    expect(screen.queryByTestId('one-time-secret')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))

    h.daemonCliPost.mockResolvedValue({ username: 'ada', hasPassword: true, generatedPassword: 'Test1-Only2-Fake3-Value', passwordMustChange: true })
    fireEvent.click(screen.getByRole('button', { name: 'Set password…' }))
    fireEvent.click(screen.getByRole('button', { name: 'Generate a password and set it' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/password', { username: 'ada', generate: true })
    })
    expect((await screen.findByTestId('one-time-secret')).textContent).toBe('Test1-Only2-Fake3-Value')
    expect(screen.getAllByText('Test1-Only2-Fake3-Value')).toHaveLength(1)
    expect(screen.getByTestId('set-password-dialog').textContent).toContain(MUST_CHANGE_NOTE)
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(document.body.textContent).not.toContain('Test1-Only2-Fake3-Value')
    fireEvent.click(screen.getByRole('button', { name: 'Set password…' }))
    expect((screen.getByLabelText('New password for ada') as HTMLInputElement).value).toBe('')
    expect(document.body.textContent).not.toContain('Test1-Only2-Fake3-Value')
  })

  it('Set password for an agent-set-up person says it keeps the agent’s hold and offers Reset', async () => {
    allowRoster([{ username: 'tina', tiedTo: 'sales', pendingReset: true }])
    render(<PeopleSection />)
    await openPerson('tina', 'Account')
    fireEvent.click(screen.getByRole('button', { name: 'Set password…' }))
    expect(screen.getByTestId('set-password-dialog').textContent).toContain('doesn’t lift that limit')
    fireEvent.click(screen.getByRole('button', { name: 'Reset password instead…' }))
    expect(screen.queryByTestId('set-password-dialog')).toBeNull()
    expect(screen.getByTestId('reset-password-dialog')).toBeTruthy()
  })

  it('offers Unlock only for a locked login, clears it and says so', async () => {
    allowRoster([{ username: 'lena', lockedUntil: 4102444800 }, { username: 'otto' }])
    h.daemonCliPost.mockResolvedValue({ ok: true, username: 'lena', cleared: true })
    render(<PeopleSection />)
    await screen.findByLabelText('Open lena')
    expect(screen.getByLabelText('Open lena').textContent).toContain('Locked')
    expect(screen.getByLabelText('Open otto').textContent).not.toContain('Locked')
    await openPerson('otto', 'Account')
    expect(screen.queryByRole('button', { name: 'Unlock' })).toBeNull()
    await openPerson('lena', 'Account')
    expect(screen.getByTestId('person-password').textContent).toContain('Too many wrong passwords')
    allowRoster([{ username: 'lena' }, { username: 'otto' }])
    fireEvent.click(screen.getByRole('button', { name: 'Unlock' }))
    await waitFor(() => {
      expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users/unlock', { username: 'lena' })
    })
    expect(await screen.findByText('lena is unlocked and can sign in again.')).toBeTruthy()
    await waitFor(() => expect(screen.getByLabelText('Open lena').textContent).not.toContain('Locked'))
  })

  it('Add person opens a sheet: a typed password, with full name and email', async () => {
    allowRoster([])
    h.daemonCliPost.mockResolvedValue({ username: 'carol' })
    render(<PeopleSection />)
    expect(screen.queryByLabelText('New user’s username')).toBeNull()
    fireEvent.click(await screen.findByRole('button', { name: 'Add person' }))
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

  it('Add person with a generated password closes the sheet and shows it once', async () => {
    allowRoster([])
    h.daemonCliPost.mockResolvedValue({ username: 'dan', generatedPassword: 'Test4-Only5-Fake6-Value', passwordMustChange: true })
    render(<PeopleSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Add person' }))
    fireEvent.change(screen.getByLabelText('New user’s username'), { target: { value: 'dan' } })
    fireEvent.click(screen.getByRole('radio', { name: /Let K2 generate one/ }))
    expect(screen.queryByLabelText('New user’s password')).toBeNull()
    allowRoster([{ username: 'dan' }])
    fireEvent.click(within(screen.getByTestId('add-user-dialog')).getByRole('button', { name: 'Add person' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('skin/users', { username: 'dan', generate: true }))
    expect(screen.queryByTestId('add-user-dialog')).toBeNull()
    expect((await screen.findByTestId('one-time-secret')).textContent).toBe('Test4-Only5-Fake6-Value')
    expect(screen.getByRole('button', { name: 'Copy password' })).toBeTruthy()
    expect(screen.getByTestId('added-user-dialog').textContent).toContain(MUST_CHANGE_NOTE)
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(document.body.textContent).not.toContain('Test4-Only5-Fake6-Value')
    expect(screen.getByLabelText('Open dan')).toBeTruthy()
  })

  it('Add person can send an invite link only with an email and an App', async () => {
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
    fireEvent.click(await screen.findByRole('button', { name: 'Add person' }))
    const invite = screen.getByRole('radio', { name: /Send them a link to set their own/ }) as HTMLInputElement
    expect(invite.disabled).toBe(true)
    expect(screen.getByTestId('add-user-dialog').textContent).toContain('Needs an email.')
    fireEvent.change(screen.getByLabelText('New user’s username'), { target: { value: 'eve' } })
    fireEvent.change(screen.getByLabelText('New user’s email'), { target: { value: 'eve@clinic.test' } })
    expect(invite.disabled).toBe(false)
    fireEvent.click(invite)
    // K2's themed menus; "Choose an App" is the placeholder, not an option.
    const appPick = screen.getByLabelText('Invite to App')
    expect(appPick.textContent).toBe('Choose an App')
    await waitForMenuOptions(appPick, (o) => expect(o).toEqual([{ value: 'svc-shop', label: 'shop (cmd)', disabled: false }]))
    const submit = screen.getByRole('button', { name: 'Add and send link' }) as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    pickValue(appPick, 'svc-shop')
    // The pick happened above the dialog: it is still open.
    expect(screen.getByTestId('add-user-dialog')).toBeTruthy()
    // Only this App's roles (and all-App ones).
    expect(menuOptions(screen.getByLabelText('Role in the App')).map((o) => o.label)).toEqual(['No role', 'staff'])
    pickValue(screen.getByLabelText('Role in the App'), 'role-staff')
    // Re-picking the same App keeps the role.
    pickValue(appPick, 'svc-shop')
    expect(screen.getByLabelText('Role in the App').textContent).toBe('staff')
    // Esc with a menu open closes the menu only.
    fireEvent.click(appPick)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()
    expect(screen.getByTestId('add-user-dialog')).toBeTruthy()
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
    fireEvent.click(screen.getByRole('button', { name: 'Add person' }))
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
