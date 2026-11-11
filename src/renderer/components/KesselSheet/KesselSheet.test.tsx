// @vitest-environment jsdom
//
// The Kessel sheet: step 1 starts from the daemon's kesselDefaults (editable), step
// 2 lists the signed-in claude, codex and grok tools with the server default
// chosen, an Admin creator is told Kessel asks an Owner, Create Kessel posts and then
// opens Kessel's Chat tab in the Thread view, and a refusal is shown plainly.
// Each step is a form (Enter moves on), a server with no tool signed in
// offers Sign in, and Kessel already there says so and opens it. The sheet
// opens centred, and with focus groups on step 1 picks Kessel's focus group.

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
// The focus groups the sheet reads (the real store pulls in the projects
// store and its daemon wiring): a plain store the tests set.
vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  return {
    useFocusGroupsStore: create(() => ({
      focusGroupsEnabled: false,
      focusGroups: [] as Array<{ id: string; name: string; color: string | null; tabOrder: number; createdAt: number }>,
      activeFocusGroupId: null as string | null,
    })),
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
import KesselSheet, {
  KESSEL_CREATE_FAILED,
  createFailedNext,
  KESSEL_MADE_ELSEWHERE,
  KESSEL_BUSY_TEXT,
  KESSEL_CLOSE_HELD,
  KESSEL_TRY_AGAIN,
  KESSEL_EXISTS_TITLE,
  KESSEL_EXISTS_INTRO,
  KESSEL_NO_TOOL_TEXT,
  KESSEL_SHEET_COPY,
  KESSEL_STEP_INTRO,
  defaultKesselTool,
  defaultKesselFocusGroup,
  focusGroupOptions,
  isAdminCreator,
  kesselFolderFor,
  nameProblem,
  signedInKesselTools,
} from './KesselSheet'
import { resetFirstRunForTests } from '@/stores/first-run'
import { resetLlmAccountsForTests, parseAccountsDoc } from '@/stores/llm-accounts'
import { useKesselSheetStore } from '@/stores/kessel-sheet'
import { useFocusGroupsStore, type FocusGroup } from '@/stores/focus-groups'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const DEFAULTS = { name: 'Kessel', handle: 'kessel', path: '/home/r/k2-agents/kessel' }

function status(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { enabled: true, hidden: false, finished: false, offer: 'meet_kessel', kessel: null, kesselDefaults: DEFAULTS, rows: [], ...over }
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
  useKesselSheetStore.setState({ isOpen: true })
  useFocusGroupsStore.setState({ focusGroupsEnabled: false, focusGroups: [], activeFocusGroupId: null })
})

afterEach(() => {
  cleanup()
  useKesselSheetStore.setState({ isOpen: false })
})

async function openSheet(): Promise<void> {
  render(<KesselSheet />)
  await waitFor(() => expect(screen.getByTestId('kessel-name')).toBeTruthy())
}

async function toStep2(): Promise<void> {
  fireEvent.click(screen.getByTestId('kessel-next'))
  await waitFor(() => expect(screen.getByTestId('kessel-create')).toBeTruthy())
}

