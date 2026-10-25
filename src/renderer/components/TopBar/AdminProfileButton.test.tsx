// @vitest-environment jsdom
//
// Rosson 2026-10-16: an "Admin profile" icon in the top bar, right of the
// Zen toggle; a click opens a read-only modal with who this window is
// signed in as. Two sign-in cases: the owner key on this computer, and a
// Connect login from another device. Every value comes from existing
// daemon routes (mocked here).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  label: 'main',
  routes: {} as Record<string, unknown>,
  calls: [] as string[],
  scope: {
    label: 'This computer',
    isRemote: false,
    host: null as null | Record<string, unknown>,
  },
  login: vi.fn(),
  remember: vi.fn(),
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  postRoutes: {} as Record<string, (body: Record<string, unknown>) => unknown>,
  settingsUpdates: [] as Array<Record<string, unknown>>,
  // Your phones (phone-web-off): the pairing API, as the daemon answers it.
  phoneCalls: [] as Array<{ mine: boolean; scope?: unknown }>,
  pairCalls: [] as Array<Record<string, unknown>>,
  phones: null as unknown,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/components/Settings/shared/phone-pairing-api', async (orig) => {
  const real = await orig<typeof import('@/components/Settings/shared/phone-pairing-api')>()
  return {
    ...real,
    fetchPairedPhones: vi.fn(async (opts: { mine: boolean; scope?: unknown }) => {
      h.phoneCalls.push(opts)
      return { ok: true, value: real.parseDevices(h.phones) }
    }),
    makePairCode: vi.fn(async (opts: Record<string, unknown>) => {
      h.pairCalls.push(opts)
      return {
        ok: true,
        value: real.parsePairCode({ code: 'K2P-AAAAA-BBBBB-CCCCC-DDDDD', user: 'rosson-phone', expiresAt: new Date(Date.now() + 600_000).toISOString(), expiresInSecs: 600, host: null, pairUrl: null }),
      }
    }),
  }
})
// `/cli/settings/update`: where the settings store saves the owner's
// display name (Settings → General uses the same store action).
vi.mock('@/lib/daemon-settings', async (orig) => {
  const real = await orig<typeof import('@/lib/daemon-settings')>()
  return {
    ...real,
    settingsUpdate: vi.fn(async (updates: Record<string, unknown>) => {
      h.settingsUpdates.push(updates)
      return { ownerDisplayName: updates.ownerDisplayName }
    }),
  }
})
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: h.label }) }))
vi.mock('@/components/Timer/TimerButton', () => ({ default: () => <span data-stub="timer" /> }))
vi.mock('@/components/Timer/UsageButton', () => ({ default: () => <span data-stub="usage" /> }))
vi.mock('@/components/Timer/KeepAwakeButton', () => ({ default: () => <span data-stub="keep-awake" /> }))
vi.mock('@/components/Help/HelpButton', () => ({ default: () => <span data-stub="help" /> }))
vi.mock('@/components/Presence/PresenceRoster', () => ({ default: () => <span data-stub="presence" /> }))
vi.mock('@/components/Presence/ModeToggle', () => ({ default: () => <span data-stub="mode" /> }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    h.calls.push(route)
    if (!(route in h.routes)) throw new Error(`unexpected route ${route}`)
    const v = h.routes[route]
    if (v instanceof Error) throw v
    return v
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    h.posts.push({ route, body })
    const fn = h.postRoutes[route]
    if (!fn) throw new Error(`unexpected POST ${route}`)
    return fn(body)
  }),
}))
vi.mock('@/stores/connect-host', async (orig) => {
  const real = await orig<typeof import('@/stores/connect-host')>()
  return { ...real, loginToHost: h.login, saveRememberedPassword: h.remember }
})
vi.mock('@/kessel/server-scope', async (orig) => {
  const real = await orig<typeof import('@/kessel/server-scope')>()
  const scope = {
    get label() {
      return h.scope.label
    },
    get isRemote() {
      return h.scope.isRemote
    },
    connectHost: () => h.scope.host,
  }
  return { ...real, primaryScope: () => scope }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import TopBarUtilities from './TopBarUtilities'
import AdminProfileButton from './AdminProfileButton'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { useSettingsStore } from '@/stores/settings'
import { CHANGE_PASSWORD_COPY } from '@/lib/password-rotation'
import { PASSWORD_CHANGED_NOTICE, ownerPasswordChangedNotice } from './AdminProfileButton'
import { COLOR_NEEDS_NEWER_SERVER, HEX_FORMAT_HINT } from '@/components/Presence/PersonColorPicker'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const USERS = {
  users: [
    { username: 'rosson-phone', createdAt: '2026-08-01T12:00:00Z', role: 'admin', disabled: false, wasViewer: false },
    { username: 'sam', createdAt: '2026-09-02T12:00:00Z', role: 'member', disabled: false, wasViewer: false },
  ],
}

// Oldest first, as the daemon sends them.
const AUDIT = {
  tail: 200,
  events: [
    { ts: '2026-10-10T09:00:00Z', event: 'login', user: 'rosson-phone', outcome: 'ok', ingress: 'tunnel', ip: '10.0.0.5' },
    { ts: '2026-10-11T09:00:00Z', event: 'login', user: 'sam', outcome: 'ok', ingress: 'tunnel', ip: '10.0.0.9' },
    { ts: '2026-10-12T09:00:00Z', event: 'change-password', user: 'rosson-phone', outcome: 'ok', ingress: 'tunnel', ip: '-' },
    { ts: '2026-10-13T09:00:00Z', event: 'login', user: 'rosson-phone', outcome: 'bad_password', ingress: 'lan', ip: '192.168.1.7' },
  ],
}

function ownerRoutes(): Record<string, unknown> {
  return {
    'auth/whoami': { owner: true, username: null, role: 'owner', mustChangePassword: false },
    'owner/accounts': { accounts: ['rosson-phone'], aliases: ['rosson-phone'] },
    users: USERS,
    'users/audit': AUDIT,
    'tunnel/status': { running: true, public_url: 'https://rosson.k2.dev', frpc_installed: true },
    'people/color': colorView('owner', null, '#8c62c4'),
    'people/name': nameView('owner', 'Rosson', 'Long'),
  }
}

function nameView(person: string, firstName: string | null, lastName: string | null): Record<string, unknown> {
  const real = [firstName, lastName].filter(Boolean).join(' ')
  return { ok: true, person, firstName, lastName, displayName: real || (person === 'owner' ? 'Owner' : person), names: {} }
}

const PALETTE = ['#3a9e96', '#3a87a8', '#6a6fc8', '#8c62c4', '#b25cb0', '#c45a86', '#9b7a95', '#64778f']

function colorView(person: string, chosen: string | null, color: string): Record<string, unknown> {
  return { ok: true, palette: PALETTE, person, color, chosen, colors: { [person]: color } }
}

function dialog(): HTMLElement {
  const el = document.querySelector('[data-testid="admin-profile-dialog"]')
  if (!(el instanceof HTMLElement)) throw new Error('admin profile dialog not open')
  return el
}

function text(testId: string): string {
  const el = document.querySelector(`[data-testid="${testId}"]`)
  if (!el) throw new Error(`no ${testId}`)
  return el.textContent ?? ''
}

function button(): HTMLElement {
  const el = document.querySelector('[data-admin-profile]')
  if (!(el instanceof HTMLElement)) throw new Error('no Admin profile button')
  return el
}

async function openLoaded(): Promise<void> {
  act(() => void fireEvent.click(button()))
  await waitFor(() => expect(text('admin-profile-role')).not.toBe(''))
}

beforeEach(() => {
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  h.label = 'main'
  __resetZenAvailableForTests()
  h.calls = []
  h.posts = []
  h.postRoutes = {}
  h.settingsUpdates = []
  h.phoneCalls = []
  h.pairCalls = []
  h.phones = { ok: true, devices: [], pairAs: { user: 'rosson-phone', source: 'owner-linked', needsPassword: false, choices: ['rosson-phone', 'sam'] } }
  h.routes = ownerRoutes()
  h.scope = { label: 'This computer', isRemote: false, host: null }
  useSettingsStore.setState({ settingsOpen: false, ownerDisplayName: 'Rosson Long' })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

/** The saved server a Connect login signed in to from this device. */
function remoteHost(remember = true): Record<string, unknown> {
  return {
    id: 'host-rosson',
    label: 'Rosson’s Mac',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'sess-old',
    remember,
    username: 'rosson-phone',
  }
}

function connectAdmin(remember = true): void {
  h.scope = { label: 'Rosson’s Mac', isRemote: true, host: remoteHost(remember) }
  h.routes['auth/whoami'] = { owner: false, username: 'rosson-phone', role: 'admin', mustChangePassword: false }
  h.routes['people/color'] = colorView('owner', null, '#8c62c4')
  delete h.routes['users/audit']
}

describe('the Admin profile modal: Your phones (phone-web-off)', () => {
  it('the owner on this computer sees their own phones and pairs without a password or a login picker', async () => {
    h.phones = {
      ok: true,
      devices: [{ id: 's1', username: 'rosson-phone', deviceId: 'ios', name: 'Rosson iPhone', createdAt: '2026-10-24T10:00:00Z', expiresAt: '2026-11-23T10:00:00Z', current: false }],
      pairAs: { user: 'rosson-phone', source: 'owner-linked', needsPassword: false, choices: ['rosson-phone', 'sam'] },
    }
    render(<AdminProfileButton />)
    await openLoaded()
    const section = await waitFor(() => screen.getByTestId('admin-profile-phones'))
    await waitFor(() => expect(section.textContent).toContain('Rosson iPhone'))
    expect(h.phoneCalls[0]?.mine, 'own phones only').toBe(true)
    expect(within(section).queryByLabelText('Pairs as')).toBeNull()
    expect(within(section).queryByLabelText('Your password')).toBeNull()
    act(() => void fireEvent.click(within(section).getByRole('button', { name: 'Pair a phone' })))
    await waitFor(() => expect(section.textContent).toContain('K2P-AAAAA-BBBBB-CCCCC-DDDDD'))
    expect(h.pairCalls[0]).toMatchObject({ password: undefined, user: undefined })
  })

  it('a Connect login types its password before a code is made', async () => {
    connectAdmin()
    h.phones = { ok: true, devices: [], pairAs: { user: 'rosson-phone', source: 'self', needsPassword: true } }
    render(<AdminProfileButton />)
    await openLoaded()
    const section = await waitFor(() => screen.getByTestId('admin-profile-phones'))
    const pw = await within(section).findByLabelText('Your password')
    const btn = within(section).getByRole('button', { name: 'Pair a phone' }) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    act(() => void fireEvent.change(pw, { target: { value: 'my-password' } }))
    act(() => void fireEvent.click(btn))
    await waitFor(() => expect(h.pairCalls).toHaveLength(1))
    expect(h.pairCalls[0]).toMatchObject({ password: 'my-password', user: undefined })
  })
})

describe('the Admin profile button in the top bar', () => {
  it('sits immediately right of the Zen toggle and matches its look', () => {
    render(
      <TopBarUtilities>
        <button data-drawer="left" />
      </TopBarUtilities>,
    )
    const zen = document.querySelector('[data-zen-enter]')
    if (!(zen instanceof HTMLElement)) throw new Error('no Zen toggle')
    expect(zen.nextElementSibling).toBe(button())
    expect(button().parentElement?.lastElementChild).toBe(button())
    expect([button().getAttribute('aria-label'), button().getAttribute('title')]).toEqual(['Admin profile', 'Admin profile'])
    for (const cls of ['h-6', 'w-6', 'cursor-pointer', 'hover:bg-[var(--color-bg-elevated)]', 'text-[var(--color-text-secondary)]']) {
      expect([cls, button().classList.contains(cls), zen.classList.contains(cls)]).toEqual([cls, true, true])
    }
    // Icon style A3: Lucide `circle-user`, squared, at the one 16px.
    const icon = button().querySelector('svg[data-k2-icon="admin"]')
    if (!icon) throw new Error('no A3 admin icon')
    expect([icon.getAttribute('width'), icon.getAttribute('stroke-linecap'), icon.getAttribute('aria-hidden')]).toEqual([
      '16',
      'square',
      'true',
    ])
    expect(icon.querySelectorAll('circle')).toHaveLength(2)
    expect(icon.querySelector('path')?.getAttribute('d')).toBe('M7 20.66V17h10v3.66')
  })

  it('is absent where the Zen toggle is (Windows, Focus windows)', () => {
    Object.defineProperty(window.navigator, 'platform', { value: 'Win32', configurable: true })
    __resetZenAvailableForTests()
    render(<TopBarUtilities />)
    expect(document.querySelector('[data-admin-profile]')).toBeNull()
    cleanup()
    Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
    h.label = 'focus-abc'
    __resetZenAvailableForTests()
    render(<TopBarUtilities />)
    expect(document.querySelector('[data-admin-profile]')).toBeNull()
  })
})

describe('the Admin profile modal: open and close', () => {
  it('opens on click; Esc, the close button and a click outside close it; focus goes back', async () => {
    render(<AdminProfileButton />)
    expect(document.querySelector('[data-testid="admin-profile-dialog"]')).toBeNull()

    await openLoaded()
    expect(dialog().getAttribute('role')).toBe('dialog')
    expect(dialog().getAttribute('aria-modal')).toBe('true')
    expect(button().getAttribute('aria-expanded')).toBe('true')
    expect(document.activeElement).toBe(screen.getByTestId('admin-profile-close'))

    act(() => void fireEvent.keyDown(window, { key: 'Escape' }))
    expect(document.querySelector('[data-testid="admin-profile-dialog"]')).toBeNull()
    expect(document.activeElement).toBe(button())

    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-close')))
    expect(document.querySelector('[data-testid="admin-profile-dialog"]')).toBeNull()

    await openLoaded()
    // A click inside does not close it.
    act(() => void fireEvent.mouseDown(dialog()))
    dialog()
    act(() => void fireEvent.mouseDown(screen.getByTestId('admin-profile-scrim')))
    expect(document.querySelector('[data-testid="admin-profile-dialog"]')).toBeNull()
  })

  it('traps focus: Tab from the last control wraps to the first, Shift+Tab from the first to the last', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    const first = screen.getByTestId('admin-profile-close')
    // The collapsed Security header sits last (secondary things at the bottom).
    const last = screen.getByTestId('admin-profile-security-toggle')
    last.focus()
    act(() => void fireEvent.keyDown(window, { key: 'Tab' }))
    expect(document.activeElement).toBe(first)
    act(() => void fireEvent.keyDown(window, { key: 'Tab', shiftKey: true }))
    expect(document.activeElement).toBe(last)
    // Focus that escaped the dialog is pulled back in.
    button().focus()
    act(() => void fireEvent.keyDown(window, { key: 'Tab' }))
    expect(document.activeElement).toBe(first)
  })

  it('has no "go to Settings" hint anywhere, for the owner (with and without an account) or a Connect login', async () => {
    // Rosson 2026-10-17: edit here, don't say where things live.
    const noHints = (): void => {
      const t = dialog().textContent ?? ''
      expect(t).not.toMatch(/Settings/)
      expect(t).not.toMatch(/Pick your account/)
      expect(t).not.toContain('→')
      expect(document.querySelector('[data-testid="admin-profile-open-settings"]')).toBeNull()
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    noHints()
    cleanup()

    h.routes['owner/accounts'] = { accounts: [], aliases: [] }
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    noHints()
    cleanup()

    connectAdmin()
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    noHints()
  })
})

describe('the Admin profile modal: who you are', () => {
  it('the owner with an account picked: a header with the name, username, role and date; the editable rows; sign-ins', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    // Header: the owner's name, the picked login, its role, how/where.
    await waitFor(() => expect(text('admin-profile-name')).toBe('Rosson Long'))
    expect(text('admin-profile-username')).toBe('rosson-phone')
    expect(text('admin-profile-role')).toBe('Admin')
    expect(text('admin-profile-how')).toContain('As the owner, on this computer')
    expect(text('admin-profile-server')).toContain('This computer')
    expect(text('admin-profile-server')).toContain('https://rosson.k2.dev')
    expect(text('admin-profile-created')).toContain('2026')
    expect(screen.getByTestId('admin-profile-header').querySelector('[data-presence-chip]')).toBeTruthy()

    // Rows, in order: Colour (top), First name, Last name, Your account.
    const labels = Array.from(screen.getByTestId('admin-profile-fields').querySelectorAll('[data-testid^="admin-profile-"] > span:first-child')).map(
      (s) => s.textContent,
    )
    expect(labels).toEqual(['Colour', 'First name', 'Last name', 'Your account'])
    // One avatar in the whole modal: the header chip (the picker has none).
    expect(dialog().querySelectorAll('[data-presence-chip]').length).toBe(1)
    expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('Rosson')
    expect((screen.getByLabelText('Last name') as HTMLInputElement).value).toBe('Long')
    expect((screen.getByLabelText('Your account') as HTMLButtonElement).textContent).toContain('rosson-phone')
    expect(dialog().textContent).not.toContain('sam')

    // Change password sits above Security; Security starts collapsed.
    const pw = screen.getByTestId('admin-profile-password')
    const security = screen.getByTestId('admin-profile-security')
    expect(pw.compareDocumentPosition(security) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    const toggle = screen.getByTestId('admin-profile-security-toggle')
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(document.querySelector('[data-testid="admin-profile-sign-in"]')).toBeNull()
    act(() => void fireEvent.click(toggle))
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(text('admin-profile-sign-ins')).toContain('Recent sign-ins')

    // Only rosson-phone's sign-in events, newest first; not sam's, not the
    // password change.
    const rows = Array.from(document.querySelectorAll('[data-testid="admin-profile-sign-in"]')).map((r) => r.textContent ?? '')
    expect(rows.length).toBe(2)
    expect(rows[0]).toContain('Failed (bad_password)')
    expect(rows[0]).toContain('192.168.1.7')
    expect(rows[0]).toContain('Local network')
    expect(rows[1]).toContain('Signed in')
    expect(rows[1]).toContain('rosson-phone')
    expect(rows[1]).toContain('K2 Connect')
    // A grid: one header row with these columns; each row has one cell per column.
    const grid = screen.getByTestId('admin-profile-sign-in-grid')
    expect(Array.from(grid.querySelectorAll('[role="columnheader"]')).map((c) => c.textContent)).toEqual([
      'Time',
      'Login',
      'Client',
      'IP',
      'Via',
      'Result',
    ])
    for (const r of grid.querySelectorAll('[data-testid="admin-profile-sign-in"]')) {
      expect(r.querySelectorAll('[role="cell"]').length).toBe(6)
    }
    expect([...h.calls].sort()).toEqual(['auth/whoami', 'owner/accounts', 'people/color', 'people/name', 'tunnel/status', 'users', 'users/audit'])
  })

  it('the owner with no account picked: the owner name and role, Your account on None, no password, no hint', async () => {
    h.routes['owner/accounts'] = { accounts: [], aliases: [] }
    // No linked account: the owner has no names; the daemon says "Owner".
    h.routes['people/name'] = nameView('owner', null, null)
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-name')).toBe('Owner')
    expect(text('admin-profile-role')).toBe('Owner')
    // The name fields wait for a pick.
    for (const label of ['First name', 'Last name']) {
      const input = screen.getByLabelText(label) as HTMLInputElement
      expect([input.disabled, input.placeholder]).toEqual([true, 'Pick Your account first'])
    }
    expect(document.querySelector('[data-testid="admin-profile-username"]')).toBeNull()
    expect((screen.getByLabelText('Your account') as HTMLButtonElement).textContent).toContain('None')
    expect(document.querySelector('[data-testid="admin-profile-sign-ins"]')).toBeNull()
    expect(document.querySelector('[data-testid="admin-profile-change-password"]')).toBeNull()
  })

  it('a Connect admin from another device: the owner\'s names (it is the owner\'s account), username, Admin, the tag; no owner-only fields', async () => {
    connectAdmin()
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect(text('admin-profile-name')).toBe('Rosson Long'))
    expect(text('admin-profile-username')).toBe('rosson-phone')
    expect(text('admin-profile-role')).toBe('Admin')
    expect(text('admin-profile-owners-account')).toBe('Owner\'s account')
    expect(text('admin-profile-how')).toContain('With a Connect login, from this device')
    expect(text('admin-profile-server')).toContain('Rosson’s Mac')
    expect(text('admin-profile-server')).toContain('rosson.k2.dev')
    expect(text('admin-profile-created')).toContain('2026')
    expect(document.querySelector('[data-testid="admin-profile-display-name"]')).toBeNull()
    expect(document.querySelector('[data-testid="admin-profile-your-account"]')).toBeNull()
    expect(document.querySelector('[data-testid="admin-profile-sign-ins"]')).toBeNull()
    expect(dialog().textContent).not.toContain('sam')
    expect(h.calls).not.toContain('users/audit')
  })

  it('a Connect member sees only their own editable fields: names, Colour, Password; only whoami, tunnel, colour and names are read', async () => {
    h.scope = { label: 'Rosson’s Mac', isRemote: true, host: { ...remoteHost(), username: 'sam' } }
    h.routes = {
      'auth/whoami': { owner: false, username: 'sam', role: 'member', mustChangePassword: false },
      'tunnel/status': { running: false, frpc_installed: true },
      'people/color': colorView('sam', null, '#3a9e96'),
      'people/name': nameView('sam', null, null),
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    expect(text('admin-profile-name')).toBe('sam')
    expect(text('admin-profile-role')).toBe('Member')
    const labels = Array.from(screen.getByTestId('admin-profile-fields').querySelectorAll('[data-testid^="admin-profile-"] > span:first-child')).map(
      (s) => s.textContent,
    )
    expect(labels).toEqual(['Colour', 'First name', 'Last name'])
    expect(document.querySelector('[data-testid="admin-profile-owners-account"]')).toBeNull()
    expect(screen.queryByLabelText('Your name')).toBeNull()
    expect((screen.getByLabelText('First name') as HTMLInputElement).disabled).toBe(false)
    expect(screen.queryByLabelText('Your account')).toBeNull()
    expect(text('admin-profile-password')).toContain('Change password')
    // No "only an admin can see this" filler.
    expect(dialog().textContent).not.toMatch(/can see this/)
    expect([...h.calls].sort()).toEqual(['auth/whoami', 'people/color', 'people/name', 'tunnel/status'])
  })

  it('whoami failing shows the error, not a guess', async () => {
    h.routes = {}
    render(<AdminProfileButton />)
    act(() => void fireEvent.click(button()))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('unexpected route auth/whoami')
  })
})

describe('the Admin profile modal: editing inline (the owner)', () => {
  function withJulie(): void {
    h.routes.users = {
      users: [
        ...USERS.users,
        { username: 'julie', createdAt: '2026-09-20T12:00:00Z', role: 'owner', disabled: false, wasViewer: false },
      ],
    }
  }

  async function pickAccount(label: string): Promise<void> {
    act(() => void fireEvent.click(screen.getByLabelText('Your account')))
    const menu = await screen.findByTestId('setting-dropdown-menu')
    act(() => void fireEvent.click(within(menu).getByText(label)))
  }

  it('changes Your account: posts owner/accounts (the General route) and the header follows at once', async () => {
    withJulie()
    h.postRoutes['owner/accounts'] = (body) => {
      // The owner's names are the linked account's: julie has none yet.
      h.routes['people/name'] = nameView('owner', null, null)
      return { ok: true, accounts: body.accounts, aliases: body.accounts }
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect(text('admin-profile-name')).toBe('Rosson Long'))
    await pickAccount('julie')
    await waitFor(() => expect(h.posts).toEqual([{ route: 'owner/accounts', body: { accounts: ['julie'] } }]))
    // The names are re-read for the new account; with none, the username shows.
    await waitFor(() => expect(text('admin-profile-name')).toBe('julie'))
    expect(text('admin-profile-role')).toBe('Owner')
    expect((screen.getByLabelText('Your account') as HTMLButtonElement).textContent).toContain('julie')
    expect(h.calls.filter((c) => c === 'people/name').length).toBe(2)

    await pickAccount('None')
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'owner/accounts', body: { accounts: [] } }))
    await waitFor(() => expect(text('admin-profile-name')).toBe('Owner'))
    expect(document.querySelector('[data-testid="admin-profile-change-password"]')).toBeNull()
  })

  it('a refused account change rolls back and says why', async () => {
    withJulie()
    h.postRoutes['owner/accounts'] = () => {
      throw new Error('owner token required')
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await pickAccount('julie')
    expect((await screen.findByRole('alert')).textContent).toContain('owner token required')
    expect((screen.getByLabelText('Your account') as HTMLButtonElement).textContent).toContain('rosson-phone')
    expect(text('admin-profile-username')).toBe('rosson-phone')
  })

  it('changes first and last name: posts people/name for the owner (the linked account) and the header follows', async () => {
    h.postRoutes['people/name'] = (body) => nameView('owner', (body.firstName as string) ?? 'Rosson', (body.lastName as string) ?? 'Long')
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('Rosson'))
    const first = screen.getByLabelText('First name') as HTMLInputElement
    act(() => void fireEvent.change(first, { target: { value: '  Ross  ' } }))
    act(() => void fireEvent.blur(first))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'people/name', body: { firstName: 'Ross' } }]))
    await waitFor(() => expect(text('admin-profile-name')).toBe('Ross Long'))
    // Clearing sends null; the old display-name setting is never written.
    const last = screen.getByLabelText('Last name') as HTMLInputElement
    act(() => void fireEvent.change(last, { target: { value: '' } }))
    act(() => void fireEvent.blur(last))
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'people/name', body: { lastName: null } }))
    expect(h.settingsUpdates).toEqual([])
    // No separate "Your name" any more.
    expect(screen.queryByLabelText('Your name')).toBeNull()
  })

  it('a refused name rolls back with the server\'s error', async () => {
    h.postRoutes['people/name'] = () => {
      throw new Error('first name is longer than 64 characters')
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('Rosson'))
    const first = screen.getByLabelText('First name') as HTMLInputElement
    act(() => void fireEvent.change(first, { target: { value: 'Robert' } }))
    act(() => void fireEvent.blur(first))
    expect((await screen.findByRole('alert')).textContent).toContain('longer than 64')
    await waitFor(() => expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('Rosson'))
    expect(text('admin-profile-name')).toBe('Rosson Long')
  })

  it('a Connect member sets their own names (no person in the body)', async () => {
    h.scope = { label: 'Rosson’s Mac', isRemote: true, host: { ...remoteHost(), username: 'sam' } }
    h.routes = {
      'auth/whoami': { owner: false, username: 'sam', role: 'member', mustChangePassword: false },
      'tunnel/status': { running: false, frpc_installed: true },
      'people/color': colorView('sam', null, '#3a9e96'),
      'people/name': nameView('sam', null, null),
    }
    h.postRoutes['people/name'] = (body) => nameView('sam', (body.firstName as string) ?? null, null)
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect((screen.getByLabelText('First name') as HTMLInputElement).disabled).toBe(false))
    const first = screen.getByLabelText('First name') as HTMLInputElement
    act(() => first.focus())
    act(() => void fireEvent.change(first, { target: { value: 'Sam' } }))
    act(() => void fireEvent.keyDown(first, { key: 'Enter' }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'people/name', body: { firstName: 'Sam' } }]))
    await waitFor(() => expect(text('admin-profile-name')).toBe('Sam'))
    expect(text('admin-profile-username')).toBe('sam')
  })
})

