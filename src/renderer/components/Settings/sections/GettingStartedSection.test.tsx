// @vitest-environment jsdom
//
// Settings → Getting started (plan-getting-started-settings-v1): the setup
// list's state line and Start setup again (restart, then Settings closes; a
// Member while the list is off is told the owner was asked), and the Kessel
// part for the owner and admins only: Set up Kessel (restart with Kessel on,
// then the sheet), Open Kessel (its Chat tab) and the welcome again behind a
// confirm. The daemon is mocked at daemon-cli.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openThread: vi.fn(),
  openSheet: vi.fn(),
  confirm: vi.fn(async () => true),
  closeSettings: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@/stores/session-events', () => {
  const none = () => () => {}
  return {
    onFirstRunChanged: none,
    onLlmAccountsChanged: none,
    onProjectsChanged: none,
    onTunnelStatusChanged: none,
    onTunnelIdentityChanged: none,
    onPairCodeChanged: none,
    onAppHello: none,
  }
})
vi.mock('@/lib/open-workspace-thread', () => ({ openWorkspaceThread: (id: string) => h.openThread(id) }))
vi.mock('@/stores/kessel-sheet', () => ({ openKesselSheet: () => h.openSheet() }))
vi.mock('@/stores/confirm-dialog', () => ({
  useConfirmDialogStore: { getState: () => ({ confirm: h.confirm }) },
}))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { productName } from '@/lib/brand'
import { resetFirstRunForTests } from '@/stores/first-run'
import { useSettingsStore } from '@/stores/settings'
import { GETTING_STARTED_COPY, GettingStartedSection, kesselLine, setupStateLine } from './GettingStartedSection'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

type Status = Record<string, unknown>

function status(over: Status = {}): Status {
  return {
    enabled: true,
    hidden: false,
    finished: false,
    rows: [
      { id: 'llm_sign_in', state: 'done', opens: 'llm_sign_in' },
      { id: 'first_agent', state: 'todo', opens: 'add_workspace' },
      { id: 'tunnel', state: 'todo', opens: 'tunnel' },
    ],
    kesselFirstRun: false,
    kesselSeat: { exists: false },
    ...over,
  }
}

let current: Status = status()
let who: Record<string, unknown> = { owner: true, role: 'owner' }

beforeEach(() => {
  resetFirstRunForTests()
  current = status()
  who = { owner: true, role: 'owner' }
  useSettingsStore.setState({ closeSettings: h.closeSettings })
  h.daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'first-run/status') return current
    if (route === 'auth/whoami') return who
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'first-run/restart') return current
    if (route === 'first-run/kessel/welcome') return { ok: true, workspaceId: 'ws-kessel', sent: true }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('the state line and the Kessel line', () => {
  it('says finished, steps left, or off', () => {
    expect(setupStateLine(null)).toBe(GETTING_STARTED_COPY.off)
    const doc = { enabled: true, hidden: false, finished: false, offer: null, kessel: null, kesselDefaults: null, rows: [] }
    expect(setupStateLine({ ...doc, enabled: false })).toBe(GETTING_STARTED_COPY.off)
    expect(setupStateLine({ ...doc, finished: true })).toBe('Finished')
    expect(setupStateLine({ ...doc, rows: [{ id: 'tunnel', state: 'todo', opens: 'tunnel', note: null }] })).toBe('1 step left')
    expect(
      setupStateLine({
        ...doc,
        rows: [
          { id: 'tunnel', state: 'todo', opens: 'tunnel', note: null },
          { id: 'phone', state: 'todo', opens: 'phone', note: null },
          { id: 'llm_sign_in', state: 'skipped', opens: 'llm_sign_in', note: null },
        ],
      }),
    ).toBe('2 steps left')
  })

  it('names what Kessel is for, or who does it', () => {
    expect(kesselLine(null)).toBe(`Kessel runs ${productName()} for you: setup, settings and hiring your agents.`)
    expect(kesselLine('Kay')).toBe(`Kay runs ${productName()} for you.`)
  })
})

