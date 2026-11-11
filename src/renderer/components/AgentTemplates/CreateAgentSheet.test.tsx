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

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import CreateAgentSheet, {
  CREATE_AGENT_CLOSE_HELD,
  CREATE_AGENT_COPY,
  CREATE_AGENT_NOT_SIGNED_IN,
  buildHireRequest,
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

  it('Tool: presets with their sign-in state, the template’s suggestion chosen', async () => {
    await toName('writer')
    fireEvent.change(screen.getByTestId('create-agent-param-about'), { target: { value: 'Posts' } })
    await next()
    await waitFor(() => expect(screen.getByTestId('create-agent-signin-p-claude')).toBeTruthy())
    expect(screen.getByTestId('create-agent-signin-p-claude').textContent).toBe('Signed in')
    expect(screen.getByTestId('create-agent-signin-p-codex').textContent).toBe('Not signed in')
    expect(screen.queryByTestId('create-agent-signin-p-aider')).toBeNull()
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

  it('Review: a dry run, and every row it returns is drawn (permissions as plain rows)', async () => {
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
    const rows = screen.getAllByTestId('create-agent-row')
    expect(rows).toHaveLength(ROWS.length)
    expect(rows[0].textContent).toContain('/home/r/agents/max')
    expect(rows[0].textContent).toContain(HIRE_STATUS_LABELS.write)
    const perms = within(screen.getByTestId('create-agent-group-permission')).getAllByTestId('create-agent-row')
    expect(perms.map((r) => r.textContent)).toEqual([
      `Can ask for permission cards${HIRE_STATUS_LABELS.suggest}`,
      `compute.use${HIRE_STATUS_LABELS.suggest}`,
    ])
    expect(screen.getByTestId('create-agent-group-permission').querySelector('input')).toBeNull()
    expect(screen.getByTestId('create-agent-group-heartbeat').textContent).toContain('a heartbeat with this name exists')
    expect(screen.getByTestId('create-agent-open-questions').textContent).toContain('How would you describe your voice?')
    expect(screen.getByTestId('create-agent-charter').textContent).toContain('You write for Rosson.')
  })

  it('Create posts for real, closes, opens the Thread and says hello', async () => {
    await toReview()
    await act(async () => {
      fireEvent.click(screen.getByTestId('create-agent-create'))
    })
    const real = h.daemonCliPost.mock.calls[1]
    expect(real[0]).toBe('agents/hire')
    expect((real[1] as { dryRun: boolean }).dryRun).toBe(false)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(false)
    expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-max')
    await waitFor(() => expect(h.daemonCliPostQuery).toHaveBeenCalledTimes(2))
    expect(h.daemonCliPostQuery.mock.calls[0]).toEqual(['workspace/ensure-canonical-session', { project: '/home/r/agents/max' }])
    expect(h.daemonCliPostQuery.mock.calls[1][0]).toBe('workspace/msg')
    expect(h.daemonCliPostQuery.mock.calls[1][1]).toMatchObject({ workspace: '/home/r/agents/max', text: 'Say hello to Rosson.', wake: 'true' })
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
    expect(screen.getByTestId('create-agent-create').textContent).toBe(CREATE_AGENT_COPY.tryAgain)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(true)
    expect(h.openWorkspaceThread).not.toHaveBeenCalled()
    // A folder problem goes back to the name and folder.
    fireEvent.click(screen.getByTestId('create-agent-back'))
    expect(screen.getByTestId('create-agent-folder')).toBeTruthy()
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
    expect((screen.getByTestId('create-agent-charter') as HTMLDetailsElement).open).toBe(true)
  })

  it('an older server (404, no code) says it can’t create from templates yet', async () => {
    listBody = new DaemonHttpError('route not found', 404, null)
    render(<CreateAgentSheet />)
    await waitFor(() => expect(screen.getByTestId('create-agent-unsupported')).toBeTruthy())
    expect(screen.getByTestId('create-agent-unsupported').textContent).toBe(CREATE_UNSUPPORTED_TEXT)
  })
})