describe('KesselSheet', () => {
  it('draws nothing while closed', () => {
    useKesselSheetStore.setState({ isOpen: false })
    const { container } = render(<KesselSheet />)
    expect(container.innerHTML).toBe('')
  })

  it('step 1 is filled from kesselDefaults and editable; the folder follows the handle', async () => {
    await openSheet()
    expect(screen.getByText(KESSEL_SHEET_COPY.step1)).toBeTruthy()
    expect((screen.getByTestId('kessel-name') as HTMLInputElement).value).toBe('Kessel')
    expect((screen.getByTestId('kessel-handle') as HTMLInputElement).value).toBe('kessel')
    expect(screen.getByTestId('kessel-folder').textContent).toBe('/home/r/k2-agents/kessel')
    fireEvent.change(screen.getByTestId('kessel-handle'), { target: { value: 'kay' } })
    expect(screen.getByTestId('kessel-folder').textContent).toBe('/home/r/k2-agents/kay')
    fireEvent.change(screen.getByTestId('kessel-handle'), { target: { value: 'a/b' } })
    expect(screen.getByTestId('kessel-handle-problem')).toBeTruthy()
    expect((screen.getByTestId('kessel-next') as HTMLButtonElement).disabled).toBe(true)
  })

  it('step 2 lists only signed-in claude, codex and grok, with the server default chosen', async () => {
    await openSheet()
    await toStep2()
    expect(screen.getByText(KESSEL_SHEET_COPY.step2)).toBeTruthy()
    expect(screen.getByTestId('kessel-tool-claude')).toBeTruthy()
    expect(screen.getByTestId('kessel-tool-codex')).toBeTruthy()
    expect(screen.queryByTestId('kessel-tool-grok')).toBeNull()
    expect(screen.queryByTestId('kessel-tool-gemini')).toBeNull()
    expect((screen.getByTestId('kessel-tool-codex') as HTMLInputElement).checked).toBe(true)
    expect((screen.getByTestId('kessel-tool-claude') as HTMLInputElement).checked).toBe(false)
  })

  it('an Owner or the owner token sees no Admin line', async () => {
    await openSheet()
    await toStep2()
    await act(async () => {})
    expect(screen.queryByTestId('kessel-admin-line')).toBeNull()
  })

  it('an Admin creator is told Kessel asks an Owner', async () => {
    who = { owner: false, role: 'admin' }
    await openSheet()
    await toStep2()
    await waitFor(() => expect(screen.getByTestId('kessel-admin-line').textContent).toBe(KESSEL_SHEET_COPY.adminCreator))
  })

  it('Create Kessel posts the pick, closes and opens Kessel in the Thread', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-kessel', name: 'Kay', path: '/home/r/k2-agents/kay' })
    await openSheet()
    fireEvent.change(screen.getByTestId('kessel-name'), { target: { value: 'Kay' } })
    fireEvent.change(screen.getByTestId('kessel-handle'), { target: { value: 'kay' } })
    await toStep2()
    fireEvent.click(screen.getByTestId('kessel-tool-claude'))
    expect(screen.getByTestId('kessel-create').textContent).toBe(KESSEL_SHEET_COPY.create)
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/kessel', { tool: 'claude', name: 'Kay', handle: 'kay' })
    await waitFor(() => expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-kessel'))
    expect(useKesselSheetStore.getState().isOpen).toBe(false)
  })

  it('a refusal is shown plainly and the sheet stays', async () => {
    h.daemonCliPost.mockRejectedValue(Object.assign(new Error('tool codex is not signed in'), { code: 'tool_not_signed_in' }))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    const err = screen.getByTestId('kessel-error').textContent
    expect(err).toContain(KESSEL_CREATE_FAILED.what)
    expect(err).toContain(KESSEL_CREATE_FAILED.next)
    expect(err).toContain('The server said: tool codex is not signed in.')
    expect(screen.getByTestId('kessel-create').textContent).toBe(KESSEL_TRY_AGAIN)
    expect(useKesselSheetStore.getState().isOpen).toBe(true)
    expect(h.openWorkspaceThread).not.toHaveBeenCalled()
  })

  it('Kessel already made: says so and opens it', async () => {
    h.daemonCliPost.mockRejectedValue(Object.assign(new Error('409'), { code: 'kessel_exists' }))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    expect(screen.getByTestId('kessel-error').textContent).toBe(KESSEL_MADE_ELSEWHERE.line)
    // The status is read again, so the sheet can show the Kessel that's there.
    await waitFor(() => expect(h.daemonCliGet.mock.calls.filter((c) => c[0] === 'first-run/status').length).toBeGreaterThan(1))
    cleanup()
    resetFirstRunForTests()
    statusBody = status({ offer: null, kessel: { workspaceId: 'ws-kessel', name: 'Kessel', path: '/p' } })
    useKesselSheetStore.setState({ isOpen: true })
    render(<KesselSheet />)
    await waitFor(() => expect(screen.getByTestId('kessel-exists')).toBeTruthy())
    expect(screen.getByRole('heading').textContent).toBe(KESSEL_EXISTS_TITLE)
    expect(screen.queryByTestId('kessel-step')).toBeNull()
    fireEvent.click(screen.getByTestId('kessel-open'))
    expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-kessel')
  })

  it('Kessel made meanwhile: Create Kessel turns into the Kessel that is there', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      statusBody = status({ offer: null, kessel: { workspaceId: 'ws-kessel', name: 'Kessel', path: '/home/r/k2-agents/kessel' } })
      throw Object.assign(new Error('409'), { code: 'kessel_exists' })
    })
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    await waitFor(() => expect(screen.getByTestId('kessel-exists')).toBeTruthy())
    expect(screen.getByTestId('kessel-exists').textContent).toContain('/home/r/k2-agents/kessel')
    // Its own words, not the ones for opening the sheet later.
    expect(screen.getByRole('heading').textContent).toBe(KESSEL_MADE_ELSEWHERE.title)
    expect(screen.getByTestId('kessel-intro').textContent).toBe(KESSEL_MADE_ELSEWHERE.intro)
  })

  it('opens centred in the window, as the Settings dialogs do', async () => {
    await openSheet()
    const frame = screen.getByTestId('kessel-sheet').closest('[data-dialog-placement]')
    expect(frame?.getAttribute('data-dialog-placement')).toBe('center')
  })

  it('each step says which it is and what it is for, and names the dialog', async () => {
    await openSheet()
    expect(screen.getByTestId('kessel-step').textContent).toBe('Step 1 of 2')
    expect(screen.getByTestId('kessel-intro').textContent).toBe(KESSEL_STEP_INTRO.step1)
    expect(screen.getByRole('dialog', { name: KESSEL_SHEET_COPY.step1 })).toBeTruthy()
    await toStep2()
    expect(screen.getByTestId('kessel-step').textContent).toBe('Step 2 of 2')
    expect(screen.getByTestId('kessel-intro').textContent).toBe(KESSEL_STEP_INTRO.step2)
    expect(screen.getByRole('dialog', { name: KESSEL_SHEET_COPY.step2 })).toBeTruthy()
  })

  it('Enter moves on: step 1 goes to step 2, and on a tool it creates Kessel', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-kessel', name: 'Kessel', path: '/home/r/k2-agents/kessel' })
    await openSheet()
    fireEvent.submit(screen.getByTestId('kessel-name').closest('form')!)
    await waitFor(() => expect(screen.getByTestId('kessel-create')).toBeTruthy())
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('kessel-tool-codex'), { key: 'Enter' })
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/kessel', { tool: 'codex', name: 'Kessel', handle: 'kessel' })
  })

  it('a blank name says so and Next waits', async () => {
    await openSheet()
    fireEvent.change(screen.getByTestId('kessel-name'), { target: { value: '  ' } })
    expect(screen.getByTestId('kessel-name-problem').textContent).toBe(nameProblem(''))
    expect((screen.getByTestId('kessel-next') as HTMLButtonElement).disabled).toBe(true)
  })

  it('while Kessel is being made, Back, the tools and the × wait, and the × says why', async () => {
    h.daemonCliPost.mockImplementation(() => new Promise(() => {}))
    await openSheet()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    const create = screen.getByTestId('kessel-create') as HTMLButtonElement
    expect(create.disabled).toBe(true)
    expect(create.textContent).toBe('Creating Kessel…')
    expect(screen.getByTestId('kessel-busy').textContent).toBe(KESSEL_BUSY_TEXT)
    expect((screen.getByTestId('kessel-back') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('kessel-tool-claude') as HTMLInputElement).disabled).toBe(true)
    const close = screen.getByTestId('dialog-close') as HTMLButtonElement
    expect(close.disabled).toBe(true)
    expect(close.parentElement!.getAttribute('title')).toBe(KESSEL_CLOSE_HELD)
    expect(screen.getByTestId('kessel-sheet').getAttribute('aria-busy')).toBe('true')
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.getByTestId('kessel-sheet')).toBeTruthy()
  })

  it('no tool signed in: says so and Sign in goes to signing in', async () => {
    accountsBody = { tools: [tool('claude', 'Claude', [{ id: 'a1', state: 'needs_login', kind: 'subscription' }])] }
    await openSheet()
    await toStep2NoTool()
    expect(screen.getByTestId('kessel-no-tool').textContent).toBe(KESSEL_NO_TOOL_TEXT)
    expect(screen.queryByTestId('kessel-create')).toBeNull()
    fireEvent.click(screen.getByTestId('kessel-sign-in'))
    expect(h.requestLlmSignIn).toHaveBeenCalledTimes(1)
    expect(useKesselSheetStore.getState().isOpen).toBe(false)
  })
})

