// @vitest-environment jsdom
// Admin Access: optional first and last names (0.46.0, Rosson 2026-10-17).
// The add-user form sends them with POST /cli/users/add; a row shows the
// real name beside the username; "Edit name" sets someone's names through
// POST /cli/people/name with `person` (the daemon applies the floors).
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  whoami: {} as Json,
  users: [] as Json[],
  fetches: [] as Array<{ path: string; method: string; body: unknown }>,
  posts: [] as Array<{ route: string; body: Json }>,
  nameError: null as string | null,
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
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Json) => {
    cli.posts.push({ route, body })
    if (route === 'people/name') {
      if (cli.nameError) throw new Error(cli.nameError)
      const row = cli.users.find((u) => u.username === body.person)
      if (!row) throw new Error(`no user ${String(body.person)}`)
      row.firstName = body.firstName ?? null
      row.lastName = body.lastName ?? null
      return { ok: true, person: body.person, firstName: row.firstName, lastName: row.lastName, displayName: body.person, names: {} }
    }
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

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  cli.whoami = { owner: true, role: 'owner' }
  cli.users = [
    { username: 'z3thon', disabled: false, role: 'admin', firstName: 'Rosson', lastName: 'Long', displayName: 'Rosson Long' },
    { username: 'gina', disabled: false, role: 'member', firstName: null, lastName: null, displayName: 'gina' },
  ]
  cli.fetches = []
  cli.posts = []
  cli.nameError = null
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      const path = new URL(url).pathname
      const body = init?.body ? JSON.parse(String(init.body)) : undefined
      cli.fetches.push({ path, method, body })
      if (path === '/cli/auth/whoami') return json(cli.whoami)
      if (path === '/cli/users' && method === 'GET') return json({ users: cli.users })
      if (path === '/cli/users/policy') return json({ minLength: 8 })
      if (path === '/cli/users/add' && method === 'POST') {
        cli.users.push({ username: body.username, disabled: false, role: 'member', firstName: body.firstName ?? null, lastName: body.lastName ?? null })
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

describe('Admin Access: first and last names', () => {
  it('a row shows the real name beside the username; no name shows the username alone', async () => {
    render(<K2ConnectSection panel="people" />)
    expect((await screen.findByTestId('user-real-name-z3thon')).textContent).toBe('Rosson Long')
    expect(screen.getByText('z3thon')).toBeTruthy()
    expect(document.querySelector('[data-testid="user-real-name-gina"]')).toBeNull()
  })

  it('the add-user form sends the optional names with users/add', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-real-name-z3thon')
    act(() => void fireEvent.change(screen.getByPlaceholderText('username'), { target: { value: 'anna' } }))
    act(() => void fireEvent.change(screen.getByPlaceholderText('initial password'), { target: { value: 'pass-1234' } }))
    act(() => void fireEvent.change(screen.getByLabelText('New user first name'), { target: { value: ' Anna ' } }))
    act(() => void fireEvent.change(screen.getByLabelText('New user last name'), { target: { value: 'Lee' } }))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Add user' })))
    await waitFor(() => expect(cli.fetches.some((f) => f.path === '/cli/users/add')).toBe(true))
    const add = cli.fetches.find((f) => f.path === '/cli/users/add')
    if (!add) throw new Error('no users/add')
    expect(add.body).toEqual({ username: 'anna', password: 'pass-1234', firstName: 'Anna', lastName: 'Lee' })
    expect((await screen.findByTestId('user-real-name-anna')).textContent).toBe('Anna Lee')
  })

  it('names stay optional: no name fields → users/add without them', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-real-name-z3thon')
    act(() => void fireEvent.change(screen.getByPlaceholderText('username'), { target: { value: 'bo' } }))
    act(() => void fireEvent.change(screen.getByPlaceholderText('initial password'), { target: { value: 'pass-1234' } }))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Add user' })))
    await waitFor(() => expect(cli.fetches.some((f) => f.path === '/cli/users/add')).toBe(true))
    expect(cli.fetches.find((f) => f.path === '/cli/users/add')?.body).toEqual({ username: 'bo', password: 'pass-1234' })
  })

  it('Edit name sets someone else\'s names through people/name with person; empty clears', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-real-name-z3thon')
    act(() => void fireEvent.click(screen.getByTestId('edit-name-gina')))
    screen.getByTestId('name-form-gina')
    act(() => void fireEvent.change(screen.getByLabelText('gina first name'), { target: { value: 'Gina' } }))
    act(() => void fireEvent.change(screen.getByLabelText('gina last name'), { target: { value: '  ' } }))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Save name' })))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'people/name', body: { person: 'gina', firstName: 'Gina', lastName: null } }]))
    expect((await screen.findByTestId('user-real-name-gina')).textContent).toBe('Gina')
    expect(document.querySelector('[data-testid="name-form-gina"]')).toBeNull()
  })

  it('a refused name keeps the form open and shows the error', async () => {
    cli.nameError = 'only an Owner can change the owner\'s names'
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-real-name-z3thon')
    act(() => void fireEvent.click(screen.getByTestId('edit-name-z3thon')))
    expect((screen.getByLabelText('z3thon first name') as HTMLInputElement).value).toBe('Rosson')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Save name' })))
    expect(await screen.findByText('only an Owner can change the owner\'s names')).toBeTruthy()
    expect(screen.getByTestId('name-form-z3thon')).toBeTruthy()
  })
})
