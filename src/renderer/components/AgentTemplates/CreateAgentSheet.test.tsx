// @vitest-environment jsdom
//
// Create an agent (prd-agent-templates-v1 §6.1): Pick hides system
// templates and puts General first, then the built-ins, then "Yours" with its
// author; the handle and folder come from the name (both editable); the
// template's params and questions are asked; the tool step shows sign-in
// state with the suggestion chosen; the Review draws every dry-run row; Create
// posts for real, closes, opens the Thread and says hello; a refusal says so
// with Try again; an older server says it can't; closing is held while
// creating; each step's heading takes focus.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  daemonCliPostQuery: vi.fn(),
  openWorkspaceThread: vi.fn(async (_id: string) => true),
  requestLlmSignIn: vi.fn(),
  defaultAgent: 'claude',
  presets: [] as unknown[],
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
    daemonCliPostQuery: primaryOnly((...a: unknown[]) => h.daemonCliPostQuery(...a)),
  }
})
vi.mock('@/stores/session-events', () => {
  const off = () => () => {}
  return { onLlmAccountsChanged: off, onSubscriptionUsageChanged: off }
})
vi.mock('@/stores/llm-sign-in-request', () => ({ requestLlmSignIn: (r: unknown) => h.requestLlmSignIn(r) }))
vi.mock('@/lib/open-workspace-thread', () => ({ openWorkspaceThread: (id: string) => h.openWorkspaceThread(id) }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { defaultAgent: string }) => unknown) => sel({ defaultAgent: h.defaultAgent }),
}))
vi.mock('@/stores/presets', () => {
  const state = () => ({ presets: h.presets, presetsLoaded: true, fetchPresets: async () => {} })
  const usePresetsStore = (sel: (s: ReturnType<typeof state>) => unknown) => sel(state())
  usePresetsStore.getState = state
  return { usePresetsStore }
})

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import CreateAgentSheet, {
  CREATE_AGENT_CLOSE_HELD,
  CREATE_AGENT_COPY,
  CREATE_AGENT_NO_TOOL,
  CREATE_AGENT_NOT_SIGNED_IN,
  NAME_GIVES_NO_HANDLE,
  buildHireRequest,
  paramProblem,
  nameStepProblems,
  defaultPresetFor,
  folderFor,
  handleFromName,
} from './CreateAgentSheet'
import { CREATE_UNSUPPORTED_TEXT, HIRE_STATUS_LABELS } from '@/lib/agent-templates-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { resetAgentTemplatesForTests, useAgentTemplatesStore } from '@/stores/agent-templates'
import { resetLlmAccountsForTests } from '@/stores/llm-accounts'
import type { AgentPreset } from '@/stores/presets'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function card(id: string, over: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, version: 1, name: id[0].toUpperCase() + id.slice(1), summary: `${id} summary`, kind: 'agent', aliases: [], author: 'K2', origin: 'builtin', hash: 'h', ...over }
}

// The daemon's list order: General first already, but the sheet must not rely on it.
const LIST = {
  ok: true,
  problems: [],
  templates: [
    card('writer'),
    card('general'),
    card('k5', { kind: 'system', name: 'K5' }),
    card('my-writer', { origin: 'library', author: 'user:rosson', name: 'My writer' }),
    card('developer'),
  ],
}

function detail(id: string, over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ...card(id),
    sharedBlock: true,
    agent: { role: id, tool: 'default', model: null },
    catalog: ['wiki:index'],
    permissions: { cards: true, suggest: [], expanded: [] },
    params: {},
    wiki: [],
    layers: [],
    skills: [],
    heartbeats: [],
    hello: 'Say hello',
    questions: [],
    dir: '',
    ...over,
  }
}

const DETAILS: Record<string, Record<string, unknown>> = {
  general: detail('general'),
  writer: detail('writer', {
    agent: { role: 'writer', tool: 'codex', model: null },
    params: { about: { label: 'What should it focus on?', max: 500, required: true } },
    questions: [{ id: 'voice', ask: 'How would you describe your voice?', fills: 'Voice and style.md' }],
  }),
  'my-writer': detail('my-writer', { origin: 'library', author: 'user:rosson' }),
}

