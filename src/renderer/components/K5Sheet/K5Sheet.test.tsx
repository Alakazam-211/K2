// @vitest-environment jsdom
//
// The K5 sheet: step 1 starts from the daemon's k5Defaults (editable), step
// 2 lists the signed-in claude, codex and grok tools with the server default
// chosen, an Admin creator is told K5 asks an Owner, Create K5 posts and then
// opens K5's Chat tab in the Thread view, and a refusal is shown plainly.
// Each step is a form (Enter moves on), a server with no tool signed in
// offers Sign in, and K5 already there says so and opens it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openWorkspaceThread: vi.fn(async (_id: string) => true),
  defaultAgent: 'codex',
  requestLlmSignIn: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})
vi.mock('@/stores/session-events', () => {
  const off = () => () => {}
  return {
    onFirstRunChanged: off,
    onLlmAccountsChanged: off,
    onSubscriptionUsageChanged: off,
    onProjectsChanged: off,
    onTunnelStatusChanged: off,
    onTunnelIdentityChanged: off,
    onPairCodeChanged: off,
    onAppHello: off,
  }
})
vi.mock('@/stores/llm-sign-in-request', () => ({ requestLlmSignIn: () => h.requestLlmSignIn() }))
vi.mock('@/lib/open-workspace-thread', () => ({ openWorkspaceThread: (id: string) => h.openWorkspaceThread(id) }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { defaultAgent: string }) => unknown) => sel({ defaultAgent: h.defaultAgent }),
}))
vi.mock('@/stores/presets', () => ({
  usePresetsStore: (sel: (s: { presets: unknown[] }) => unknown) => sel({ presets: [] }),
}))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import K5Sheet, {
  K5_CREATE_FAILED,
  K5_EXISTS_TEXT,
  K5_EXISTS_TITLE,
  K5_NO_TOOL_TEXT,
  K5_SHEET_COPY,
  K5_STEP_INTRO,
  defaultK5Tool,
  isAdminCreator,
  k5FolderFor,
  nameProblem,
  signedInK5Tools,
} from './K5Sheet'
import { resetFirstRunForTests } from '@/stores/first-run'
import { resetLlmAccountsForTests, parseAccountsDoc } from '@/stores/llm-accounts'
import { useK5SheetStore } from '@/stores/k5-sheet'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const DEFAULTS = { name: 'K5', handle: 'k5', path: '/home/r/k2-agents/k5' }

function status(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { enabled: true, hidden: false, finished: false, offer: 'meet_k5', k5: null, k5Defaults: DEFAULTS, rows: [], ...over }
}

function tool(name: string, display: string, accounts: Array<Record<string, unknown>>): Record<string, unknown> {
  return { tool: name, display, supported: true, accounts }
}

const ACCOUNTS = {
  tools: [
    tool('claude', 'Claude', [{ id: 'a1', state: 'signed_in', kind: 'subscription' }]),
    tool('codex', 'Codex', [{ id: 'a2', state: 'needs_login' }, { id: 'a3', state: 'unknown', kind: 'api_key' }]),
    tool('grok', 'Grok', [{ id: 'a4', state: 'needs_login', kind: 'subscription' }]),
    tool('gemini', 'Gemini', [{ id: 'a5', state: 'signed_in', kind: 'api_key' }]),
  ],
}

let who: Record<string, unknown> = { owner: true, role: 'owner' }
let accountsBody: Record<string, unknown> = ACCOUNTS
let statusBody: Record<string, unknown> = status()

beforeEach(() => {
  resetFirstRunForTests()
  resetLlmAccountsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openWorkspaceThread.mockClear()
  h.requestLlmSignIn.mockClear()
  h.defaultAgent = 'codex'
  who = { owner: true, role: 'owner' }
  statusBody = status()
  accountsBody = ACCOUNTS
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'first-run/status') return statusBody
    if (route === 'llm/accounts/list') return accountsBody
    if (route === 'auth/whoami') return who
    throw new Error(`unexpected GET ${route}`)
  })
  useK5SheetStore.setState({ isOpen: true })
})

afterEach(() => {
  cleanup()
  useK5SheetStore.setState({ isOpen: false })
})

async function openSheet(): Promise<void> {
  render(<K5Sheet />)
  await waitFor(() => expect(screen.getByTestId('k5-name')).toBeTruthy())
}

async function toStep2(): Promise<void> {
  fireEvent.click(screen.getByTestId('k5-next'))
  await waitFor(() => expect(screen.getByTestId('k5-create')).toBeTruthy())
}

