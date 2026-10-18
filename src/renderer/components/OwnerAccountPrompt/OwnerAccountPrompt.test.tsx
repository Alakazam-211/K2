// @vitest-environment jsdom
// "Your account" setup prompt (0.46.0, Rosson 2026-10-17). The daemon
// decides whether it shows (`GET /cli/owner/account-prompt`); this client
// shows it only in the main window of a local connection. Pick mode: a
// list with Not now and "Create a new account"; create mode (no admins):
// a required create form with no Not now.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  label: 'main',
  remote: false,
  routes: {} as Record<string, unknown>,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  postRoutes: {} as Record<string, (body: Record<string, unknown>) => unknown>,
}))

vi.mock('@/lib/window-chrome', () => ({ getWindowLabel: () => h.label }))
vi.mock('@/kessel/server-scope', async (orig) => {
  const real = await orig<typeof import('@/kessel/server-scope')>()
  const scope = {
    get isRemote() {
      return h.remote
    },
    label: 'This computer',
  }
  return { ...real, primaryScope: () => scope }
})
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    h.gets.push(route)
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

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import OwnerAccountPrompt, { CREATE_TITLE, PROMPT_TITLE, PROMPT_WHY, parsePromptView } from './OwnerAccountPrompt'
import { SQUARE_RADIO_CLASS } from '@/components/ui'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function cand(username: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { username, role: 'admin', firstName: null, lastName: null, displayName: username, color: '#3a87a8', ...extra }
}

function pickView(candidates: Record<string, unknown>[], preselect: string | null = null): Record<string, unknown> {
  return { ok: true, show: true, mode: 'pick', candidates, preselect }
}

const CREATE_VIEW = { ok: true, show: true, mode: 'create', candidates: [], preselect: null }

beforeEach(() => {
  h.label = 'main'
  h.remote = false
  h.gets = []
  h.posts = []
  h.postRoutes = {
    'owner/accounts': (b) => ({ ok: true, accounts: b.accounts, aliases: b.accounts }),
    'owner/account-prompt/dismiss': () => ({ ok: true, show: false, mode: 'pick', candidates: [] }),
    'users/add': () => ({ success: true }),
    'users/set-role': () => ({ success: true }),
  }
  h.routes = {
    'owner/account-prompt': pickView([cand('z3thon', { firstName: 'Rosson', lastName: 'Long', displayName: 'Rosson Long' }), cand('julie')]),
    'people/color': { ok: true, palette: [], person: 'owner', color: '#8c62c4', chosen: null, colors: {} },
  }
})

afterEach(() => cleanup())

function prompt(): HTMLElement | null {
  return document.querySelector('[data-testid="owner-account-prompt"]')
}

function radio(username: string): HTMLInputElement {
  const row = screen.getByTestId(`owner-account-option-${username}`)
  const input = row.querySelector('input[type="radio"]')
  if (!(input instanceof HTMLInputElement)) throw new Error(`no radio for ${username}`)
  // The shared square radio (components/ui/SquareRadio), not the browser's round one.
  if (!input.classList.contains(SQUARE_RADIO_CLASS)) throw new Error(`${username}: not a SquareRadio`)
  return input
}

async function shown(): Promise<HTMLElement> {
  render(<OwnerAccountPrompt />)
  return screen.findByTestId('owner-account-prompt')
}