const ROWS = [
  { step: 'folder', target: '/home/r/agents/max', status: 'write' },
  { step: 'register', target: '/home/r/agents/max', status: 'write' },
  { step: 'role', target: '.k2/agent/ROLE.md', status: 'write' },
  { step: 'wiki', target: '.k2/wiki/Voice and style.md', status: 'write' },
  { step: 'catalog', target: 'wiki:index', status: 'write' },
  { step: 'permission', target: 'permission-cards', status: 'suggest' },
  { step: 'permission', target: 'compute.use', status: 'suggest' },
  { step: 'heartbeat', target: 'daily-digest', status: 'skip', reason: 'a heartbeat with this name exists' },
  { step: 'lock', target: '.k2/agent/template.lock', status: 'write' },
  { step: 'hello', target: 'thread', status: 'write' },
]

function plan(dryRun: boolean): Record<string, unknown> {
  return {
    ok: true,
    path: '/home/r/agents/max',
    id: dryRun ? '' : 'ws-max',
    workspaceId: dryRun ? '' : 'ws-max',
    handle: 'max',
    name: 'Max',
    created: true,
    changed: !dryRun,
    dryRun,
    template: { id: 'writer', version: 1, hash: 'h', kind: 'agent', name: 'Writer', origin: 'builtin', author: 'K2' },
    steps: [],
    rows: ROWS,
    charter: 'You write for Rosson.',
    permissions: { cards: true, suggested: ['compute.use'], granted: [], carded: [] },
    hello: dryRun ? null : 'Say hello to Rosson.',
    questions: [{ id: 'voice', ask: 'How would you describe your voice?', fills: 'Voice and style.md', answered: false }],
  }
}

function preset(id: string, label: string, command: string): AgentPreset {
  return { id, label, command, icon: null, enabled: 1, sortOrder: 0, isBuiltIn: 1, createdAt: 0 }
}

const ACCOUNTS = {
  tools: [
    { tool: 'claude', display: 'Claude', supported: true, accounts: [{ id: 'a1', state: 'signed_in', kind: 'subscription' }] },
    { tool: 'codex', display: 'Codex', supported: true, accounts: [{ id: 'a2', state: 'needs_login', kind: 'subscription' }] },
  ],
}

let listBody: unknown = LIST

beforeEach(() => {
  resetAgentTemplatesForTests()
  resetLlmAccountsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliPostQuery.mockReset()
  h.daemonCliPostQuery.mockResolvedValue({ success: true })
  h.openWorkspaceThread.mockClear()
  h.requestLlmSignIn.mockClear()
  h.defaultAgent = 'claude'
  h.presets = [preset('p-claude', 'Claude', 'claude'), preset('p-codex', 'Codex', 'codex --yolo'), preset('p-aider', 'Aider', 'aider')]
  listBody = LIST
  h.daemonCliGet.mockImplementation(async (route: string, params?: { id?: string }) => {
    if (route === 'agent-templates') {
      if (listBody instanceof Error) throw listBody
      return listBody
    }
    if (route === 'agent-templates/show') return { ok: true, template: DETAILS[params?.id ?? ''] }
    if (route === 'agents/hire-root') return { ok: true, root: '/home/r/agents/', default: true }
    if (route === 'llm/accounts/list') return ACCOUNTS
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: { dryRun: boolean }) => {
    if (route === 'agents/hire') return plan(body.dryRun)
    throw new Error(`unexpected POST ${route}`)
  })
  useAgentTemplatesStore.setState({ createOpen: true })
})

afterEach(() => {
  cleanup()
})

async function openSheet(): Promise<void> {
  render(<CreateAgentSheet />)
  await waitFor(() => expect(screen.getByTestId('create-agent-template-general')).toBeTruthy())
}

async function next(): Promise<void> {
  await act(async () => {
    fireEvent.click(screen.getByTestId('create-agent-next'))
  })
}

async function toName(id = 'writer'): Promise<void> {
  await openSheet()
  fireEvent.click(screen.getByTestId(`create-agent-template-${id}`))
  await next()
  await waitFor(() => expect(screen.getByTestId('create-agent-name')).toBeTruthy())
}

async function toReview(): Promise<void> {
  await toName('writer')
  fireEvent.change(screen.getByTestId('create-agent-name'), { target: { value: 'Max' } })
  fireEvent.change(screen.getByTestId('create-agent-param-about'), { target: { value: 'Blog posts' } })
  await next()
  await waitFor(() => expect(screen.getByTestId('create-agent-tools')).toBeTruthy())
  await next()
  await waitFor(() => expect(screen.getByTestId('create-agent-rows')).toBeTruthy())
}