describe('K5Sheet', () => {
  it('draws nothing while closed', () => {
    useK5SheetStore.setState({ isOpen: false })
    const { container } = render(<K5Sheet />)
    expect(container.innerHTML).toBe('')
  })

  it('step 1 is filled from k5Defaults and editable; the folder follows the handle', async () => {
    await openSheet()
    expect(screen.getByText(K5_SHEET_COPY.step1)).toBeTruthy()
    expect((screen.getByTestId('k5-name') as HTMLInputElement).value).toBe('K5')
    expect((screen.getByTestId('k5-handle') as HTMLInputElement).value).toBe('k5')
    expect(screen.getByTestId('k5-folder').textContent).toBe('/home/r/k2-agents/k5')
    fireEvent.change(screen.getByTestId('k5-handle'), { target: { value: 'kay' } })
    expect(screen.getByTestId('k5-folder').textContent).toBe('/home/r/k2-agents/kay')
    fireEvent.change(screen.getByTestId('k5-handle'), { target: { value: 'a/b' } })
    expect(screen.getByTestId('k5-handle-problem')).toBeTruthy()
    expect((screen.getByTestId('k5-next') as HTMLButtonElement).disabled).toBe(true)
  })

  it('step 2 lists only signed-in claude, codex and grok, with the server default chosen', async () => {
    await openSheet()
    await toStep2()
    expect(screen.getByText(K5_SHEET_COPY.step2)).toBeTruthy()
    expect(screen.getByTestId('k5-tool-claude')).toBeTruthy()
    expect(screen.getByTestId('k5-tool-codex')).toBeTruthy()
    expect(screen.queryByTestId('k5-tool-grok')).toBeNull()
    expect(screen.queryByTestId('k5-tool-gemini')).toBeNull()
    expect((screen.getByTestId('k5-tool-codex') as HTMLInputElement).checked).toBe(true)
    expect((screen.getByTestId('k5-tool-claude') as HTMLInputElement).checked).toBe(false)
  })

  it('an Owner or the owner token sees no Admin line', async () => {
    await openSheet()
    await toStep2()
    await act(async () => {})
    expect(screen.queryByTestId('k5-admin-line')).toBeNull()
  })

  it('an Admin creator is told K5 asks an Owner', async () => {
    who = { owner: false, role: 'admin' }
    await openSheet()
    await toStep2()
    await waitFor(() => expect(screen.getByTestId('k5-admin-line').textContent).toBe(K5_SHEET_COPY.adminCreator))
  })

  it('Create K5 posts the pick, closes and opens K5 in the Thread', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-k5', name: 'Kay', path: '/home/r/k2-agents/kay' })
    await openSheet()
    fireEvent.change(screen.getByTestId('k5-name'), { target: { value: 'Kay' } })
    fireEvent.change(screen.getByTestId('k5-handle'), { target: { value: 'kay' } })
    await toStep2()
    fireEvent.click(screen.getByTestId('k5-tool-claude'))
    expect(screen.getByTestId('k5-create').textContent).toBe(K5_SHEET_COPY.create)
    await act(async () => {
      fireEvent.click(screen.getByTestId('k5-create'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/k5', { tool: 'claude', name: 'Kay', handle: 'kay' })
    await waitFor(() => expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-k5'))
    expect(useK5SheetStore.getState().isOpen).toBe(false)
  })

  it('a refusal is shown plainly and the sheet stays', async () => {
    h.daemonCliPost.mockRejectedValue(Object.assign(new Error('tool codex is not signed in'), { code: 'tool_not_signed_in' }))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('k5-create'))
    })
    const err = screen.getByTestId('k5-error').textContent
    expect(err).toContain(K5_CREATE_FAILED.what)
    expect(err).toContain(K5_CREATE_FAILED.next)
    expect(err).toContain('The server said: tool codex is not signed in.')
    expect(useK5SheetStore.getState().isOpen).toBe(true)
    expect(h.openWorkspaceThread).not.toHaveBeenCalled()
  })

  it('K5 already made: says so and opens it', async () => {
    h.daemonCliPost.mockRejectedValue(Object.assign(new Error('409'), { code: 'k5_exists' }))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('k5-create'))
    })
    expect(screen.getByTestId('k5-error').textContent).toBe(K5_EXISTS_TEXT)
    // The status is read again, so the sheet can show the K5 that's there.
    await waitFor(() => expect(h.daemonCliGet.mock.calls.filter((c) => c[0] === 'first-run/status').length).toBeGreaterThan(1))
    cleanup()
    resetFirstRunForTests()
    statusBody = status({ offer: null, k5: { workspaceId: 'ws-k5', name: 'K5', path: '/p' } })
    useK5SheetStore.setState({ isOpen: true })
    render(<K5Sheet />)
    await waitFor(() => expect(screen.getByTestId('k5-exists')).toBeTruthy())
    expect(screen.getByRole('heading').textContent).toBe(K5_EXISTS_TITLE)
    expect(screen.queryByTestId('k5-step')).toBeNull()
    fireEvent.click(screen.getByTestId('k5-open'))
    expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-k5')
  })

  it('K5 made meanwhile: Create K5 turns into the K5 that is there', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      statusBody = status({ offer: null, k5: { workspaceId: 'ws-k5', name: 'K5', path: '/home/r/k2-agents/k5' } })
      throw Object.assign(new Error('409'), { code: 'k5_exists' })
    })
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('k5-create'))
    })
    await waitFor(() => expect(screen.getByTestId('k5-exists')).toBeTruthy())
    expect(screen.getByTestId('k5-exists').textContent).toContain('/home/r/k2-agents/k5')
  })

  it('each step says which it is and what it is for, and names the dialog', async () => {
    await openSheet()
    expect(screen.getByTestId('k5-step').textContent).toBe('Step 1 of 2')
    expect(screen.getByTestId('k5-intro').textContent).toBe(K5_STEP_INTRO.step1)
    expect(screen.getByRole('dialog', { name: K5_SHEET_COPY.step1 })).toBeTruthy()
    await toStep2()
    expect(screen.getByTestId('k5-step').textContent).toBe('Step 2 of 2')
    expect(screen.getByTestId('k5-intro').textContent).toBe(K5_STEP_INTRO.step2)
    expect(screen.getByRole('dialog', { name: K5_SHEET_COPY.step2 })).toBeTruthy()
  })

  it('Enter moves on: step 1 goes to step 2, and on a tool it creates K5', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-k5', name: 'K5', path: '/home/r/k2-agents/k5' })
    await openSheet()
    fireEvent.submit(screen.getByTestId('k5-name').closest('form')!)
    await waitFor(() => expect(screen.getByTestId('k5-create')).toBeTruthy())
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('k5-tool-codex'), { key: 'Enter' })
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/k5', { tool: 'codex', name: 'K5', handle: 'k5' })
  })

  it('a blank name says so and Next waits', async () => {
    await openSheet()
    fireEvent.change(screen.getByTestId('k5-name'), { target: { value: '  ' } })
    expect(screen.getByTestId('k5-name-problem').textContent).toBe(nameProblem(''))
    expect((screen.getByTestId('k5-next') as HTMLButtonElement).disabled).toBe(true)
  })

  it('while K5 is being made, Back, the tools and the × wait', async () => {
    h.daemonCliPost.mockImplementation(() => new Promise(() => {}))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('k5-create'))
    })
    const create = screen.getByTestId('k5-create') as HTMLButtonElement
    expect(create.disabled).toBe(true)
    expect(create.textContent).toBe('Creating K5…')
    expect((screen.getByTestId('k5-back') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('k5-tool-claude') as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByTestId('dialog-close') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('k5-sheet').getAttribute('aria-busy')).toBe('true')
  })

  it('no tool signed in: says so and Sign in goes to signing in', async () => {
    accountsBody = { tools: [tool('claude', 'Claude', [{ id: 'a1', state: 'needs_login', kind: 'subscription' }])] }
    await openSheet()
    await toStep2NoTool()
    expect(screen.getByTestId('k5-no-tool').textContent).toBe(K5_NO_TOOL_TEXT)
    expect(screen.queryByTestId('k5-create')).toBeNull()
    fireEvent.click(screen.getByTestId('k5-sign-in'))
    expect(h.requestLlmSignIn).toHaveBeenCalledTimes(1)
    expect(useK5SheetStore.getState().isOpen).toBe(false)
  })
})

