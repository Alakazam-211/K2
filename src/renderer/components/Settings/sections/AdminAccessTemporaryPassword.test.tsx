// @vitest-environment jsdom
// Admin Access (0.46.8, Rosson 2026-10-31): a password you set for someone
// else is TEMPORARY by default — at their first sign-in K2 asks them to
// choose their own (the shared PasswordRotationStep). The add form sends
// mustChangePassword (on unless unticked) and shows the note; Reset
// password shows the note and leaves the default to the daemon; a row with
// a temporary password shows a "Temporary password" badge.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

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

function fillAdd(username: string): void {
  act(() => void fireEvent.change(screen.getByPlaceholderText('username'), { target: { value: username } }))
  act(() => void fireEvent.change(screen.getByPlaceholderText('initial password'), { target: { value: 'pass-1234' } }))
}

function lastBody(path: string): Json {
  const hit = [...cli.fetches].reverse().find((f) => f.path === path && f.method === 'POST')
  if (!hit || !hit.body) throw new Error(`no POST ${path}`)
  return hit.body
}

describe('Admin Access: temporary passwords', () => {
  it('a row with a temporary password shows the badge; others do not', async () => {
    render(<K2ConnectSection panel="people" />)
    expect((await screen.findByTestId('temporary-password-gina')).textContent).toBe('Temporary password')
    expect(document.querySelector('[data-testid="temporary-password-z3thon"]')).toBeNull()
  })

  it('the add form is temporary by default: sends mustChangePassword true and shows the note', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    const box = screen.getByLabelText('Temporary password') as HTMLInputElement
    expect(box.checked).toBe(true)
    expect(screen.getByTestId('new-user-temporary').textContent).toContain(MUST_CHANGE_NOTE)
    fillAdd('anna')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Add user' })))
    expect((await screen.findByTestId('added-user-msg')).textContent).toBe(
      `Added — share these credentials with anna. ${MUST_CHANGE_NOTE}`,
    )
    expect(lastBody('/cli/users/add')).toEqual({ username: 'anna', password: 'pass-1234', mustChangePassword: true })
    expect((await screen.findByTestId('temporary-password-anna')).textContent).toBe('Temporary password')
  })

  it('unticking Temporary password sends false and the note is not shown after adding', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    act(() => void fireEvent.click(screen.getByLabelText('Temporary password')))
    expect(screen.getByTestId('new-user-temporary').textContent).not.toContain(MUST_CHANGE_NOTE)
    fillAdd('bo')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Add user' })))
    expect((await screen.findByTestId('added-user-msg')).textContent).toBe('Added — share these credentials with bo.')
    expect(lastBody('/cli/users/add')).toEqual({ username: 'bo', password: 'pass-1234', mustChangePassword: false })
    await waitFor(() => expect(cli.fetches.filter((f) => f.path === '/cli/users' && f.method === 'GET').length).toBeGreaterThan(1))
    expect(document.querySelector('[data-testid="temporary-password-bo"]')).toBeNull()
  })

  it('Reset password shows the note, sends no flag, and reports the temporary answer', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    cli.users[1].mustChangePassword = false
    const resets = screen.getAllByRole('button', { name: 'Reset password' })
    // Rows are listed in the daemon's order: z3thon, gina.
    act(() => void fireEvent.click(resets[1]))
    const form = await screen.findByTestId('reset-form-gina')
    expect(form.textContent).toContain(MUST_CHANGE_NOTE)
    act(() => void fireEvent.change(screen.getByPlaceholderText('new password'), { target: { value: 'reset-123' } }))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Set password' })))
    expect((await screen.findByTestId('reset-password-msg')).textContent).toBe(`Password reset for gina. ${MUST_CHANGE_NOTE}`)
    expect(lastBody('/cli/users/set-password')).toEqual({ username: 'gina', password: 'reset-123' })
    expect((await screen.findByTestId('temporary-password-gina')).textContent).toBe('Temporary password')
  })

  it("Reset password on the owner's own linked login: kept, and says so", async () => {
    cli.linked = ['z3thon']
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('temporary-password-gina')
    const resets = screen.getAllByRole('button', { name: 'Reset password' })
    act(() => void fireEvent.click(resets[0]))
    const form = await screen.findByTestId('reset-form-z3thon')
    await waitFor(() => expect(form.textContent).toContain('Your own login: you keep this password.'))
    act(() => void fireEvent.change(screen.getByPlaceholderText('new password'), { target: { value: 'mine-1234' } }))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Set password' })))
    expect((await screen.findByTestId('reset-password-msg')).textContent).toBe('Password reset for z3thon.')
    expect(document.querySelector('[data-testid="temporary-password-z3thon"]')).toBeNull()
  })
})
