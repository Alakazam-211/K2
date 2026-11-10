// @vitest-environment jsdom
// Admin Access (0.46.8, Rosson 2026-10-31): a password you set for someone
// else is TEMPORARY by default — at their first sign-in K2 asks them to
// choose their own (the shared PasswordRotationStep). Add a person sends
// mustChangePassword (on unless unticked) and its done step says what the
// person does next; Reset password shows the note and leaves the default
// to the daemon; a row with a temporary password shows a badge.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  users: [] as Json[],
  fetches: [] as Array<{ path: string; method: string; body: Json | undefined }>,
  linked: [] as string[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', listen: async () => () => undefined }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'owner/accounts') return { ok: true, accounts: cli.linked, aliases: cli.linked }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  getLocalDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  daemonHttpBase: () => 'http://daemon.test',
  invalidateDaemonWs: vi.fn(),
}))

vi.mock('./useNotifyPrefs', async (orig) => ({
  ...(await orig<typeof import('./useNotifyPrefs')>()),
  threadPushAvailable: () => true,
}))

import { K2ConnectSection } from './K2ConnectSection'
import { MUST_CHANGE_NOTE } from './AppUserDialogs'

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  cli.users = [
    { username: 'z3thon', disabled: false, role: 'owner', displayName: 'z3thon', mustChangePassword: false },
    { username: 'gina', disabled: false, role: 'member', displayName: 'gina', mustChangePassword: true },
  ]
  cli.fetches = []
  cli.linked = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      const path = new URL(url).pathname
      const body = init?.body ? (JSON.parse(String(init.body)) as Json) : undefined
      cli.fetches.push({ path, method, body })
      if (path === '/cli/auth/whoami') return json({ owner: true, role: 'owner' })
      if (path === '/cli/users' && method === 'GET') return json({ users: cli.users })
      if (path === '/cli/users/policy') return json({ minLength: 8 })
      if (path === '/cli/users/add' && method === 'POST') {
        if (!body) throw new Error('users/add without a body')
        const must = body.mustChangePassword !== false
        cli.users.push({ username: body.username, disabled: false, role: 'member', mustChangePassword: must })
        return json({ success: true, mustChangePassword: must })
      }
      if (path === '/cli/users/set-password' && method === 'POST') {
        if (!body) throw new Error('set-password without a body')
        // The daemon's default: temporary unless it is the owner's own login.
        const must = typeof body.mustChangePassword === 'boolean' ? body.mustChangePassword : !cli.linked.includes(String(body.username))
        const row = cli.users.find((u) => u.username === body.username)
        if (!row) throw new Error(`no user ${String(body.username)}`)
        row.mustChangePassword = must
        return json({ success: true, mustChangePassword: must })
      }
      throw new Error(`unexpected fetch ${method} ${path}`)
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

/** Open Add a person, type a username and (unless `make`) a password,
 *  and add. Returns the dialog. */
async function addPerson(username: string, opts: { make?: boolean; untick?: boolean } = {}): Promise<HTMLElement> {
  await screen.findByTestId('temporary-password-gina')
  act(() => void fireEvent.click(screen.getByRole('button', { name: 'Add person' })))
  const dialog = screen.getByTestId('add-person-dialog')
  const box = within(dialog).getByLabelText('Temporary password') as HTMLInputElement
  expect(box.checked).toBe(true)
  expect(within(dialog).getByTestId('new-user-temporary').textContent).toContain(MUST_CHANGE_NOTE)
  if (opts.untick) {
    act(() => void fireEvent.click(box))
    expect(within(dialog).getByTestId('new-user-temporary').textContent).not.toContain(MUST_CHANGE_NOTE)
  }
  act(() => void fireEvent.change(within(dialog).getByLabelText('Username'), { target: { value: username } }))
  if (!opts.make) {
    act(() => void fireEvent.click(within(dialog).getByRole('radio', { name: 'Type one' })))
    act(() => void fireEvent.change(within(dialog).getByLabelText('Password'), { target: { value: 'pass-1234' } }))
  }
  act(() => void fireEvent.click(within(dialog).getByRole('button', { name: 'Add person' })))
  return screen.findByTestId('person-added-dialog')
}

function lastBody(path: string): Json {
  const hit = [...cli.fetches].reverse().find((f) => f.path === path && f.method === 'POST')
  if (!hit || !hit.body) throw new Error(`no POST ${path}`)
  return hit.body
}

