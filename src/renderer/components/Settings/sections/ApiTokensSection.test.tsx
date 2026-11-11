// @vitest-environment jsdom
// Settings → API Keys: the page (keys in a card, a "…" menu per key, the
// public-API-off banner, Owner only), the create dialog with the one-time
// key, revoke asking first, and the hire scope (0.45.6,
// prd-agent-hire-templates-v1 HT3/HT5/HT8): the DB door, "Hire agents" with
// limits, the owner confirm before the mint.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const settingsState = vi.hoisted(() => ({ apiEnabled: true, setSection: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))
const projectsState = vi.hoisted(() => ({ projects: [] as Array<{ name: string; path: string }> }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) => sel(projectsState),
}))
vi.mock('@/stores/settings', () => {
  const useSettingsStore = (sel: (s: unknown) => unknown) => sel(settingsState)
  ;(useSettingsStore as unknown as { getState: () => unknown }).getState = () => settingsState
  return { useSettingsStore }
})

import { ApiTokensSection, publicApiSection } from './ApiTokensSection'
import { escOwnedByDialog } from '../settings-esc'
import { menuLabels, menuOptions, pickValue, pickedValue, waitForMenuOptions } from '@/test-utils/setting-dropdown'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const HIRE_KEY = {
  id: 'key-1',
  label: 'jobs-app',
  scope: 'owner',
  createdAt: 1,
  revokedAt: null,
  disabledAt: null,
  keySet: true,
  anthropicKeySet: false,
  allowedWorkspaces: null,
  provider: null,
  baseUrl: null,
  capabilities: { hostSessions: false, canonicalMessage: false, sandboxes: false, db: false, hire: true },
  hireLimits: {
    maxMembersPerDay: 40,
    maxLiveMembers: 200,
    maxMembersPerTeam: 12,
    db: 'off',
    link: true,
    linkTargets: 'created',
    retire: true,
    reach: 'none',
    folder: 'jobs',
  },
}

beforeEach(() => {
  projectsState.projects = []
  settingsState.apiEnabled = true
  settingsState.setSection.mockReset()
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockResolvedValue({ keys: [HIRE_KEY] })
})

afterEach(() => cleanup())

const openMenu = async (name: string) => {
  fireEvent.click(await screen.findByRole('button', { name: `More actions for ${name}` }))
  return screen.getAllByRole('menuitem').map((m) => m.textContent ?? '')
}
const openCreate = async () => {
  fireEvent.click(await screen.findByRole('button', { name: 'Create API key' }))
  return screen.getByRole('dialog', { name: 'Create API key' })
}
const createKey = () => fireEvent.click(screen.getByRole('button', { name: 'Create key' }))