describe('helpers', () => {
  it('the handle comes from the name and the folder from the root', () => {
    expect(handleFromName('Max the Writer')).toBe('max-the-writer')
    expect(handleFromName('  ')).toBe('')
    expect(folderFor('/home/r/agents/', 'max')).toBe('/home/r/agents/max')
    expect(folderFor(null, 'max')).toBeNull()
  })

  it('the folder is sent only when the person changed it', () => {
    const base = { template: 'writer', name: ' Max ', handle: 'Max', defaultFolder: '/r/max', preset: 'p-codex', params: { about: ' x ', other: '' }, answers: {}, dryRun: true }
    expect(buildHireRequest({ ...base, folder: '/r/max' })).toEqual({
      template: 'writer',
      name: 'Max',
      handle: 'max',
      preset: 'p-codex',
      params: { about: 'x' },
      dryRun: true,
    })
    expect(buildHireRequest({ ...base, folder: '/elsewhere/max' }).path).toBe('/elsewhere/max')
  })

  it('name step: while the handle and folder follow the name, only the name’s problem shows', () => {
    const base = { handleEdited: false, folderEdited: false }
    expect(nameStepProblems({ ...base, name: '', handle: '', folder: '' })).toEqual({ name: 'Give the agent a name.', handle: null, folder: null })
    expect(nameStepProblems({ ...base, name: '!!!', handle: '', folder: '' })).toEqual({ name: NAME_GIVES_NO_HANDLE, handle: null, folder: null })
    expect(nameStepProblems({ ...base, name: 'Max', handle: 'max', folder: '/r/max' })).toEqual({ name: null, handle: null, folder: null })
    // Edited: each says its own.
    expect(nameStepProblems({ name: 'Max', handle: 'a b/c', folder: '', handleEdited: true, folderEdited: false })).toEqual({ name: null, handle: 'Use letters, numbers and hyphens.', folder: null })
    expect(nameStepProblems({ name: '', handle: 'max', folder: 'rel/path', handleEdited: true, folderEdited: true })).toEqual({
      name: 'Give the agent a name.',
      handle: null,
      folder: 'Give the full path to the folder.',
    })
  })

  it('the template’s tool is chosen first, else the server default', () => {
    const ps = [preset('p-claude', 'Claude', 'claude'), preset('p-codex', 'Codex', 'codex')]
    expect(defaultPresetFor(ps, 'codex', 'claude')).toBe('p-codex')
    expect(defaultPresetFor(ps, 'default', 'claude')).toBe('p-claude')
    expect(defaultPresetFor(ps, 'grok', 'p-codex')).toBe('p-codex')
  })
})

