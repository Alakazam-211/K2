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
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
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
    return h.routes[route]
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
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import TopBarUtilities from './TopBarUtilities'
import AdminProfileButton from './AdminProfileButton'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { useSettingsStore } from '@/stores/settings'
import { CHANGE_PASSWORD_COPY } from '@/lib/password-rotation'
import { PASSWORD_CHANGED_NOTICE } from './AdminProfileButton'

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
  }
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
  delete h.routes['users/audit']
}

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
    const last = screen.getByTestId('admin-profile-open-settings')
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

  it('the settings button opens Settings → User Access and closes the modal', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    act(() => void fireEvent.click(screen.getByTestId('admin-profile-open-settings')))
    expect(document.querySelector('[data-testid="admin-profile-dialog"]')).toBeNull()
    const s = useSettingsStore.getState()
    expect([s.settingsOpen, s.activeSection]).toEqual([true, 'people'])
  })
})

describe('the Admin profile modal: who you are', () => {
  it('the owner on this computer: display name, Owner, linked logins, their dates and sign-ins', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-name')).toContain('Rosson Long')
    expect(text('admin-profile-role')).toContain('Owner')
    expect(text('admin-profile-how')).toContain('As the owner, on this computer')
    expect(text('admin-profile-server')).toContain('This computer')
    expect(text('admin-profile-server')).toContain('https://rosson.k2.dev')
    expect(text('admin-profile-linked')).toContain('rosson-phone')

    const account = text('admin-profile-account')
    expect(account).toContain('The owner key isn’t an account')
    expect(account).toContain('rosson-phone')
    expect(account).not.toContain('sam')

    // Only rosson-phone's sign-in events, newest first; not sam's, not the
    // password change.
    const rows = Array.from(document.querySelectorAll('[data-testid="admin-profile-sign-in"]')).map((r) => r.textContent ?? '')
    expect(rows.length).toBe(2)
    expect(rows[0]).toContain('Failed (bad_password)')
    expect(rows[0]).toContain('192.168.1.7')
    expect(rows[1]).toContain('Signed in')
    expect(rows[1]).toContain('as rosson-phone')
    expect([...h.calls].sort()).toEqual(['auth/whoami', 'owner/accounts', 'tunnel/status', 'users', 'users/audit'])
  })

  it('the owner with no linked login says how to link one', async () => {
    h.routes['owner/accounts'] = { accounts: [], aliases: [] }
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-linked')).toContain('No Connect login is marked as you yet')
    expect(text('admin-profile-sign-ins')).toContain('No Connect sign-ins for your logins')
  })

  it('a Connect admin from another device: username, Admin, created date; sign-ins are owner-only', async () => {
    connectAdmin()
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-name')).toContain('rosson-phone')
    expect(text('admin-profile-name')).toContain('your Connect username')
    expect(text('admin-profile-role')).toContain('Admin')
    expect(text('admin-profile-how')).toContain('With a Connect login, from this device')
    expect(text('admin-profile-server')).toContain('Rosson’s Mac')
    expect(text('admin-profile-server')).toContain('rosson.k2.dev')
    expect(text('admin-profile-linked')).toContain('This login is marked as the owner’s own.')
    expect(text('admin-profile-created')).toContain('2026')
    expect(text('admin-profile-account')).not.toContain('sam')
    expect(text('admin-profile-sign-ins')).toContain('Only the owner can see this.')
    expect(h.calls).not.toContain('users/audit')
  })

  it('a Connect member: only whoami and tunnel status are read; the rest says who can see it', async () => {
    h.scope = { label: 'Rosson’s Mac', isRemote: true, host: { ...remoteHost(), username: 'sam' } }
    h.routes = {
      'auth/whoami': { owner: false, username: 'sam', role: 'member', mustChangePassword: false },
      'tunnel/status': { running: false, frpc_installed: true },
    }
    render(<AdminProfileButton />)
    await openLoaded()
    expect(text('admin-profile-name')).toContain('sam')
    expect(text('admin-profile-role')).toContain('Member')
    expect(text('admin-profile-linked')).toContain('Only an admin can see this.')
    expect(text('admin-profile-account')).toContain('Only an admin can see this.')
    expect(text('admin-profile-sign-ins')).toContain('Only the owner can see this.')
    expect([...h.calls].sort()).toEqual(['auth/whoami', 'tunnel/status'])
  })

  it('whoami failing shows the error, not a guess', async () => {
    h.routes = {}
    render(<AdminProfileButton />)
    act(() => void fireEvent.click(button()))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('unexpected route auth/whoami')
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

  it('the owner on this computer gets no Change password button', async () => {
    render(<AdminProfileButton />)
    await openLoaded()
    expect(document.querySelector('[data-testid="admin-profile-change-password"]')).toBeNull()
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