/** An open Focus group list's option, by its value. */
async function pickOption(value: string): Promise<HTMLElement> {
  const menu = await screen.findByTestId('setting-dropdown-menu')
  const option = menu.querySelector<HTMLElement>(`[data-value="${value}"]`)
  if (!option) throw new Error(`no option ${value}`)
  return option
}

const GROUPS: FocusGroup[] = [
  { id: 'fg-ops', name: 'Ops', color: null, tabOrder: 0, createdAt: 1 },
  { id: 'fg-home', name: 'Home', color: null, tabOrder: 1, createdAt: 1 },
]

describe('KesselSheet focus group', () => {
  it('focus groups off: no picker, and Create Kessel sends no focusGroup', async () => {
    useFocusGroupsStore.setState({ focusGroupsEnabled: false, focusGroups: GROUPS, activeFocusGroupId: 'fg-ops' })
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-kessel', name: 'Kessel', path: '/home/r/k2-agents/kessel' })
    await openSheet()
    expect(screen.queryByTestId('kessel-focus-group')).toBeNull()
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/kessel', { tool: 'codex', name: 'Kessel', handle: 'kessel' })
  })

  it('focus groups on with no groups: no picker', async () => {
    useFocusGroupsStore.setState({ focusGroupsEnabled: true, focusGroups: [], activeFocusGroupId: null })
    await openSheet()
    expect(screen.queryByTestId('kessel-focus-group')).toBeNull()
  })

  it('focus groups on: the picker starts on the active group and Create Kessel posts it', async () => {
    useFocusGroupsStore.setState({ focusGroupsEnabled: true, focusGroups: GROUPS, activeFocusGroupId: 'fg-home' })
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-kessel', name: 'Kessel', path: '/home/r/k2-agents/kessel' })
    await openSheet()
    const picker = screen.getByTestId('kessel-focus-group')
    expect(picker.getAttribute('aria-label')).toBe('Focus group')
    expect(picker.textContent).toContain('Home')
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/kessel', { tool: 'codex', name: 'Kessel', handle: 'kessel', focusGroup: 'fg-home' })
  })

  it('a group picked in the sheet is the one posted; No group posts none', async () => {
    useFocusGroupsStore.setState({ focusGroupsEnabled: true, focusGroups: GROUPS, activeFocusGroupId: 'fg-home' })
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-kessel', name: 'Kessel', path: '/home/r/k2-agents/kessel' })
    await openSheet()
    fireEvent.click(screen.getByTestId('kessel-focus-group'))
    fireEvent.click(await pickOption('fg-ops'))
    expect(screen.getByTestId('kessel-focus-group').textContent).toContain('Ops')
    fireEvent.click(screen.getByTestId('kessel-focus-group'))
    fireEvent.click(await pickOption(''))
    expect(screen.getByTestId('kessel-focus-group').textContent).toContain('No group')
    await toStep2()
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-create'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/kessel', { tool: 'codex', name: 'Kessel', handle: 'kessel' })
  })
})