describe('CreateAgentSheet', () => {
  it('draws nothing while closed', () => {
    useAgentTemplatesStore.setState({ createOpen: false })
    const { container } = render(<CreateAgentSheet />)
    expect(container.innerHTML).toBe('')
  })

  it('Pick: system hidden, General first, then built-ins, then Yours with the author', async () => {
    await openSheet()
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe(CREATE_AGENT_COPY.pick)
    const builtins = within(screen.getByTestId('create-agent-builtins'))
      .getAllByRole('radio')
      .map((r) => (r as HTMLInputElement).value)
    expect(builtins).toEqual(['general', 'writer', 'developer'])
    expect(screen.queryByTestId('create-agent-template-k5')).toBeNull()
    const yours = within(screen.getByTestId('create-agent-yours')).getAllByRole('radio')
    expect(yours.map((r) => (r as HTMLInputElement).value)).toEqual(['my-writer'])
    expect(screen.getByTestId('create-agent-author-my-writer').textContent).toBe('By rosson')
    expect((screen.getByTestId('create-agent-template-general') as HTMLInputElement).checked).toBe(true)
  })

  it('name step: clearing the name shows only the name’s problem; the handle’s shows once it’s edited', async () => {
    await toName('writer')
    fireEvent.change(screen.getByTestId('create-agent-param-about'), { target: { value: 'Posts' } })
    fireEvent.change(screen.getByTestId('create-agent-name'), { target: { value: '' } })
    await next()
    expect(screen.getByTestId('create-agent-name-problem').textContent).toBe('Give the agent a name.')
    expect(screen.queryByTestId('create-agent-handle-problem')).toBeNull()
    expect(screen.queryByTestId('create-agent-folder-problem')).toBeNull()
    expect(screen.queryByTestId('create-agent-tools')).toBeNull()
    // Edited (typed, then cleared): now the handle says its own.
    fireEvent.change(screen.getByTestId('create-agent-handle'), { target: { value: 'x' } })
    fireEvent.change(screen.getByTestId('create-agent-handle'), { target: { value: '' } })
    expect(screen.getByTestId('create-agent-handle-problem').textContent).toBe('Give the agent a handle.')
    expect(screen.queryByTestId('create-agent-folder-problem')).toBeNull()
  })

  it('opens on a starting template (Save as template’s Create an agent from it), else on the first card', async () => {
    useAgentTemplatesStore.getState().openCreate('my-writer')
    await openSheet()
    expect((screen.getByTestId('create-agent-template-my-writer') as HTMLInputElement).checked).toBe(true)
    expect((screen.getByTestId('create-agent-template-general') as HTMLInputElement).checked).toBe(false)
    cleanup()
    useAgentTemplatesStore.getState().closeCreate()
    expect(useAgentTemplatesStore.getState().createStart).toBeNull()
    useAgentTemplatesStore.getState().openCreate('gone')
    await openSheet()
    expect((screen.getByTestId('create-agent-template-general') as HTMLInputElement).checked).toBe(true)
  })

  it('the charter caps its headings: # reads at the ## size', async () => {
    await toReview()
    expect(screen.getByTestId('create-agent-charter').querySelector('.markdown-content[data-heading-cap]')).toBeTruthy()
    const css = readFileSync(join(__dirname, '..', '..', 'globals.css'), 'utf8')
    const size = (sel: string): string | undefined => new RegExp(`${sel.replace(/[.[\]]/g, '\\$&')} \\{[^}]*font-size: ([^;]+);`).exec(css)?.[1]
    expect(size('.markdown-content[data-heading-cap] h1')).toBe(size('.markdown-content h2'))
    expect(size('.markdown-content h2')).toBe('1.4em')
    expect(size('.markdown-content h3')).toBe('1.15em')
  })

  it('focus moves to each step’s heading', async () => {
    await openSheet()
    expect(document.activeElement?.textContent).toBe(CREATE_AGENT_COPY.pick)
    fireEvent.click(screen.getByTestId('create-agent-template-writer'))
    await next()
    await waitFor(() => expect(document.activeElement?.textContent).toBe(CREATE_AGENT_COPY.name))
  })

  it('Name and folder: the handle follows the name and the folder the handle; both editable', async () => {
    await toName('writer')
    const name = screen.getByTestId('create-agent-name') as HTMLInputElement
    const handle = screen.getByTestId('create-agent-handle') as HTMLInputElement
    const folder = screen.getByTestId('create-agent-folder') as HTMLInputElement
    expect(name.value).toBe('Writer')
    expect(handle.value).toBe('writer')
    expect(folder.value).toBe('/home/r/agents/writer')
    fireEvent.change(name, { target: { value: 'Max Power' } })
    expect(handle.value).toBe('max-power')
    expect(folder.value).toBe('/home/r/agents/max-power')
    fireEvent.change(handle, { target: { value: 'maxp' } })
    expect(folder.value).toBe('/home/r/agents/maxp')
    fireEvent.change(name, { target: { value: 'Other' } })
    expect(handle.value).toBe('maxp')
    fireEvent.change(folder, { target: { value: '/srv/maxp' } })
    fireEvent.change(handle, { target: { value: 'z' } })
    expect(folder.value).toBe('/srv/maxp')
  })

  it('asks the template’s params and questions; a required param holds Next', async () => {
    await toName('writer')
    expect(screen.getByLabelText(/What should it focus on/)).toBeTruthy()
    expect(screen.getByLabelText('How would you describe your voice?')).toBeTruthy()
    await next()
    expect(screen.getByTestId('create-agent-param-about-problem')).toBeTruthy()
    expect(screen.queryByTestId('create-agent-tools')).toBeNull()
    fireEvent.change(screen.getByTestId('create-agent-param-about'), { target: { value: 'Posts' } })
    await next()
    expect(screen.getByTestId('create-agent-tools')).toBeTruthy()
  })

  it('Tool: a tool that isn’t signed in says so (signed in is the usual case), the template’s suggestion chosen', async () => {
    await toName('writer')
    fireEvent.change(screen.getByTestId('create-agent-param-about'), { target: { value: 'Posts' } })
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-signin-p-codex')).toBeTruthy())
    expect(screen.getByTestId('create-agent-signin-p-codex').textContent).toBe('Not signed in')
    expect(screen.queryByTestId('create-agent-signin-p-claude')).toBeNull()
    expect(screen.queryByTestId('create-agent-signin-p-aider')).toBeNull()
    // The Kessel sheet's tags: the template's suggestion and the server default.
    const tags = (id: string) => Array.from(screen.getByTestId(`create-agent-tool-${id}`).closest('label')!.querySelectorAll('.k2-tag')).map((t) => t.textContent)
    expect(tags('p-codex')).toEqual(['Suggested'])
    expect(tags('p-claude')).toEqual(['Server default'])
    expect((screen.getByTestId('create-agent-tool-p-codex') as HTMLInputElement).checked).toBe(true)
    expect(screen.getByTestId('create-agent-not-signed-in').textContent).toContain(CREATE_AGENT_NOT_SIGNED_IN)
    fireEvent.click(screen.getByTestId('create-agent-tool-p-claude'))
    expect(screen.queryByTestId('create-agent-not-signed-in')).toBeNull()
  })

  it('General suggests no tool: the server default is chosen', async () => {
    await toName('general')
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-tools')).toBeTruthy())
    expect((screen.getByTestId('create-agent-tool-p-claude') as HTMLInputElement).checked).toBe(true)
  })

  it('Review: a dry run, and every row it returns is drawn (an older daemon’s suggested permissions as plain rows)', async () => {
    await toReview()
    const [route, body] = h.daemonCliPost.mock.calls[0]
    expect(route).toBe('agents/hire')
    expect(body).toEqual({
      template: 'writer',
      name: 'Max',
      handle: 'max',
      preset: 'p-codex',
      params: { about: 'Blog posts' },
      dryRun: true,
    })
    expect(body).not.toHaveProperty('launch')
    const rows = screen.getAllByTestId('create-agent-row')
    expect(rows).toHaveLength(ROWS.length)
    expect(rows[0].textContent).toContain('/home/r/agents/max')
    expect(rows[0].textContent).toContain(HIRE_STATUS_LABELS.write)
    // Plain names first, the raw path or id as a quiet second line.
    const step = (s: string) => rows.filter((r) => r.getAttribute('data-step') === s)
    expect(step('permission').map((r) => r.textContent)).toEqual([
      `Can ask you to change one of its settings, on a card in its Thread${HIRE_STATUS_LABELS.suggest}`,
      `Use compute (permission)compute.use${HIRE_STATUS_LABELS.suggest}`,
    ])
    expect(step('permission')[0].querySelector('input')).toBeNull()
    expect(step('role')[0].textContent).toBe(`Instructions (charter).k2/agent/ROLE.md${HIRE_STATUS_LABELS.write}`)
    expect(step('register')[0].textContent).toBe(`Adds it to your agents in K2${HIRE_STATUS_LABELS.write}`)
    expect(step('hello')[0].textContent).toBe(`Says hello in its Thread${HIRE_STATUS_LABELS.write}`)
    // The heartbeat says why once: the reason, not the state as well.
    expect(step('heartbeat')[0].textContent).toBe('Daily digest (heartbeat)daily-digestA heartbeat with this name exists')
    expect(screen.getByTestId('create-agent-open-questions').textContent).toContain('How would you describe your voice?')
    expect(screen.getByTestId('create-agent-charter').textContent).toContain('You write for Rosson.')
  })

  it('Review (T3): each permission is a checkbox; an unticked one is sent as declinePermissions', async () => {
    const T3_ROWS = [
      ...ROWS.filter((r) => r.step !== 'permission'),
      { step: 'permission', target: 'permission-cards', status: 'skip', reason: 'already on' },
      { step: 'permission', target: 'compute.use', status: 'grant' },
      { step: 'permission', target: 'apps.build', status: 'card', reason: 'only the server’s owner can turn it on; a person is asked on a card' },
      {
        step: 'permission',
        target: 'vault.manage',
        status: 'refused',
        reason: 'your role can’t turn it on; permission cards are off on this server',
        hint: 'An owner or admin can turn on "Let this agent manage server-wide secrets" for max: Settings → Workspaces → max → Agent.',
      },
    ]
    h.daemonCliPost.mockImplementation(async (_r: string, body: { dryRun: boolean }) => ({ ...plan(body.dryRun), rows: T3_ROWS }))
    await toReview()
    const box = (id: string) => screen.getAllByTestId('create-agent-permission-check').find((b) => b.getAttribute('data-permission') === id) as HTMLInputElement
    const rowOf = (id: string) => box(id).closest('[data-testid="create-agent-row"]') as HTMLElement
    // Would be turned on, or asked on a card: ticked, and the person may untick it.
    expect([box('compute.use').checked, box('compute.use').disabled]).toEqual([true, false])
    expect(rowOf('compute.use').textContent).toContain(HIRE_STATUS_LABELS.grant)
    expect([box('apps.build').checked, box('apps.build').disabled]).toEqual([true, false])
    expect(rowOf('apps.build').textContent).toContain('It will ask you on a card')
    // Already on: ticked and fixed. Stays off: unticked and fixed, with who can turn it on.
    expect([box('permission-cards').checked, box('permission-cards').disabled]).toEqual([true, true])
    expect(rowOf('permission-cards').textContent).toContain('Already on')
    expect([box('vault.manage').checked, box('vault.manage').disabled]).toEqual([false, true])
    expect(rowOf('vault.manage').textContent).toContain('An owner or admin can turn on')
    fireEvent.click(box('compute.use'))
    expect(box('compute.use').checked).toBe(false)
    expect(rowOf('compute.use').textContent).toContain(HIRE_STATUS_LABELS.declined)
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    const real = h.daemonCliPost.mock.calls[1][1] as { dryRun: boolean; declinePermissions?: string[] }
    expect(real.dryRun).toBe(false)
    expect(real.declinePermissions).toEqual(['compute.use'])
    // The review's own dry run never sends one.
    expect(h.daemonCliPost.mock.calls[0][1]).not.toHaveProperty('declinePermissions')
  })

  it('Create posts for real, closes, opens the Thread; an older daemon (no helloDelivered) gets the hello from the app', async () => {
    await toReview()
    expect(plan(false)).not.toHaveProperty('helloDelivered')
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    const real = h.daemonCliPost.mock.calls[1]
    expect(real[0]).toBe('agents/hire')
    expect((real[1] as { dryRun: boolean }).dryRun).toBe(false)
    expect((real[1] as { launch?: boolean }).launch).toBe(true)
    expect(real[1]).not.toHaveProperty('declinePermissions')
    expect(useAgentTemplatesStore.getState().createOpen).toBe(false)
    expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-max')
    await waitFor(() => expect(h.daemonCliPostQuery).toHaveBeenCalledTimes(2))
    expect(h.daemonCliPostQuery.mock.calls[0]).toEqual(['workspace/ensure-canonical-session', { project: '/home/r/agents/max' }])
    expect(h.daemonCliPostQuery.mock.calls[1][0]).toBe('workspace/msg')
    expect(h.daemonCliPostQuery.mock.calls[1][1]).toMatchObject({ workspace: '/home/r/agents/max', text: 'Say hello to Rosson.', wake: 'true' })
  })

  it('helloDelivered: true: the daemon said hello, so the app sends nothing', async () => {
    await toReview()
    h.daemonCliPost.mockImplementation(async (_r: string, body: { dryRun: boolean }) => ({ ...plan(body.dryRun), helloDelivered: !body.dryRun }))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-max')
    await act(async () => {})
    expect(h.daemonCliPostQuery).not.toHaveBeenCalled()
  })

  it('helloDelivered: false with a hello: the app still sends it', async () => {
    await toReview()
    h.daemonCliPost.mockImplementation(async (_r: string, body: { dryRun: boolean }) => ({ ...plan(body.dryRun), helloDelivered: false }))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    await waitFor(() => expect(h.daemonCliPostQuery).toHaveBeenCalledTimes(2))
    expect(h.daemonCliPostQuery.mock.calls[1][0]).toBe('workspace/msg')
  })

  it('closing is held while the agent is being made', async () => {
    await toReview()
    let release: (v: unknown) => void = () => {}
    h.daemonCliPost.mockImplementation(() => new Promise((r) => (release = r)))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    const close = screen.getByTestId('dialog-close') as HTMLButtonElement
    expect(close.disabled).toBe(true)
    expect(screen.getByTestId('create-agent-busy')).toBeTruthy()
    expect(screen.getByTestId('create-agent-sheet').getAttribute('aria-busy')).toBe('true')
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(useAgentTemplatesStore.getState().createOpen).toBe(true)
    expect(CREATE_AGENT_CLOSE_HELD).toBeTruthy()
    await act(async () => {
      release(plan(false))
    })
  })

  it('a refusal is said clearly, with Try again, and the sheet stays', async () => {
    await toReview()
    h.daemonCliPost.mockRejectedValue(new DaemonHttpError('/home/r/agents/max already exists', 409, 'folder_exists'))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    const err = screen.getByTestId('create-agent-error').textContent
    expect(err).toContain('That folder is already in use.')
    expect(err).toContain('The server said: /home/r/agents/max already exists.')
    expect(err).toContain('Nothing was made. Go back to change it.')
    // The line says go back, so Back is the one main button (no Try again).
    expect(screen.queryByTestId('create-agent-create')).toBeNull()
    expect(screen.getByTestId('create-agent-back').textContent).toBe('Back')
    expect(useAgentTemplatesStore.getState().createOpen).toBe(true)
    expect(h.openWorkspaceThread).not.toHaveBeenCalled()
    // A folder problem goes back to the name and folder.
    fireEvent.click(screen.getByTestId('create-agent-back'))
    expect(screen.getByTestId('create-agent-folder')).toBeTruthy()
  })

  it('a refusal the name can’t fix says Try again, and the button says the same', async () => {
    await toReview()
    h.daemonCliPost.mockRejectedValue(new DaemonHttpError('disk full', 500, 'internal'))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    expect(screen.getByTestId('create-agent-error').textContent).toContain('Nothing was made. Try again.')
    expect(screen.getByTestId('create-agent-create').textContent).toBe(CREATE_AGENT_COPY.tryAgain)
  })

  it('a required field left empty is named in its problem', async () => {
    await toName('writer')
    await next()
    expect(screen.getByTestId('create-agent-param-about-problem').textContent).toBe(paramProblem('What should it focus on?'))
    expect(paramProblem('What should it focus on?')).toContain('“What should it focus on?”')
  })

  it('a failed review says so and offers Try again', async () => {
    h.daemonCliPost.mockRejectedValueOnce(new DaemonHttpError('boom', 500, 'internal'))
    await toName('general')
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-tools')).toBeTruthy())
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-review-error')).toBeTruthy())
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-retry-review'))
    })
    await waitFor(() => expect(screen.getByTestId('create-agent-rows')).toBeTruthy())
  })

  it('a library template’s review names its author and shows its charter open', async () => {
    h.daemonCliPost.mockImplementation(async (_r: string, body: { dryRun: boolean }) => ({
      ...plan(body.dryRun),
      template: { id: 'my-writer', version: 2, hash: 'h', kind: 'agent', name: 'My writer', origin: 'library', author: 'user:rosson' },
    }))
    await toName('my-writer')
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-tools')).toBeTruthy())
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-library-note')).toBeTruthy())
    expect(screen.getByTestId('create-agent-review-agent').textContent).toContain('by rosson')
    expect(screen.getByTestId('create-agent-charter-toggle').getAttribute('aria-expanded')).toBe('true')
  })

  it('Pick: the picked card carries the selected edge; picking another moves it', async () => {
    await openSheet()
    const tile = (id: string) => screen.getByTestId(`create-agent-template-${id}`).closest('[data-radio-tile]') as HTMLElement
    expect(tile('general').hasAttribute('data-checked')).toBe(true)
    expect(tile('writer').hasAttribute('data-checked')).toBe(false)
    fireEvent.click(screen.getByTestId('create-agent-template-writer'))
    expect(tile('writer').hasAttribute('data-checked')).toBe(true)
    expect(tile('general').hasAttribute('data-checked')).toBe(false)
    // One group across both headings: every card is a radio of one name.
    const names = new Set(screen.getAllByRole('radio').map((r) => (r as HTMLInputElement).name))
    expect(names.size).toBe(1)
    // Each card's summary describes its radio.
    const r = screen.getByTestId('create-agent-template-writer')
    expect(document.getElementById(r.getAttribute('aria-describedby')!)?.textContent).toBe('writer summary')
  })

  it('Tool: none signed in says so, and Sign in asks for the chosen tool', async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: { id?: string }) => {
      if (route === 'agent-templates') return LIST
      if (route === 'agent-templates/show') return { ok: true, template: DETAILS[params?.id ?? ''] }
      if (route === 'agents/hire-root') return { ok: true, root: '/home/r/agents/', default: true }
      if (route === 'llm/accounts/list') return { tools: ACCOUNTS.tools.map((t) => ({ ...t, accounts: [] })) }
      throw new Error(`unexpected GET ${route}`)
    })
    await toName('general')
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-not-signed-in')).toBeTruthy())
    expect(screen.getByTestId('create-agent-not-signed-in').textContent).toContain(CREATE_AGENT_NO_TOOL)
    fireEvent.click(screen.getByTestId('create-agent-sign-in'))
    expect(h.requestLlmSignIn).toHaveBeenCalledWith({ tool: 'claude', toolLabel: 'Claude' })
    expect(useAgentTemplatesStore.getState().createOpen).toBe(false)
  })

  it('Review: one card of the agent, each row’s Edit going back to its step', async () => {
    await toReview()
    const card = screen.getByTestId('create-agent-review-agent')
    expect(card.textContent).toContain('Writer (version 1)')
    expect(card.textContent).toContain('Max @max')
    expect(card.textContent).toContain('/home/r/agents/max')
    expect(card.textContent).toContain('Codex')
    expect(card.textContent).toContain('not signed in yet')
    fireEvent.click(screen.getByRole('button', { name: 'Change the tool' }))
    expect(screen.getByTestId('create-agent-tools')).toBeTruthy()
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-rows')).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: 'Change the folder' }))
    expect(screen.getByTestId('create-agent-folder')).toBeTruthy()
    // The name step keeps what was typed.
    expect((screen.getByTestId('create-agent-name') as HTMLInputElement).value).toBe('Max')
    fireEvent.click(screen.getByTestId('create-agent-back'))
    expect((screen.getByTestId('create-agent-template-writer') as HTMLInputElement).checked).toBe(true)
  })

  it('Review: no Edit while the agent is being made', async () => {
    await toReview()
    h.daemonCliPost.mockImplementation(() => new Promise(() => {}))
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    expect(screen.queryByRole('button', { name: 'Change the name' })).toBeNull()
  })

  it('a built-in’s charter starts folded; its toggle opens it', async () => {
    await toReview()
    const toggle = screen.getByTestId('create-agent-charter-toggle')
    const panel = document.getElementById(toggle.getAttribute('aria-controls')!)!
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(panel.hidden).toBe(true)
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(panel.hidden).toBe(false)
    expect(panel.textContent).toContain('You write for Rosson.')
  })

  it('the footer’s hairline shows only while the body scrolls under it', async () => {
    await openSheet()
    const body = screen.getByTestId('create-agent-sheet-body')
    const foot = screen.getByTestId('create-agent-sheet-footer')
    expect(foot.className).toContain('border-transparent')
    Object.defineProperty(body, 'scrollHeight', { configurable: true, value: 900 })
    Object.defineProperty(body, 'clientHeight', { configurable: true, value: 400 })
    fireEvent.scroll(body)
    expect(foot.className).toContain('border-[var(--color-border)]')
    expect(foot.hasAttribute('data-scrolled-under')).toBe(true)
    Object.defineProperty(body, 'scrollTop', { configurable: true, value: 500 })
    fireEvent.scroll(body)
    expect(foot.className).toContain('border-transparent')
    // Scrolled: the head's hairline shows instead.
    expect(screen.getByTestId('create-agent-sheet-head').className).toContain('border-[var(--color-border)]')
  })

  it('every step and state keeps one height', async () => {
    await openSheet()
    const sheet = screen.getByTestId('create-agent-sheet')
    const h0 = sheet.className.match(/\bh-\[[^\]]+\]/)?.[0]
    expect(h0).toBeTruthy()
    fireEvent.click(screen.getByTestId('create-agent-template-writer'))
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-name')).toBeTruthy())
    expect(sheet.className.match(/\bh-\[[^\]]+\]/)?.[0]).toBe(h0)
    expect(sheet.className).not.toMatch(/\bmax-h-/)
  })

  it('an older server (404, no code) says it can’t create from templates yet', async () => {
    listBody = new DaemonHttpError('route not found', 404, null)
    render(<CreateAgentSheet />)
    await waitFor(() => expect(screen.getByTestId('create-agent-unsupported')).toBeTruthy())
    expect(screen.getByTestId('create-agent-unsupported').textContent).toBe(CREATE_UNSUPPORTED_TEXT)
  })
})
