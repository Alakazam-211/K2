// @vitest-environment jsdom
// Settings → Admin Access (the redesign): the people list's states, the
// lock-out guard (you can't demote, turn off or remove your own login),
// removal behind a confirm, a role change that says Saved or why not, and
// the password helpers the dialogs use.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  whoami: {} as Json,
  users: [] as Json[] | null,
  usersError: null as string | null,
  fetches: [] as Array<{ path: string; method: string; body: Json | undefined }>,
  roleError: null as string | null,
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
    if (route === 'owner/accounts') return { ok: true, accounts: [], aliases: [] }
    throw new Error(`route not found: ${route}`)
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

import { AdminAccessPanel } from './AdminAccessPanel'
import { makePassword, passwordProblem, usernameProblem } from './admin-access-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

const POLICY = { minLength: 10, requireSpecial: false, requireNumber: true, requireUppercase: false }

beforeEach(() => {
  cli.whoami = { owner: true, role: 'owner' }
  cli.users = [
    { username: 'alex', disabled: false, role: 'owner' },
    { username: 'jordan', disabled: false, role: 'member' },
  ]
  cli.usersError = null
  cli.roleError = null
  cli.fetches = []
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      const path = new URL(url).pathname
      const body = init?.body ? (JSON.parse(String(init.body)) as Json) : undefined
      cli.fetches.push({ path, method, body })
      if (path === '/cli/auth/whoami') return json(cli.whoami)
      if (path === '/cli/users' && method === 'GET') {
        if (cli.usersError) return json({ error: cli.usersError }, 500)
        return json({ users: cli.users })
      }
      if (path === '/cli/users/policy') return json(POLICY)
      if (path === '/cli/tunnel/config') return json({ publicUrl: 'https://alex.k2.dev' })
      if (path === '/cli/users/set-role') {
        if (cli.roleError) return json({ error: cli.roleError }, 403)
        const row = (cli.users ?? []).find((u) => u.username === body?.username)
        if (!row) throw new Error('no such user')
        row.role = body?.role
        return json({ success: true })
      }
      if (path === '/cli/users/remove') {
        cli.users = (cli.users ?? []).filter((u) => u.username !== body?.username)
        return json({ success: true })
      }
      throw new Error(`unexpected fetch ${method} ${path}`)
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function menu(user: string): void {
  act(() => void fireEvent.click(screen.getByRole('button', { name: `More actions for ${user}` })))
}

function item(name: RegExp): HTMLElement {
  return screen.getByRole('menuitem', { name })
}

describe('Admin Access: states', () => {
  it('a failed read says what failed, the server’s reason, and offers Try again', async () => {
    cli.usersError = 'users file is locked'
    render(<AdminAccessPanel />)
    const card = await screen.findByTestId('admin-access-read-failed')
    expect(card.textContent).toContain('Couldn’t load the people on this server')
    expect(card.textContent).toContain('The server said: users file is locked.')
    cli.usersError = null
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Try again' })))
    await screen.findByTestId('person-row-jordan')
  })

  it('no logins yet: says it is only you, with Add person', async () => {
    cli.users = []
    render(<AdminAccessPanel />)
    const row = await screen.findByTestId('admin-only-you')
    expect(row.textContent).toContain('Only you so far')
    expect(screen.getAllByRole('button', { name: 'Add person' })).toHaveLength(1)
  })

  it('a Member sees why they can’t manage people, and no list is read', async () => {
    cli.whoami = { owner: false, role: 'member', username: 'jordan' }
    render(<AdminAccessPanel />)
    await screen.findByTestId('admin-access-not-admin')
    expect(cli.fetches.some((f) => f.path === '/cli/users')).toBe(false)
  })
})

describe('Admin Access: lock-out guard', () => {
  it('your own login: role locked, sign-in and remove disabled, and it says why', async () => {
    cli.users = [
      { username: 'alex', disabled: false, role: 'owner' },
      { username: 'sam', disabled: false, role: 'owner' },
    ]
    cli.whoami = { owner: false, role: 'owner', username: 'alex' }
    render(<AdminAccessPanel />)
    const row = await screen.findByTestId('person-row-alex')
    expect(row.textContent).toContain('You')
    expect(row.textContent).toContain('This is you. Another owner can change your role.')
    expect((screen.getByRole('button', { name: 'Role for alex' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Role for sam' }) as HTMLButtonElement).disabled).toBe(false)
    menu('alex')
    expect(item(/Remove from this server/).getAttribute('aria-disabled')).toBe('true')
    expect(item(/Remove from this server/).textContent).toContain('You can’t remove yourself.')
    expect(item(/Turn off sign-in/).getAttribute('aria-disabled')).toBe('true')
  })

  it('the only Owner login can’t be demoted or removed by a login', async () => {
    cli.whoami = { owner: false, role: 'owner', username: 'jordan' }
    cli.users = [
      { username: 'alex', disabled: false, role: 'owner' },
      { username: 'jordan', disabled: false, role: 'owner' },
      { username: 'casey', disabled: true, role: 'owner' },
    ]
    // jordan is you; alex is the other enabled owner, so alex stays changeable.
    render(<AdminAccessPanel />)
    await screen.findByTestId('person-row-alex')
    expect((screen.getByRole('button', { name: 'Role for alex' }) as HTMLButtonElement).disabled).toBe(false)
    cli.users = [
      { username: 'alex', disabled: false, role: 'owner' },
      { username: 'jordan', disabled: false, role: 'admin' },
    ]
    cleanup()
    cli.whoami = { owner: false, role: 'admin', username: 'jordan' }
    render(<AdminAccessPanel />)
    const row = await screen.findByTestId('person-row-alex')
    expect(row.textContent).toContain('The only owner. Make someone else an owner first.')
    expect((screen.getByRole('button', { name: 'Role for alex' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('Admin Access: changes', () => {
  it('a role change says Saved by the row', async () => {
    render(<AdminAccessPanel />)
    await screen.findByTestId('person-row-jordan')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Role for jordan' })))
    act(() => void fireEvent.click(screen.getByTestId('setting-dropdown-menu').querySelector('button[data-value="admin"]') as HTMLElement))
    await waitFor(() => expect(screen.getByTestId('person-row-jordan').textContent).toContain('Saved'))
    expect(cli.fetches.find((f) => f.path === '/cli/users/set-role')?.body).toEqual({ username: 'jordan', role: 'admin' })
  })

  it('a refused role change puts the role back and says why', async () => {
    cli.roleError = 'role_change_not_allowed'
    render(<AdminAccessPanel />)
    await screen.findByTestId('person-row-jordan')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Role for jordan' })))
    act(() => void fireEvent.click(screen.getByTestId('setting-dropdown-menu').querySelector('button[data-value="admin"]') as HTMLElement))
    await waitFor(() => expect(screen.getByTestId('person-row-jordan').textContent).toContain('Couldn’t save: role_change_not_allowed. Nothing changed; try again.'))
    expect(screen.getByRole('button', { name: 'Role for jordan' }).textContent).toContain('Member')
  })

  it('Remove asks first; cancelling sends nothing, confirming removes and says so', async () => {
    render(<AdminAccessPanel />)
    await screen.findByTestId('person-row-jordan')
    menu('jordan')
    act(() => void fireEvent.click(item(/Remove from this server/)))
    await waitFor(() => expect(useConfirmDialogStore.getState().isOpen).toBe(true))
    expect(useConfirmDialogStore.getState().title).toBe('Remove jordan from this server?')
    expect(useConfirmDialogStore.getState().confirmDestructive).toBe(true)
    act(() => useConfirmDialogStore.getState().close())
    await new Promise((r) => setTimeout(r, 0))
    expect(cli.fetches.some((f) => f.path === '/cli/users/remove')).toBe(false)

    menu('jordan')
    act(() => void fireEvent.click(item(/Remove from this server/)))
    await waitFor(() => expect(useConfirmDialogStore.getState().isOpen).toBe(true))
    act(() => useConfirmDialogStore.getState().onResolve?.(true))
    expect((await screen.findByTestId('admin-notice')).textContent).toContain('jordan was removed.')
    await waitFor(() => expect(screen.queryByTestId('person-row-jordan')).toBeNull())
  })
})

describe('Admin Access: password helpers', () => {
  it('makePassword passes every rule at any minimum, without look-alike characters', () => {
    for (const min of [4, 10, 16, 40]) {
      const pw = makePassword({ minLength: min })
      expect(pw.length).toBe(Math.max(16, min))
      expect(passwordProblem(pw, { minLength: min, requireNumber: true, requireUppercase: true, requireSpecial: true })).toBeNull()
      expect(pw).not.toMatch(/[lIO01]/)
    }
  })

  it('passwordProblem and usernameProblem say what to fix', () => {
    expect(passwordProblem('short', POLICY)).toBe('Use at least 10 characters.')
    expect(passwordProblem('longenough-pw', POLICY)).toBe('Add a number.')
    expect(passwordProblem('longenough-pw1', POLICY)).toBeNull()
    expect(usernameProblem('')).toBeNull()
    expect(usernameProblem('Riley')).toBeNull()
    expect(usernameProblem('r')).toBe('Use at least 2 characters.')
    expect(usernameProblem('ri ley')).toBe('Use only letters, numbers, - and _.')
    expect(usernameProblem('owner')).toContain('kept for K2 itself')
  })
})