describe('the Admin profile modal: change password (Connect logins only)', () => {
  function fill(label: string, value: string): void {
    act(() => void fireEvent.change(screen.getByLabelText(label), { target: { value } }))
  }

  /** The daemon side of the shared step: the policy read and the
   *  change-password POST, on the server's own address. */
  function stubDaemon(changeStatus = 200): ReturnType<typeof vi.fn> {
    const fetchMock = vi.fn(async (url: string) => {
      if (url.startsWith('https://rosson.k2.dev/cli/users/policy')) {
        return new Response(JSON.stringify({ minLength: 8, requireSpecial: false, requireNumber: true, requireUppercase: false }), { status: 200 })
      }
      if (url.startsWith('https://rosson.k2.dev/cli/auth/change-password')) {
        return new Response(changeStatus === 200 ? '{"success":true}' : '{"error":"unauthorized"}', { status: changeStatus })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)
    return fetchMock
  }

  beforeEach(() => {
    h.login.mockReset()
    h.remember.mockReset()
  })

  it('the owner on this computer with no account picked gets no Change password button', async () => {
    h.routes['owner/accounts'] = { accounts: [], aliases: [] }
    render(<AdminProfileButton />)
    await openLoaded()
    expect(document.querySelector('[data-testid="admin-profile-change-password"]')).toBeNull()
  })

  it('the owner on this computer sets the linked login\'s password with the owner\'s permission (users/set-password)', async () => {
    h.postRoutes['users/set-password'] = () => ({ success: true })
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    const form = screen.getByTestId('admin-profile-owner-password')
    expect(form.textContent).toContain('rosson-phone')
    // Not the self-service step: no current password, no re-login.
    expect(document.querySelector('[data-testid="password-rotation-step"]')).toBeNull()
    fill('New password', 'new-pass-22')
    fill('Confirm new password', 'new-pass-22')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Set password' })))
    await waitFor(() => expect(text('admin-profile-notice')).toBe(ownerPasswordChangedNotice('rosson-phone')))
    expect(h.posts).toEqual([{ route: 'users/set-password', body: { username: 'rosson-phone', password: 'new-pass-22' } }])
    expect(h.login).not.toHaveBeenCalled()
    expect(h.remember).not.toHaveBeenCalled()
  })

  it('the owner\'s set-password refusal stays on the form with the error', async () => {
    h.postRoutes['users/set-password'] = () => {
      throw new Error('password too short')
    }
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    fill('New password', 'x1')
    fill('Confirm new password', 'x1')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Set password' })))
    expect((await screen.findByRole('alert')).textContent).toContain('password too short')
    expect(screen.getByTestId('admin-profile-owner-password')).toBeTruthy()
  })

  it('a Connect member gets it too (self-service, any role)', async () => {
    h.scope = { label: 'Rosson’s Mac', isRemote: true, host: { ...remoteHost(), username: 'sam' } }
    h.routes = {
      'auth/whoami': { owner: false, username: 'sam', role: 'member', mustChangePassword: false },
      'tunnel/status': { running: false, frpc_installed: true },
    }
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-account')).toContain('Change password')
  })

  it('opens the shared first-password step with change-password words; Cancel goes back', async () => {
    connectAdmin()
    stubDaemon()
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    const step = screen.getByTestId('password-rotation-step')
    expect(dialog().contains(step)).toBe(true)
    expect(step.textContent).toContain(CHANGE_PASSWORD_COPY.title)
    expect(screen.getByLabelText(CHANGE_PASSWORD_COPY.currentLabel)).toBeTruthy()
    // The policy comes from the server, as in the first-password flow.
    await waitFor(() => expect(step.textContent).toContain('a number'))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Cancel' })))
    expect(document.querySelector('[data-testid="password-rotation-step"]')).toBeNull()
    expect(text('admin-profile-role')).toContain('Admin')
  })

  it('success: change-password with the current session, re-login with the new password, remembered password updated, back to the profile', async () => {
    connectAdmin(true)
    const fetchMock = stubDaemon()
    h.login.mockResolvedValue({ ok: true, token: 'sess-new', mustChangePassword: false })
    h.remember.mockResolvedValue(null)
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    fill(CHANGE_PASSWORD_COPY.currentLabel, 'old-pass-1')
    fill(CHANGE_PASSWORD_COPY.newLabel, 'new-pass-22')
    fill(CHANGE_PASSWORD_COPY.confirmLabel, 'new-pass-22')
    act(() => void fireEvent.click(screen.getByRole('button', { name: CHANGE_PASSWORD_COPY.submit })))

    await waitFor(() => expect(text('admin-profile-notice')).toBe(PASSWORD_CHANGED_NOTICE))
    expect(document.querySelector('[data-testid="password-rotation-step"]')).toBeNull()

    const post = fetchMock.mock.calls.find(([url]) => String(url).includes('/cli/auth/change-password'))
    if (!post) throw new Error('no change-password POST')
    expect(String(post[0])).toContain('token=sess-old')
    expect((post[1] as RequestInit).method).toBe('POST')
    expect(JSON.parse(String((post[1] as RequestInit).body))).toEqual({ currentPassword: 'old-pass-1', newPassword: 'new-pass-22' })

    expect(h.login).toHaveBeenCalledTimes(1)
    const [loginHost, loginPw] = h.login.mock.calls[0] as [{ id: string; username: string }, string]
    expect([loginHost.id, loginHost.username, loginPw]).toEqual(['host-rosson', 'rosson-phone', 'new-pass-22'])
    expect(h.remember.mock.calls).toEqual([['host-rosson', 'new-pass-22']])
  })

  it('a login that does not remember its password leaves the keychain alone', async () => {
    connectAdmin(false)
    stubDaemon()
    h.login.mockResolvedValue({ ok: true, token: 'sess-new', mustChangePassword: false })
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    fill(CHANGE_PASSWORD_COPY.currentLabel, 'old-pass-1')
    fill(CHANGE_PASSWORD_COPY.newLabel, 'new-pass-22')
    fill(CHANGE_PASSWORD_COPY.confirmLabel, 'new-pass-22')
    act(() => void fireEvent.click(screen.getByRole('button', { name: CHANGE_PASSWORD_COPY.submit })))
    await waitFor(() => expect(text('admin-profile-notice')).toBe(PASSWORD_CHANGED_NOTICE))
    expect(h.remember).not.toHaveBeenCalled()
  })

  it('a wrong current password stays on the step with the change-password wording, and no re-login', async () => {
    connectAdmin()
    stubDaemon(401)
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-change-password')))
    fill(CHANGE_PASSWORD_COPY.currentLabel, 'wrong-pass-1')
    fill(CHANGE_PASSWORD_COPY.newLabel, 'new-pass-22')
    fill(CHANGE_PASSWORD_COPY.confirmLabel, 'new-pass-22')
    act(() => void fireEvent.click(screen.getByRole('button', { name: CHANGE_PASSWORD_COPY.submit })))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe(CHANGE_PASSWORD_COPY.badCurrent)
    expect(screen.getByTestId('password-rotation-step')).toBeTruthy()
    expect(h.login).not.toHaveBeenCalled()
  })
})

describe('the Admin profile modal: colour', () => {
  function hexField(): HTMLInputElement {
    return screen.getByTestId('person-color-hex') as HTMLInputElement
  }

  it('Automatic + a colour well + a hex field + a preview chip; a typed hex posts people/color lowercased', async () => {
    h.postRoutes['people/color'] = (body) => colorView('owner', body.color as string | null, (body.color as string) ?? '#8c62c4')
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    expect(screen.getByTestId('person-color-auto').getAttribute('aria-pressed')).toBe('true')
    expect((screen.getByTestId('person-color-well') as HTMLInputElement).type).toBe('color')
    // No preset swatches any more.
    expect(document.querySelector('[data-testid="person-color-#3a9e96"]')).toBeNull()
    // No second avatar beside the picker: the header chip is the preview.
    expect(screen.getByTestId('person-color-picker').querySelector('[data-presence-chip]')).toBeNull()
    expect(text('admin-profile-color')).toContain('Your linked logins show this colour too.')

    act(() => void fireEvent.change(hexField(), { target: { value: '#12AB9F' } }))
    act(() => void fireEvent.keyDown(hexField(), { key: 'Enter' }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'people/color', body: { color: '#12ab9f' } }]))
    await waitFor(() => expect(screen.getByTestId('person-color-auto').getAttribute('aria-pressed')).toBe('false'))
    expect(screen.getByTestId('admin-profile-header').querySelector('[data-presence-chip]')?.getAttribute('data-person-color')).toBe('#12ab9f')

    act(() => void fireEvent.click(screen.getByTestId('person-color-auto')))
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'people/color', body: { color: null } }))
  })

  it('the colour well saves after the drag settles', async () => {
    h.postRoutes['people/color'] = (body) => colorView('owner', body.color as string | null, body.color as string)
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    act(() => void fireEvent.change(screen.getByTestId('person-color-well'), { target: { value: '#204060' } }))
    act(() => void fireEvent.change(screen.getByTestId('person-color-well'), { target: { value: '#405060' } }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'people/color', body: { color: '#405060' } }]), { timeout: 2000 })
  })

  it('a bad hex is refused locally with the format hint and never posted', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    for (const bad of ['#abc', 'red', '12ab9f', '#12ab9', '#gg0000']) {
      act(() => void fireEvent.change(hexField(), { target: { value: bad } }))
      expect(text('person-color-hex-hint')).toBe(HEX_FORMAT_HINT)
      expect(hexField().getAttribute('aria-invalid')).toBe('true')
      act(() => void fireEvent.keyDown(hexField(), { key: 'Enter' }))
      act(() => void fireEvent.blur(hexField()))
    }
    expect(h.posts).toEqual([])
  })

  it('a refused colour rolls back and says why', async () => {
    h.routes['people/color'] = colorView('owner', '#3a9e96', '#3a9e96')
    h.postRoutes['people/color'] = () => {
      throw new Error('color must be #RRGGBB')
    }
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-picker')
    act(() => void fireEvent.change(hexField(), { target: { value: '#ffffff' } }))
    act(() => void fireEvent.keyDown(hexField(), { key: 'Enter' }))
    expect((await screen.findByRole('alert')).textContent).toContain('color must be #RRGGBB')
    expect(hexField().value).toBe('#3a9e96')
  })

  it('a Connect member picks their own colour', async () => {
    h.scope = { label: 'Rosson’s Mac', isRemote: true, host: { ...remoteHost(), username: 'sam' } }
    h.routes = {
      'auth/whoami': { owner: false, username: 'sam', role: 'member', mustChangePassword: false },
      'tunnel/status': { running: false, frpc_installed: true },
      'people/color': colorView('sam', '#64778f', '#64778f'),
      'people/name': nameView('sam', null, null),
    }
    h.postRoutes['people/color'] = (body) => colorView('sam', body.color as string, body.color as string)
    render(<AdminProfileButton />)
    await openLoaded()
    await waitFor(() => expect(hexField().value).toBe('#64778f'))
    expect(text('admin-profile-color')).not.toContain('linked logins')
    act(() => void fireEvent.change(hexField(), { target: { value: '#c026d3' } }))
    act(() => void fireEvent.blur(hexField()))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'people/color', body: { color: '#c026d3' } }]))
  })

  it('an older daemon without the route shows the controls disabled with "Needs a newer K2 server", never hidden', async () => {
    h.routes['people/color'] = new Error('route not found')
    render(<AdminProfileButton />)
    await openLoaded()
    await screen.findByTestId('person-color-needs-newer')
    expect(text('admin-profile-color')).toContain('Colour')
    expect(text('person-color-needs-newer')).toBe(COLOR_NEEDS_NEWER_SERVER)
    expect(COLOR_NEEDS_NEWER_SERVER).toBe('Needs a newer K2 server.')
    for (const id of ['person-color-auto', 'person-color-well', 'person-color-hex']) {
      expect((screen.getByTestId(id) as HTMLInputElement).disabled).toBe(true)
    }
    expect(h.posts).toEqual([])
    expect(text('admin-profile-username')).toBe('rosson-phone')
  })
})