/** Let the GET settle when the prompt should stay hidden. */
async function settled(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

describe('owner account prompt: when it shows', () => {
  it('shows in the main window of a local connection when the daemon says so', async () => {
    const el = await shown()
    expect(el.textContent).toContain(PROMPT_TITLE)
    expect(el.textContent).toContain(PROMPT_WHY)
    expect(PROMPT_WHY).toBe('Agents see your messages as from this user.')
    // Display name + username + colour dot per admin.
    expect(screen.getByTestId('owner-account-option-z3thon').textContent).toContain('Rosson Long')
    expect(screen.getByTestId('owner-account-option-z3thon').textContent).toContain('z3thon')
    expect(screen.getByTestId('owner-account-dot-julie').style.background).toBe('rgb(58, 135, 168)')
  })

  it('never shows (and never asks) in a focus window or on a remote connection', async () => {
    h.label = 'focus-abc'
    render(<OwnerAccountPrompt />)
    await settled()
    expect(prompt()).toBeNull()
    cleanup()
    h.label = 'main'
    h.remote = true
    render(<OwnerAccountPrompt />)
    await settled()
    expect(prompt()).toBeNull()
    expect(h.gets).toEqual([])
  })

  it('hides when the daemon says no (linked, dismissed, not the owner token, older daemon)', async () => {
    for (const answer of [
      { ok: true, show: false, mode: 'pick', candidates: [cand('z3thon')] },
      new Error('route not found'),
      new Error('403 forbidden'),
    ]) {
      h.routes['owner/account-prompt'] = answer
      render(<OwnerAccountPrompt />)
      await settled()
      expect(prompt()).toBeNull()
      cleanup()
    }
  })

  it('re-reads on window focus and hides when another window linked an account', async () => {
    await shown()
    h.routes['owner/account-prompt'] = { ok: true, show: false, mode: 'pick', candidates: [] }
    act(() => void window.dispatchEvent(new Event('focus')))
    await waitFor(() => expect(prompt()).toBeNull())
  })
})

describe('owner account prompt: pre-select', () => {
  it('takes the daemon pre-selection (one admin, or the old "Your name" match)', async () => {
    h.routes['owner/account-prompt'] = pickView([cand('z3thon'), cand('julie')], 'z3thon')
    await shown()
    expect([radio('z3thon').checked, radio('julie').checked]).toEqual([true, false])
  })

  it('selects nothing when the daemon has no obvious match; Use this account waits for a pick', async () => {
    await shown()
    expect([radio('z3thon').checked, radio('julie').checked]).toEqual([false, false])
    expect((screen.getByRole('button', { name: 'Use this account' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('the choices are the shared square radios: one group, each labelled by its account, keyboard-selectable', async () => {
    await shown()
    const group = screen.getByRole('radiogroup', { name: 'Your account' })
    const radios = Array.from(group.querySelectorAll('input[type="radio"]')) as HTMLInputElement[]
    expect(radios.map((r) => [r.name, r.value, r.classList.contains(SQUARE_RADIO_CLASS)])).toEqual([
      ['owner-account', 'z3thon', true],
      ['owner-account', 'julie', true],
    ])
    // The label still names it (a real radio inside its label).
    expect(screen.getByRole('radio', { name: /Rosson Long/ })).toBe(radio('z3thon'))
    // Focusable, and Space/arrow selection is the browser's own radio:
    // a change event picks it like a click does.
    radio('julie').focus()
    expect(document.activeElement).toBe(radio('julie'))
    act(() => void fireEvent.click(radio('julie')))
    expect([radio('z3thon').checked, radio('julie').checked]).toEqual([false, true])
  })

  it('ignores a pre-selection that is not in the list', () => {
    expect(parsePromptView(pickView([cand('a')], 'ghost')).preselect).toBeNull()
  })
})

describe('owner account prompt: pick and Not now', () => {
  it('a pick links it through owner/accounts and hides the prompt', async () => {
    await shown()
    act(() => void fireEvent.click(radio('julie')))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Use this account' })))
    await waitFor(() => expect(prompt()).toBeNull())
    expect(h.posts).toEqual([{ route: 'owner/accounts', body: { accounts: ['julie'] } }])
  })

  it('Not now stores the dismissal on the daemon and hides it', async () => {
    await shown()
    act(() => void fireEvent.click(screen.getByTestId('owner-account-not-now')))
    await waitFor(() => expect(prompt()).toBeNull())
    expect(h.posts).toEqual([{ route: 'owner/account-prompt/dismiss', body: {} }])
  })

  it('a refused link stays open with the error', async () => {
    h.postRoutes['owner/accounts'] = () => {
      throw new Error('owner token required')
    }
    await shown()
    act(() => void fireEvent.click(radio('julie')))
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Use this account' })))
    expect((await screen.findByRole('alert')).textContent).toContain('owner token required')
    expect(prompt()).toBeTruthy()
  })
})

function fill(label: string, value: string): void {
  act(() => void fireEvent.change(screen.getByLabelText(label), { target: { value } }))
}

describe('owner account prompt: create a new account (admins exist)', () => {
  it('the create option swaps to the form; Back returns; Not now stays', async () => {
    await shown()
    act(() => void fireEvent.click(screen.getByTestId('owner-account-create-option')))
    expect(screen.getByTestId('owner-account-create')).toBeTruthy()
    expect(prompt()?.textContent).toContain(CREATE_TITLE)
    expect(screen.getByTestId('owner-account-not-now')).toBeTruthy()
    act(() => void fireEvent.click(screen.getByTestId('owner-account-back')))
    expect(document.querySelector('[data-testid="owner-account-create"]')).toBeNull()
    expect(screen.getByTestId('owner-account-option-julie')).toBeTruthy()
  })

  it('a create makes the user (add, admin role) and links it, then closes', async () => {
    await shown()
    act(() => void fireEvent.click(screen.getByTestId('owner-account-create-option')))
    fill('Username', '  Rosson-Mac ')
    fill('Password', 'pass-1234')
    fill('Confirm password', 'pass-1234')
    fill('First name', 'Rosson')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Create and use this account' })))
    await waitFor(() => expect(prompt()).toBeNull())
    expect(h.posts).toEqual([
      { route: 'users/add', body: { username: 'rosson-mac', password: 'pass-1234', firstName: 'Rosson' } },
      { route: 'users/set-role', body: { username: 'rosson-mac', role: 'admin' } },
      { route: 'owner/accounts', body: { accounts: ['rosson-mac'] } },
    ])
  })
})

describe('owner account prompt: no admins (create is required)', () => {
  beforeEach(() => {
    h.routes['owner/account-prompt'] = CREATE_VIEW
  })

  it('renders the create form with no Not now and no Back', async () => {
    const el = await shown()
    expect(el.getAttribute('data-mode')).toBe('create')
    expect(el.textContent).toContain(CREATE_TITLE)
    expect(CREATE_TITLE).toBe('Create your admin user')
    expect(screen.getByTestId('owner-account-create')).toBeTruthy()
    expect(document.querySelector('[data-testid="owner-account-not-now"]')).toBeNull()
    expect(document.querySelector('[data-testid="owner-account-back"]')).toBeNull()
    // The colour picker is part of the form (Automatic by default).
    await screen.findByTestId('person-color-picker')
    expect(screen.getByTestId('person-color-auto').getAttribute('aria-pressed')).toBe('true')
  })

  it('Esc and a click outside do not close it', async () => {
    await shown()
    act(() => void fireEvent.keyDown(window, { key: 'Escape' }))
    act(() => void fireEvent.keyDown(document.body, { key: 'Escape' }))
    act(() => void fireEvent.mouseDown(screen.getByTestId('owner-account-prompt-scrim')))
    act(() => void fireEvent.click(screen.getByTestId('owner-account-prompt-scrim')))
    expect(prompt()).toBeTruthy()
  })

  it('the create button waits for a username and matching passwords', async () => {
    await shown()
    const create = screen.getByRole('button', { name: 'Create and use this account' }) as HTMLButtonElement
    expect(create.disabled).toBe(true)
    fill('Username', 'rosson')
    fill('Password', 'pass-1234')
    fill('Confirm password', 'pass-9999')
    expect(screen.getByText('The passwords don’t match.')).toBeTruthy()
    expect(create.disabled).toBe(true)
    fill('Confirm password', 'pass-1234')
    expect(create.disabled).toBe(false)
  })

  it('a successful create links the user and closes the modal', async () => {
    await shown()
    fill('Username', 'rosson')
    fill('Password', 'pass-1234')
    fill('Confirm password', 'pass-1234')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Create and use this account' })))
    await waitFor(() => expect(prompt()).toBeNull())
    expect(h.posts.map((p) => p.route)).toEqual(['users/add', 'users/set-role', 'owner/accounts'])
    expect(h.posts[2].body).toEqual({ accounts: ['rosson'] })
  })

  it('a daemon error stays inline and the modal stays open', async () => {
    h.postRoutes['users/add'] = () => {
      throw new Error("user 'rosson' already exists")
    }
    await shown()
    fill('Username', 'rosson')
    fill('Password', 'pass-1234')
    fill('Confirm password', 'pass-1234')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Create and use this account' })))
    expect((await screen.findByRole('alert')).textContent).toContain('already exists')
    expect(prompt()).toBeTruthy()
    expect(h.posts.map((p) => p.route)).toEqual(['users/add'])
  })

  it('a failed link after the user was made retries the link without re-creating', async () => {
    let fail = true
    h.postRoutes['owner/accounts'] = (b) => {
      if (fail) throw new Error('link refused')
      return { ok: true, accounts: b.accounts, aliases: b.accounts }
    }
    await shown()
    fill('Username', 'rosson')
    fill('Password', 'pass-1234')
    fill('Confirm password', 'pass-1234')
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Create and use this account' })))
    expect((await screen.findByRole('alert')).textContent).toContain('link refused')
    fail = false
    act(() => void fireEvent.click(screen.getByRole('button', { name: 'Create and use this account' })))
    await waitFor(() => expect(prompt()).toBeNull())
    expect(h.posts.map((p) => p.route)).toEqual(['users/add', 'users/set-role', 'owner/accounts', 'users/set-role', 'owner/accounts'])
  })
})