/** A row's "…" menu item. */
function menuItem(user: string, name: RegExp): void {
  act(() => void fireEvent.click(screen.getByRole('button', { name: `More actions for ${user}` })))
  act(() => void fireEvent.click(screen.getByRole('menuitem', { name })))
}

const ASKS_TO_CHOOSE = 'K2 then asks them to choose their own password.'

describe('Admin Access: temporary passwords', () => {
  it('a row with a temporary password shows the badge; others do not', async () => {
    render(<K2ConnectSection panel="people" />)
    expect((await screen.findByTestId('temporary-password-gina')).textContent).toBe('Temporary password')
    expect(document.querySelector('[data-testid="temporary-password-z3thon"]')).toBeNull()
  })

  it('Add a person is temporary by default: sends mustChangePassword true and says what they do next', async () => {
    render(<K2ConnectSection panel="people" />)
    const done = await addPerson('anna')
    expect(within(done).getByTestId('added-user-msg').textContent).toBe('Send anna these details yourself. Use the password you typed.')
    expect(done.textContent).toContain(ASKS_TO_CHOOSE)
    expect(lastBody('/cli/users/add')).toEqual({ username: 'anna', password: 'pass-1234', mustChangePassword: true })
    expect((await screen.findByTestId('temporary-password-anna')).textContent).toBe('Temporary password')
  })

  it('unticking it sends false, and the done step does not promise a new password', async () => {
    render(<K2ConnectSection panel="people" />)
    const done = await addPerson('bo', { untick: true })
    expect(done.textContent).not.toContain(ASKS_TO_CHOOSE)
    expect(lastBody('/cli/users/add')).toEqual({ username: 'bo', password: 'pass-1234', mustChangePassword: false })
    await waitFor(() => expect(cli.fetches.filter((f) => f.path === '/cli/users' && f.method === 'GET').length).toBeGreaterThan(1))
    expect(document.querySelector('[data-testid="temporary-password-bo"]')).toBeNull()
  })

  it('Make one for me sends a password that passes the rules and shows that same password once', async () => {
    render(<K2ConnectSection panel="people" />)
    const done = await addPerson('cy', { make: true })
    const sent = String(lastBody('/cli/users/add').password)
    expect(sent.length).toBeGreaterThanOrEqual(16)
    expect(sent).toMatch(/[a-z]/)
    expect(sent).toMatch(/[A-Z]/)
    expect(sent).toMatch(/[0-9]/)
    expect(sent).toMatch(/[^A-Za-z0-9]/)
    expect(within(done).getByTestId('one-time-secret').textContent).toBe(sent)
    expect(within(done).getByTestId('added-user-msg').textContent).toContain('K2 won’t show this password again')
  })

  it('Reset password shows the note, sends no flag, and reports the temporary answer', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    cli.users[1].mustChangePassword = false
    menuItem('gina', /Reset password/)
    const form = screen.getByTestId('reset-form-gina')
    expect(form.textContent).toContain(MUST_CHANGE_NOTE)
    act(() => void fireEvent.click(within(form).getByRole('radio', { name: 'Type one' })))
    act(() => void fireEvent.change(within(form).getByLabelText('Password'), { target: { value: 'reset-123' } }))
    act(() => void fireEvent.click(within(form).getByRole('button', { name: 'Reset password' })))
    expect((await screen.findByTestId('reset-password-msg')).textContent).toBe(`Password reset for gina. ${MUST_CHANGE_NOTE}`)
    expect(lastBody('/cli/users/set-password')).toEqual({ username: 'gina', password: 'reset-123' })
    expect((await screen.findByTestId('temporary-password-gina')).textContent).toBe('Temporary password')
  })

  it("Reset password on the owner's own linked login: kept, and says so", async () => {
    cli.linked = ['z3thon']
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    await waitFor(() => expect(screen.getByTestId('owner-account-tag-z3thon')).toBeTruthy())
    menuItem('z3thon', /Reset password/)
    const form = screen.getByTestId('reset-form-z3thon')
    expect(form.textContent).toContain('Your own login: you keep this password.')
    act(() => void fireEvent.click(within(form).getByRole('radio', { name: 'Type one' })))
    act(() => void fireEvent.change(within(form).getByLabelText('Password'), { target: { value: 'mine-1234' } }))
    act(() => void fireEvent.click(within(form).getByRole('button', { name: 'Reset password' })))
    expect((await screen.findByTestId('reset-password-msg')).textContent).toBe('Password reset for z3thon.')
    expect(document.querySelector('[data-testid="temporary-password-z3thon"]')).toBeNull()
  })
})