async function toStep2NoTool(): Promise<void> {
  fireEvent.click(screen.getByTestId('k5-next'))
  await waitFor(() => expect(screen.getByTestId('k5-sign-in')).toBeTruthy())
}

describe('K5 sheet helpers', () => {
  it('signed-in tools follow the llm_sign_in rule', () => {
    expect(signedInK5Tools(parseAccountsDoc(ACCOUNTS)).map((t) => t.tool)).toEqual(['claude', 'codex'])
    expect(signedInK5Tools(null)).toEqual([])
  })

  it('the server default is chosen when signed in, else the first tool', () => {
    const tools = [
      { tool: 'claude', display: 'Claude' },
      { tool: 'codex', display: 'Codex' },
    ]
    expect(defaultK5Tool(tools, 'codex')).toBe('codex')
    expect(defaultK5Tool(tools, 'grok')).toBe('claude')
    expect(defaultK5Tool([], 'claude')).toBeNull()
  })

  it('an Admin creator is an Admin login, not an Owner or the owner token', () => {
    expect(isAdminCreator({ owner: false, role: 'admin' })).toBe(true)
    expect(isAdminCreator({ owner: true, role: 'owner' })).toBe(false)
    expect(isAdminCreator({ owner: true })).toBe(false)
    expect(isAdminCreator({ role: 'member' })).toBe(false)
    expect(isAdminCreator(null)).toBe(false)
  })

  it('the folder is the default folder parent plus the handle', () => {
    expect(k5FolderFor(DEFAULTS, ' kay ')).toBe('/home/r/k2-agents/kay')
    expect(k5FolderFor(null, 'k5')).toBe('~/k2-agents/k5')
  })
})