describe('Settings → Getting started', () => {
  it('draws the list state and starts setup again, then closes Settings', async () => {
    render(<GettingStartedSection />)
    await waitFor(() => expect(screen.getByTestId('getting-started.setup-list-line').textContent).toBe('2 steps left'))
    fireEvent.click(screen.getByTestId('getting-started-restart'))
    await waitFor(() => expect(h.closeSettings).toHaveBeenCalledTimes(1))
    expect(h.daemonCliPost).toHaveBeenCalledWith(expect.anything(), 'first-run/restart', {})
  })

  it('says the list is off on this server', async () => {
    current = status({ enabled: false })
    render(<GettingStartedSection />)
    await waitFor(() => expect(screen.getByTestId('getting-started.setup-list-line').textContent).toBe(GETTING_STARTED_COPY.off))
  })

  it('a Member while the list is off is told the owner was asked, and never sees Kessel', async () => {
    who = { owner: false, role: 'member' }
    current = status({ enabled: false })
    const refusal = (already: boolean) =>
      new DaemonHttpError('only the owner or an admin can turn the setup list on', 403, 'role_required', null, {
        ok: false,
        error: { code: 'role_required', hint: 'x' },
        asked: { ticketId: 't-1', already },
      })
    h.daemonCliPost.mockRejectedValueOnce(refusal(false)).mockRejectedValueOnce(refusal(true))
    render(<GettingStartedSection />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith(expect.anything(), 'auth/whoami'))
    fireEvent.click(screen.getByTestId('getting-started-restart'))
    await waitFor(() => expect(screen.getByTestId('getting-started-note').textContent).toBe(GETTING_STARTED_COPY.asked))
    fireEvent.click(screen.getByTestId('getting-started-restart'))
    await waitFor(() => expect(screen.getByTestId('getting-started-note').textContent).toBe(GETTING_STARTED_COPY.askedAlready))
    expect(h.closeSettings).not.toHaveBeenCalled()
    expect(screen.queryByTestId('getting-started-set-up-kessel')).toBeNull()
    expect(screen.queryByTestId('getting-started-open-kessel')).toBeNull()
  })

  it('shows any other refusal as the server says it', async () => {
    h.daemonCliPost.mockRejectedValueOnce(new DaemonHttpError('the store is busy', 500, 'store'))
    render(<GettingStartedSection />)
    await waitFor(() => expect(screen.getByTestId('getting-started-restart')).toBeTruthy())
    fireEvent.click(screen.getByTestId('getting-started-restart'))
    await waitFor(() => expect(screen.getByTestId('getting-started-error').textContent).toBe('the store is busy'))
  })

  it('with no Kessel, Set up Kessel restarts with Kessel on and opens the sheet', async () => {
    render(<GettingStartedSection />)
    const button = await screen.findByTestId('getting-started-set-up-kessel')
    expect(screen.getByTestId('getting-started.kessel-line').textContent).toBe(kesselLine(null))
    fireEvent.click(button)
    await waitFor(() => expect(h.openSheet).toHaveBeenCalledTimes(1))
    expect(h.daemonCliPost).toHaveBeenCalledWith(expect.anything(), 'first-run/restart', { kessel: true })
  })

  it('with Kessel, Open Kessel goes to its Chat tab and the welcome waits for the confirm', async () => {
    who = { owner: false, role: 'admin' }
    current = status({ kesselSeat: { exists: true, workspaceId: 'ws-kessel', name: 'Kay' } })
    render(<GettingStartedSection />)
    const open = await screen.findByTestId('getting-started-open-kessel')
    expect(screen.getByTestId('getting-started.kessel-line').textContent).toBe(kesselLine('Kay'))
    expect(screen.queryByTestId('getting-started-set-up-kessel')).toBeNull()
    fireEvent.click(open)
    expect(h.openThread).toHaveBeenCalledWith('ws-kessel')

    h.confirm.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByTestId('getting-started-kessel-welcome'))
    await waitFor(() => expect(h.confirm).toHaveBeenCalledTimes(1))
    expect(h.confirm.mock.calls[0]).toEqual([expect.objectContaining({ message: GETTING_STARTED_COPY.welcomeConfirm })])
    expect(h.daemonCliPost).not.toHaveBeenCalledWith(expect.anything(), 'first-run/kessel/welcome', expect.anything())

    fireEvent.click(screen.getByTestId('getting-started-kessel-welcome'))
    await waitFor(() => expect(screen.getByTestId('getting-started-note').textContent).toBe(GETTING_STARTED_COPY.welcomeSent))
    expect(h.daemonCliPost).toHaveBeenCalledWith(expect.anything(), 'first-run/kessel/welcome', {})
  })
})