describe('ApiTokensSection page', () => {
  const PLAIN = {
    ...HIRE_KEY,
    id: 'key-plain',
    label: 'Support chat',
    createdAt: 1790000000,
    allowedWorkspaces: '["notes","web-app"]',
    capabilities: { hostSessions: true, canonicalMessage: true, sandboxes: false, db: false, hire: false },
    hireLimits: null,
  }
  const OFF = { ...PLAIN, id: 'key-off', label: 'Build server', disabledAt: 1791000000, allowedWorkspaces: '*' }
  const GONE = { ...PLAIN, id: 'key-gone', label: 'Trial', revokedAt: 1791000000 }

  it('says in plain words what each key reaches, and keeps revoked keys in their own card', async () => {
    daemonCliGet.mockResolvedValue({ keys: [PLAIN, OFF, GONE] })
    render(<ApiTokensSection />)
    expect((await screen.findByTestId('api-key-reach-key-plain')).textContent).toBe('Agent sessions and messages in notes and web-app.')
    expect(screen.getByTestId('api-key-reach-key-off').textContent).toBe('Agent sessions and messages in every workspace.')
    expect(screen.getByTestId('api-key-row-key-off').textContent).toMatch(/Off.*Turned off/)
    const revoked = screen.getByTestId('api-keys-revoked')
    expect(within(revoked).getByTestId('api-key-row-key-gone').textContent).toContain('Revoked')
    // A revoked key has no actions; the others keep theirs in a menu.
    expect(within(revoked).queryByRole('button', { name: /More actions/ })).toBeNull()
    expect(await openMenu('Support chat')).toEqual([
      expect.stringMatching(/^Turn off/),
      expect.stringMatching(/^Revoke…/),
    ])
    // No route or flag names on the page.
    expect(document.body.textContent).not.toMatch(/k2sk_|\/v1|host-sessions|canonical|Caps:/)
  })

  it('turns a key off from its menu and says what happened', async () => {
    daemonCliGet.mockResolvedValue({ keys: [PLAIN] })
    daemonCliPost.mockResolvedValue({ success: true })
    render(<ApiTokensSection />)
    await openMenu('Support chat')
    fireEvent.click(screen.getByRole('menuitem', { name: /Turn off/ }))
    expect((await screen.findByTestId('api-keys-notice')).textContent).toContain('Support chat is off.')
    expect(daemonCliPost).toHaveBeenCalledWith('primary', 'api-keys/disable', { id: 'key-plain' })
  })

  it('revoke asks first, says it is permanent, and only then revokes', async () => {
    daemonCliGet.mockResolvedValue({ keys: [PLAIN] })
    daemonCliPost.mockResolvedValue({ success: true })
    render(<ApiTokensSection />)
    await openMenu('Support chat')
    fireEvent.click(screen.getByRole('menuitem', { name: /Revoke/ }))
    const sheet = screen.getByTestId('revoke-confirm-dialog')
    expect(sheet.textContent).toContain('Revoke Support chat?')
    expect(sheet.textContent).toContain('can’t be turned back on')
    expect(daemonCliPost).not.toHaveBeenCalled()
    fireEvent.click(within(sheet).getByRole('button', { name: 'Revoke key' }))
    expect((await screen.findByTestId('api-keys-notice')).textContent).toContain('Support chat is revoked.')
    expect(daemonCliPost).toHaveBeenCalledWith('primary', 'api-keys/revoke', { id: 'key-plain' })
  })

  it('a failed revoke says so on the row, with the server’s reason', async () => {
    daemonCliGet.mockResolvedValue({ keys: [PLAIN] })
    daemonCliPost.mockRejectedValue(new Error('database is locked'))
    render(<ApiTokensSection />)
    await openMenu('Support chat')
    fireEvent.click(screen.getByRole('menuitem', { name: /Revoke/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Revoke key' }))
    const alert = await within(screen.getByTestId('api-key-row-key-plain')).findByRole('alert')
    expect(alert.textContent).toBe('Couldn’t revoke Support chat. It still works. Try again in a moment.The server said: database is locked.')
    expect(screen.queryByTestId('api-keys-notice')).toBeNull()
  })

  it('loading, failed and empty each use the shared card', async () => {
    daemonCliGet.mockReturnValue(new Promise(() => {}))
    const { unmount } = render(<ApiTokensSection />)
    expect(screen.getByTestId('api-keys-loading')).toBeTruthy()
    unmount()

    daemonCliGet.mockRejectedValueOnce(new Error('database is locked')).mockResolvedValue({ keys: [] })
    render(<ApiTokensSection />)
    const err = await screen.findByTestId('api-keys-error')
    expect(err.textContent).toContain('Couldn’t load your API keys')
    expect(err.textContent).toContain('The server said: database is locked.')
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    const empty = await screen.findByTestId('api-keys-empty')
    expect(empty.textContent).toContain('No API keys yet')
    expect(empty.textContent).not.toMatch(/e\.g\./)
    expect(within(empty).getByRole('button', { name: 'Create API key' })).toBeTruthy()
  })

  it('someone who isn’t an Owner sees why, not a failure', async () => {
    daemonCliGet.mockRejectedValue(new DaemonHttpError('Invalid or missing auth token', 403, null))
    render(<ApiTokensSection />)
    const card = await screen.findByTestId('api-keys-not-owner')
    expect(card.textContent).toContain('Only an Owner can manage API keys')
    expect(screen.queryByRole('button', { name: 'Create API key' })).toBeNull()
    expect(screen.queryByTestId('api-surface-off')).toBeNull()
  })
})

// PRD boot-status-ghost B6: a fleet monitor's status key reads this server's
// /boot-status only. It must not look like a full owner key with doors.
describe('ApiTokensSection status keys', () => {
  const STATUS_KEY = {
    ...HIRE_KEY,
    id: 'key-status',
    label: 'k2-watch',
    scope: 'status',
    capabilities: { hostSessions: false, canonicalMessage: false, sandboxes: false, db: false, hire: false },
    hireLimits: null,
  }

  it('labels a status key, says it reads status only, and offers Revoke only', async () => {
    daemonCliGet.mockResolvedValue({ keys: [STATUS_KEY, HIRE_KEY] })
    render(<ApiTokensSection />)
    const row = await screen.findByTestId('api-key-row-key-status')
    expect(row.textContent).toContain('k2-watch')
    expect(row.textContent).toContain('Status only')
    expect(row.textContent).toContain('Nothing else.')
    expect(await openMenu('k2-watch')).toEqual([expect.stringMatching(/^Revoke…/)])
    // The owner key next to it keeps its usual row.
    expect(screen.getByTestId('hire-limits-key-1')).toBeTruthy()
  })
})

describe('ApiTokensSection hire scope', () => {
  it('shows a hire key with its limits in a sentence', async () => {
    render(<ApiTokensSection />)
    const line = await screen.findByTestId('hire-limits-key-1')
    expect(line.textContent).toBe('Hires up to 40 agents a day (200 at once, 12 per team). May link into teams and retire the agents it hired.')
    // A key that only hires has no second line saying so.
    expect(screen.queryByTestId('api-key-reach-key-1')).toBeNull()
    expect(screen.queryByTestId('api-surface-off')).toBeNull()
  })

  it('when the public API is off, one banner says so and opens the setting', async () => {
    settingsState.apiEnabled = false
    render(<ApiTokensSection />)
    const banner = await screen.findByTestId('api-surface-off')
    expect(banner.textContent).toContain('The public API is off, so no key works yet.')
    // No breadcrumb sentence and no command line in the banner.
    expect(banner.textContent).not.toMatch(/→|k2 api/)
    fireEvent.click(within(banner).getByRole('button', { name: 'Open the Public API setting' }))
    expect(settingsState.setSection).toHaveBeenCalledWith('k2-connect')
  })

  it('opens Connected Servers for the switch under air-gap (Tunnel is hidden)', () => {
    expect(publicApiSection(true)).toBe('connections')
    expect(publicApiSection(false)).toBe('k2-connect')
  })

  it('confirms on the Mac, then mints a hire-only key with the limits', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-1' }
      if (route === 'api-keys/create') return { id: 'k2', key: 'k2sk_secret' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Jobs board' } })
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '5' } })
    fireEvent.click(screen.getByLabelText('Link its agents into teams'))
    fireEvent.change(screen.getByLabelText('Folder for their workspaces'), { target: { value: 'jobs' } })
    createKey()
    await waitFor(() => expect(screen.getByTestId('one-time-secret').textContent).toBe('k2sk_secret'))
    const routes = daemonCliPost.mock.calls.map((c) => c[1])
    expect(routes).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'api-keys/create'])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({ action: 'hire-key-mint' })
    expect(daemonCliPost.mock.calls[1][2]).toMatchObject({ action: 'hire-key-mint', method: 'local-auth' })
    const body = daemonCliPost.mock.calls[2][2] as Record<string, unknown>
    expect(body.label).toBe('Jobs board')
    expect(body.confirmToken).toBe('tok-1')
    expect(body.capabilities).toEqual({ hostSessions: false, canonicalMessage: false, sandboxes: false, db: false })
    expect(body.hire).toEqual({
      maxMembersPerDay: 5,
      maxLiveMembers: 100,
      maxMembersPerTeam: 12,
      db: 'off',
      link: true,
      retire: false,
      reach: 'none',
      folder: 'jobs',
    })
    // Done drops the secret and says the key is ready.
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText('k2sk_secret')).toBeNull()
    expect((await screen.findByTestId('api-keys-notice')).textContent).toContain('Jobs board is ready.')
  })

  it('asks for the vault password when there is no Touch ID', async () => {
    daemonCliPost.mockImplementation(async (_scope: string, route: string, body: Record<string, unknown>) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'vault/confirm/finish') {
        expect(body.password).toBe('correct horse battery')
        return { token: 'tok-2' }
      }
      if (route === 'api-keys/create') return { id: 'k3', key: 'k2sk_two' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    createKey()
    const pw = await screen.findByLabelText('Confirm password')
    fireEvent.change(pw, { target: { value: 'correct horse battery' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm and create' }))
    await waitFor(() => expect(screen.getByTestId('one-time-secret').textContent).toBe('k2sk_two'))
    expect((daemonCliPost.mock.calls[2][2] as Record<string, unknown>).confirmToken).toBe('tok-2')
  })

  it('refuses database access for hired agents without the key’s database access, before any confirm', async () => {
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    // K2's themed menu: read and write are there but off without the db door.
    expect(menuOptions(screen.getByLabelText('Database access for hired agents'))).toEqual([
      { value: 'off', label: 'None', disabled: false },
      { value: 'read', label: 'Read only', disabled: true },
      { value: 'write', label: 'Read and change', disabled: true },
    ])
    fireEvent.change(screen.getByLabelText('Agents per day'), { target: { value: '0' } })
    createKey()
    expect(await screen.findByText('Use a whole number, 1 or more.')).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a key that reaches workspaces needs at least one, before anything is sent', async () => {
    render(<ApiTokensSection />)
    await openCreate()
    createKey()
    expect(await screen.findByText('Pick at least one workspace, or choose every workspace.')).toBeTruthy()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    expect(await screen.findByText('Choose at least one thing this key can do.')).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a plain key mints without a confirm and sends the db door', async () => {
    daemonCliPost.mockResolvedValue({ id: 'k4', key: 'k2sk_plain' })
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Every workspace, including new ones'))
    fireEvent.click(screen.getByLabelText('Workspace databases'))
    createKey()
    await waitFor(() => expect(screen.getByTestId('one-time-secret').textContent).toBe('k2sk_plain'))
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    const body = daemonCliPost.mock.calls[0][2] as Record<string, unknown>
    expect(body.capabilities).toEqual({ hostSessions: true, canonicalMessage: false, sandboxes: false, db: true })
    expect(body.workspaces).toBe('*')
    expect(body.hire).toBeUndefined()
  })

  it('keeps the title above the scrolling form and the buttons below it', async () => {
    render(<ApiTokensSection />)
    const dialog = await openCreate()
    const head = within(dialog).getByTestId('dialog-head')
    const body = within(dialog).getByTestId('dialog-body')
    const foot = within(dialog).getByTestId('dialog-foot')
    expect(head.textContent).toContain('Create API key')
    expect(body.contains(head)).toBe(false)
    expect(body.className).toContain('overflow-y-auto')
    expect(within(foot).getByRole('button', { name: 'Create key' })).toBeTruthy()
    expect(within(foot).getByRole('button', { name: 'Cancel' })).toBeTruthy()
    expect(body.contains(within(foot).getByRole('button', { name: 'Create key' }))).toBe(false)
  })

  it('lists the first workspaces with Show all, never a scroll box of its own', async () => {
    projectsState.projects = Array.from({ length: 9 }, (_, i) => ({ name: `ws-${i + 1}`, path: `/x/ws-${i + 1}` }))
    render(<ApiTokensSection />)
    const dialog = await openCreate()
    const list = within(dialog).getByTestId('api-key-workspace-list')
    expect(list.className).not.toContain('overflow')
    expect(within(list).queryByLabelText('ws-6')).toBeNull()
    fireEvent.click(within(list).getByRole('button', { name: 'Show all 9 workspaces' }))
    fireEvent.click(within(list).getByLabelText('ws-9'))
    fireEvent.click(within(list).getByRole('button', { name: 'Show fewer' }))
    // A ticked workspace stays in sight when the list folds again.
    expect((within(list).getByLabelText('ws-9') as HTMLInputElement).checked).toBe(true)
    expect(within(list).queryByLabelText('ws-7')).toBeNull()
  })

  it('a failed create stays in the dialog with what failed and the server’s reason', async () => {
    daemonCliPost.mockRejectedValue(new Error('database is locked'))
    render(<ApiTokensSection />)
    const dialog = await openCreate()
    fireEvent.click(screen.getByLabelText('Every workspace, including new ones'))
    createKey()
    const note = await within(dialog).findByTestId('dialog-error')
    expect(note.textContent).toContain('Couldn’t create the key.')
    expect(note.textContent).toContain('The server said: database is locked.')
    expect(screen.getByRole('dialog', { name: 'Create API key' })).toBeTruthy()
  })
})

// ── App access limit (0.45.6, Rosson 2026-10-14) ─────────────────────────

const OPTIONS = {
  apps: [
    {
      id: 'svc-jobs',
      name: 'jobs',
      kind: 'skin',
      workspaceId: 'ws-portal',
      workspace: 'portal',
      roomIds: ['ws-portal', 'ws-a'],
      roles: [
        { id: 'role-member', name: 'Member', appId: 'svc-jobs', rooms: ['ws-a'] },
        { id: 'role-viewer', name: 'Viewer', appId: null, rooms: ['ws-a'] },
      ],
    },
  ],
  rooms: [
    { id: 'self', handle: 'self', name: "The hired agent's own workspace" },
    { id: 'ws-portal', handle: 'portal', name: 'Portal' },
    { id: 'ws-a', handle: 'job-a', name: 'Job A' },
    { id: 'ws-b', handle: 'job-b', name: 'Job B' },
  ],
}

const CEILING_KEY = {
  ...HIRE_KEY,
  id: 'key-c',
  label: 'jobs-ceiling',
  hireLimits: {
    ...HIRE_KEY.hireLimits,
    apps: [{ app: 'svc-jobs', rooms: ['self', 'ws-a', 'ws-b'], roles: ['role-member'] }],
  },
}

function routeGets(keys: unknown[]) {
  daemonCliGet.mockImplementation(async (_scope: string, route: string) => {
    if (route === 'api-keys/list') return { keys }
    if (route === 'api-keys/hire-ceiling-options') return OPTIONS
    throw new Error(`unexpected GET ${route}`)
  })
}

const confirmRequired = () =>
  new Error(JSON.stringify({ ok: false, error: { code: 'vault_confirm_required', hint: 'confirm first' } }))

describe('ApiTokensSection App access limit', () => {
  it('shows the limit in one line on the key row', async () => {
    routeGets([CEILING_KEY])
    render(<ApiTokensSection />)
    await waitFor(() =>
      expect(screen.getByTestId('hire-ceiling-key-c').textContent).toBe('App access: jobs · 3 rooms · up to Member'),
    )
  })

  it('says "No App access" for a hire key with an empty limit', async () => {
    routeGets([HIRE_KEY])
    render(<ApiTokensSection />)
    const line = await screen.findByTestId('hire-ceiling-key-1')
    expect(line.textContent).toBe('App access: No App access')
  })

  it('the create form starts empty and says agents get no App login', async () => {
    routeGets([])
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    const none = screen.getByTestId('app-access-none')
    expect(none.textContent).toContain('No App access: agents hired with this key get no App login.')
    expect(screen.getByLabelText('App').tagName).toBe('BUTTON')
    expect(document.querySelector('select')).toBeNull()
    expect(pickedValue(screen.getByLabelText('App'))).toBe('')
    expect(screen.queryByLabelText('Highest role')).toBeNull()
    // The menu sits above the dialog; Esc closes the menu, not the dialog.
    fireEvent.click(screen.getByLabelText('App'))
    expect(Number(screen.getByTestId('setting-dropdown-menu').style.zIndex)).toBeGreaterThan(80)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()
    expect(screen.getByRole('dialog', { name: 'Create API key' })).toBeTruthy()
  })

  it('mints a hire key with the App, rooms and highest role picked in the form', async () => {
    routeGets([])
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-c' }
      if (route === 'api-keys/create') return { id: 'k9', key: 'k2sk_ceiling' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    await waitForMenuOptions(screen.getByLabelText('App'), (o) => expect(o.map((x) => x.value)).toEqual(['', 'svc-jobs']))
    pickValue(screen.getByLabelText('App'), 'svc-jobs')
    expect((screen.getByLabelText('Room self') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(screen.getByLabelText('Room job-a'))
    // A re-pick of the same App keeps the rooms ticked.
    pickValue(screen.getByLabelText('App'), 'svc-jobs')
    expect((screen.getByLabelText('Room job-a') as HTMLInputElement).checked).toBe(true)
    // The pick happened inside the dialog: it is still open.
    expect(screen.getByRole('dialog', { name: 'Create API key' })).toBeTruthy()
    const role = screen.getByLabelText('Highest role')
    expect(menuLabels(role)).toEqual([
      'No role (Thread only in these rooms)',
      'Member',
      'Viewer (all Apps)',
    ])
    pickValue(role, 'role-member')
    createKey()
    await waitFor(() => expect(screen.getByTestId('one-time-secret').textContent).toBe('k2sk_ceiling'))
    const body = daemonCliPost.mock.calls[2][2] as { hire: Record<string, unknown>; confirmToken: string }
    expect(body.confirmToken).toBe('tok-c')
    expect(body.hire.apps).toEqual([{ app: 'svc-jobs', rooms: ['self', 'ws-a'], roles: ['role-member'] }])
  })

  it('refuses an App with no rooms before any confirm', async () => {
    routeGets([])
    render(<ApiTokensSection />)
    await openCreate()
    fireEvent.click(screen.getByLabelText('Run agent sessions'))
    fireEvent.click(screen.getByLabelText('Hire agents'))
    await waitForMenuOptions(screen.getByLabelText('App'), (o) => expect(o.map((x) => x.value)).toEqual(['', 'svc-jobs']))
    pickValue(screen.getByLabelText('App'), 'svc-jobs')
    fireEvent.click(screen.getByLabelText('Room self'))
    createKey()
    expect(await screen.findByText('Pick at least one room, or choose No App access.')).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('lowering to "No App access" saves without a confirm', async () => {
    routeGets([CEILING_KEY])
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'api-keys/hire-ceiling') return { id: 'key-c', change: 'lower', confirmed: false, hireLimits: {} }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openMenu('jobs-ceiling')
    fireEvent.click(screen.getByRole('menuitem', { name: /App access/ }))
    const dialog = screen.getByTestId('edit-app-access')
    await waitFor(() => expect(dialog.textContent).toContain('Now: jobs · 3 rooms · up to Member'))
    expect(pickedValue(screen.getByLabelText('App'))).toBe('svc-jobs')
    expect((screen.getByLabelText('Room job-b') as HTMLInputElement).checked).toBe(true)
    expect(pickedValue(screen.getByLabelText('Highest role'))).toBe('role-member')
    pickValue(screen.getByLabelText('App'), '')
    expect(screen.getByTestId('edit-app-access')).toBeTruthy()
    expect(screen.getByTestId('app-access-none').textContent).toContain('get no App login')
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(daemonCliPost.mock.calls.map((c) => c[1])).toEqual(['api-keys/hire-ceiling'])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({ id: 'key-c', apps: [] })
  })

  it('raising asks for the owner confirm on the Mac, then saves with the token', async () => {
    routeGets([CEILING_KEY])
    let saves = 0
    daemonCliPost.mockImplementation(async (_scope: string, route: string) => {
      if (route === 'api-keys/hire-ceiling') {
        saves += 1
        if (saves === 1) throw confirmRequired()
        return { id: 'key-c', change: 'raise', confirmed: true, hireLimits: {} }
      }
      if (route === 'vault/confirm/start') return { methods: ['local-auth'] }
      if (route === 'vault/confirm/finish') return { token: 'tok-raise' }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openMenu('jobs-ceiling')
    fireEvent.click(screen.getByRole('menuitem', { name: /App access/ }))
    fireEvent.click(await screen.findByLabelText('Room portal'))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(daemonCliPost.mock.calls.map((c) => c[1])).toEqual([
      'api-keys/hire-ceiling',
      'vault/confirm/start',
      'vault/confirm/finish',
      'api-keys/hire-ceiling',
    ])
    expect(daemonCliPost.mock.calls[0][2]).toEqual({
      id: 'key-c',
      apps: [{ app: 'svc-jobs', rooms: ['self', 'ws-a', 'ws-b', 'ws-portal'], roles: ['role-member'] }],
    })
    expect(daemonCliPost.mock.calls[1][2]).toEqual({ action: 'hire-key-mint' })
    expect(daemonCliPost.mock.calls[2][2]).toMatchObject({ action: 'hire-key-mint', method: 'local-auth' })
    expect(daemonCliPost.mock.calls[3][2]).toMatchObject({ id: 'key-c', confirmToken: 'tok-raise' })
  })

  it('raising without Touch ID asks for the password; a wrong one saves nothing and keeps the dialog', async () => {
    routeGets([CEILING_KEY])
    let saves = 0
    daemonCliPost.mockImplementation(async (_scope: string, route: string, body: Record<string, unknown>) => {
      if (route === 'api-keys/hire-ceiling') {
        saves += 1
        if (saves === 1) throw confirmRequired()
        expect(body.confirmToken).toBe('tok-pw')
        return { id: 'key-c', change: 'raise', confirmed: true, hireLimits: {} }
      }
      if (route === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (route === 'vault/confirm/finish') {
        if (body.password !== 'right one') {
          throw new Error(JSON.stringify({ error: { code: 'bad_password', hint: 'That password is not right.' } }))
        }
        return { token: 'tok-pw' }
      }
      throw new Error(`unexpected ${route}`)
    })
    render(<ApiTokensSection />)
    await openMenu('jobs-ceiling')
    fireEvent.click(screen.getByRole('menuitem', { name: /App access/ }))
    pickValue(await screen.findByLabelText('Highest role'), 'role-viewer')
    fireEvent.click(screen.getByLabelText('Room portal'))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    const pw = await screen.findByLabelText('Confirm password')
    fireEvent.change(pw, { target: { value: 'wrong' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm and save' }))
    expect((await screen.findByTestId('dialog-error')).textContent).toBe(
      'Couldn’t confirm it’s you.Nothing was saved. Check the password, then try again.The server said: That password is not right.',
    )
    expect(saves).toBe(1)
    expect(screen.getByTestId('edit-app-access')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Confirm password'), { target: { value: 'right one' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm and save' }))
    await waitFor(() => expect(screen.queryByTestId('edit-app-access')).toBeNull())
    expect(saves).toBe(2)
  })
})

describe('Esc in the Create API key dialog (0.45.6 Linux check)', () => {
  // Settings closes on Escape from a window listener registered before any
  // dialog inside it opens. Same order here, same guard as Settings.tsx.
  let settingsClose: ReturnType<typeof vi.fn>
  let settingsEsc: (e: KeyboardEvent) => void
  beforeEach(() => {
    settingsClose = vi.fn()
    settingsEsc = (e) => {
      if (e.key !== 'Escape' || escOwnedByDialog(e)) return
      settingsClose()
    }
    window.addEventListener('keydown', settingsEsc)
  })
  afterEach(() => window.removeEventListener('keydown', settingsEsc))

  const esc = () => fireEvent.keyDown(window, { key: 'Escape' })

  it('one Esc closes the dialog only; the next closes Settings', async () => {
    render(<ApiTokensSection />)
    await openCreate()
    esc()
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Create API key' })).toBeNull())
    expect(settingsClose).not.toHaveBeenCalled()
    esc()
    expect(settingsClose).toHaveBeenCalledTimes(1)
  })

  it('Settings stands down even when the dialog listener ran first', () => {
    const ev = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })
    ev.preventDefault()
    expect(escOwnedByDialog(ev)).toBe(true)
    const plain = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })
    expect(escOwnedByDialog(plain)).toBe(false)
  })

  it('Settings.tsx checks the guard before closing', () => {
    const src = readFileSync(resolve(__dirname, '../Settings.tsx'), 'utf8')
    const at = src.indexOf("if (e.key === 'Escape' && !searchOpen) {")
    expect(at).toBeGreaterThan(-1)
    const guard = src.indexOf('if (escOwnedByDialog(e)) return', at)
    const close = src.indexOf('closeSettings()', at)
    expect(guard).toBeGreaterThan(at)
    expect(guard).toBeLessThan(close)
  })
})