async function toStep2NoTool(): Promise<void> {
  fireEvent.click(screen.getByTestId('kessel-next'))
  await waitFor(() => expect(screen.getByTestId('kessel-sign-in')).toBeTruthy())
}

describe('Kessel sheet helpers', () => {
  it('says Kessel, in the words K2 Design approved', () => {
    expect(KESSEL_STEP_INTRO.step1).toBe('Kessel is K2\u2019s own agent. It works in its own folder on this server.')
    expect(KESSEL_SHEET_COPY.step2).toBe('Choose Kessel\u2019s tool')
    expect(KESSEL_SHEET_COPY.create).toBe('Create Kessel')
    expect(KESSEL_MADE_ELSEWHERE.intro).toBe('This one wasn\u2019t made, since a server has one Kessel. Open it to talk with it in its Thread.')
    expect(KESSEL_EXISTS_TITLE).toBe('Kessel is already on this server')
    expect(KESSEL_EXISTS_INTRO).toBe('Open Kessel to talk with it in its Thread.')
    expect(KESSEL_BUSY_TEXT).toBe('Making Kessel\u2019s folder and starting it\u2026')
    expect(KESSEL_NO_TOOL_TEXT.endsWith('then come back to make Kessel.')).toBe(true)
    expect(KESSEL_CREATE_FAILED.what).toBe('K2 couldn\u2019t create Kessel.')
  })

  it('the focus-group picker is Workspace settings\u2019 options, starting on the active group', () => {
    expect(focusGroupOptions(GROUPS)).toEqual([
      { value: '', label: 'No group' },
      { value: 'fg-ops', label: 'Ops' },
      { value: 'fg-home', label: 'Home' },
    ])
    expect(defaultKesselFocusGroup(GROUPS, 'fg-home')).toBe('fg-home')
    expect(defaultKesselFocusGroup(GROUPS, 'fg-gone')).toBe('')
    expect(defaultKesselFocusGroup(GROUPS, null)).toBe('')
  })

  it('a folder K2 cannot write gets its own next step; anything else the general one', () => {
    expect(createFailedNext("couldn't make the folder /home/a/k2-agents/kessel: Permission denied")).toBe(KESSEL_CREATE_FAILED.permission)
    expect(createFailedNext('EACCES: open /x')).toBe(KESSEL_CREATE_FAILED.permission)
    expect(createFailedNext('tool codex is not signed in')).toBe(KESSEL_CREATE_FAILED.next)
  })

  it('signed-in tools follow the llm_sign_in rule', () => {
    expect(signedInKesselTools(parseAccountsDoc(ACCOUNTS)).map((t) => t.tool)).toEqual(['claude', 'codex'])
    expect(signedInKesselTools(null)).toEqual([])
  })

  it('the server default is chosen when signed in, else the first tool', () => {
    const tools = [
      { tool: 'claude', display: 'Claude' },
      { tool: 'codex', display: 'Codex' },
    ]
    expect(defaultKesselTool(tools, 'codex')).toBe('codex')
    expect(defaultKesselTool(tools, 'grok')).toBe('claude')
    expect(defaultKesselTool([], 'claude')).toBeNull()
  })

  it('an Admin creator is an Admin login, not an Owner or the owner token', () => {
    expect(isAdminCreator({ owner: false, role: 'admin' })).toBe(true)
    expect(isAdminCreator({ owner: true, role: 'owner' })).toBe(false)
    expect(isAdminCreator({ owner: true })).toBe(false)
    expect(isAdminCreator({ role: 'member' })).toBe(false)
    expect(isAdminCreator(null)).toBe(false)
  })

  it('the folder is the default folder parent plus the handle', () => {
    expect(kesselFolderFor(DEFAULTS, ' kay ')).toBe('/home/r/k2-agents/kay')
    expect(kesselFolderFor(null, 'kessel')).toBe('~/k2-agents/kessel')
  })
})
