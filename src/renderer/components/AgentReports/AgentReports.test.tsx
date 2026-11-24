// @vitest-environment jsdom
//
// The agent-reports card and Settings → General → Send feedback
// (plan-agent-reports-opt-in-app-v1): the copy with the brand's names, the
// parts in the daemon's words, one POST of all three switches per answer,
// Skip on the row, Ask me later on the offer, the offer drawn only while
// the daemon offers it, and the Settings section for the owner or an
// admin, a Member, nobody answered yet and an older daemon. The daemon is
// mocked at daemon-cli; an unexpected route throws.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
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

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { primaryScope } from '@/kessel/server-scope'
import { CLI_NAME, COMPANY_NAME, PRODUCT_NAME } from '@/lib/brand'
import { agentReportsBody } from '@/lib/product-feedback'
import { resetFirstRunForTests } from '@/stores/first-run'
import { useAgentReportsCardStore } from '@/stores/agent-reports-card'
import { SendFeedbackRows, answeredLine } from '@/components/Settings/sections/SendFeedbackRows'
import { AgentReportsCard, agentReportsCopy } from './AgentReportsCard'
import { AgentReportsHost } from './AgentReportsHost'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const PARTS = [
  { id: 'computer', label: 'Computer size and free space' },
  { id: 'k2-state', label: 'The names of settings that are on, and counts' },
  { id: 'log-tail', label: 'The end of the log' },
]

type Doc = Record<string, unknown>

function settings(over: Doc = {}): Doc {
  return {
    enabled: true,
    agents: false,
    agentPlatform: false,
    agentAttach: [],
    agentDiagnostics: false,
    agentScopes: [],
    agentsAsked: false,
    agentsAskedAt: null,
    agentsAskedBy: null,
    attachParts: PARTS,
    rememberDiagnostics: false,
    canEdit: true,
    canSend: true,
    endpoint: 'https://feedback.example',
    pending: 0,
    service: { state: 'ok' },
    ...over,
  }
}

function status(over: Doc = {}): Doc {
  return {
    enabled: false,
    hidden: false,
    finished: false,
    rows: [
      { id: 'llm_sign_in', state: 'done', opens: 'llm_sign_in' },
      { id: 'agent_reports', state: 'todo', opens: 'agent_reports' },
      { id: 'first_agent', state: 'todo', opens: 'add_workspace' },
    ],
    kesselFirstRun: false,
    kesselSeat: { exists: false },
    ...over,
  }
}

let served: Doc = settings()
let first: Doc = status()
const posts: Array<[string, unknown]> = []
const gets: string[] = []

beforeEach(() => {
  resetFirstRunForTests()
  useAgentReportsCardStore.setState({ openAs: null })
  vi.spyOn(primaryScope(), 'serverSupports').mockReturnValue(true)
  served = settings()
  first = status()
  posts.length = 0
  gets.length = 0
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    gets.push(route)
    if (route === 'product-feedback/settings') return served
    if (route === 'first-run/status') return first
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: unknown) => {
    posts.push([route, body])
    if (route === 'product-feedback/settings') {
      const b = body as { agents: boolean; agentPlatform: boolean; agentAttach: string[] | 'none' }
      served = settings({
        agents: b.agents,
        agentPlatform: b.agentPlatform,
        agentAttach: b.agentAttach === 'none' ? [] : b.agentAttach,
        agentsAsked: true,
        agentsAskedAt: Date.UTC(2026, 10, 23),
        agentsAskedBy: 'owner',
      })
      return served
    }
    if (route === 'first-run/skip' || route === 'first-run/offer/dismiss') {
      first = status({ rows: [{ id: 'agent_reports', state: 'skipped', opens: 'agent_reports' }] })
      return first
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the agent-reports card', () => {
  it('says what it is for with the brand names, never a hard-coded one', () => {
    const copy = agentReportsCopy()
    expect(copy.heading).toBe(`May your agents report ${PRODUCT_NAME} problems?`)
    expect(copy.body).toContain(`a short report to ${COMPANY_NAME}`)
    expect(copy.answers.map((a) => a.label)).toEqual([
      `Yes, problems with the ${CLI_NAME} commands agents run.`,
      `Yes, anything about ${PRODUCT_NAME}.`,
      'No.',
    ])
    expect(copy.always).toBe(`Always included: ${PRODUCT_NAME} version and install kind`)
    expect(JSON.stringify(copy)).not.toMatch(/\bCLI\b/)
  })

  it('posts all three switches in one write: a yes with the parts ticked, in the daemon order', async () => {
    const onClose = vi.fn()
    render(<AgentReportsCard scope={primaryScope()} kind="row" onClose={onClose} />)
    const card = await screen.findByTestId('agent-reports-card-row')
    const save = within(card).getByTestId('agent-reports-save')
    await waitFor(() => expect(gets).toContain('product-feedback/settings'))
    expect(save).toHaveProperty('disabled', true)
    expect(within(card).queryByTestId('agent-reports-attach')).toBeNull()

    fireEvent.click(within(card).getByTestId('agent-reports-answer-cli'))
    const attach = within(card).getByTestId('agent-reports-attach')
    // The daemon's labels, every part off, the versions always.
    for (const p of PARTS) {
      expect(within(attach).getByText(p.label)).toBeTruthy()
      expect(within(attach).getByTestId(`agent-reports-part-${p.id}`)).toHaveProperty('checked', false)
    }
    expect(within(attach).getByTestId('agent-reports-part-always')).toHaveProperty('checked', true)
    fireEvent.click(within(attach).getByTestId('agent-reports-part-log-tail'))
    fireEvent.click(within(attach).getByTestId('agent-reports-part-computer'))
    fireEvent.click(save)

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    expect(posts).toEqual([
      ['product-feedback/settings', { agents: true, agentPlatform: false, agentAttach: ['computer', 'log-tail'] }],
    ])
    await waitFor(() => expect(gets).toContain('first-run/status'))
  })

  it('sends No as agents, agentPlatform and agentAttach off together, without the parts list', async () => {
    const onClose = vi.fn()
    render(<AgentReportsCard scope={primaryScope()} kind="offer" onClose={onClose} />)
    const card = await screen.findByTestId('agent-reports-card-offer')
    await waitFor(() => expect(gets).toContain('product-feedback/settings'))
    fireEvent.click(within(card).getByTestId('agent-reports-answer-all'))
    fireEvent.click(within(card).getByTestId('agent-reports-part-computer'))
    fireEvent.click(within(card).getByTestId('agent-reports-answer-off'))
    expect(within(card).queryByTestId('agent-reports-attach')).toBeNull()
    fireEvent.click(within(card).getByTestId('agent-reports-save'))
    await waitFor(() => expect(onClose).toHaveBeenCalled())
    expect(posts).toEqual([['product-feedback/settings', { agents: false, agentPlatform: false, agentAttach: 'none' }]])
    expect(agentReportsBody('all', [])).toEqual({ agents: true, agentPlatform: true, agentAttach: 'none' })
  })

  it('Skip on the row skips the row; Ask me later on the offer dismisses it for this person', async () => {
    const onClose = vi.fn()
    const { unmount } = render(<AgentReportsCard scope={primaryScope()} kind="row" onClose={onClose} />)
    const row = await screen.findByTestId('agent-reports-card-row')
    const skip = within(row).getByTestId('agent-reports-later')
    expect(skip.textContent).toBe('Skip')
    fireEvent.click(skip)
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    expect(posts).toEqual([['first-run/skip', { row: 'agent_reports' }]])
    unmount()

    posts.length = 0
    render(<AgentReportsCard scope={primaryScope()} kind="offer" onClose={onClose} />)
    const offer = await screen.findByTestId('agent-reports-card-offer')
    const later = within(offer).getByTestId('agent-reports-later')
    expect(later.textContent).toBe('Ask me later')
    fireEvent.click(later)
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(2))
    expect(posts).toEqual([['first-run/offer/dismiss', { offer: 'agent_reports' }]])
  })

  it('shows the daemon’s own refusal and stays open', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw Object.assign(new Error('403'), {
        body: { ok: false, error: 'admin_only', message: 'Only the owner or an admin can change this server’s feedback settings.' },
      })
    })
    const onClose = vi.fn()
    render(<AgentReportsCard scope={primaryScope()} kind="row" onClose={onClose} />)
    const card = await screen.findByTestId('agent-reports-card-row')
    await waitFor(() => expect(gets).toContain('product-feedback/settings'))
    fireEvent.click(within(card).getByTestId('agent-reports-answer-off'))
    fireEvent.click(within(card).getByTestId('agent-reports-save'))
    expect((await within(card).findByTestId('agent-reports-error')).textContent).toContain('Only the owner or an admin')
    expect(onClose).not.toHaveBeenCalled()
  })
})

describe('the host', () => {
  it('draws the offer only while the daemon offers it, and the row card when opened', async () => {
    render(<AgentReportsHost />)
    await waitFor(() => expect(gets).toContain('first-run/status'))
    expect(screen.queryByTestId('agent-reports-card-offer')).toBeNull()
    useAgentReportsCardStore.getState().open('row')
    expect(await screen.findByTestId('agent-reports-card-row')).toBeTruthy()
    cleanup()

    resetFirstRunForTests()
    useAgentReportsCardStore.setState({ openAs: null })
    first = status({ offers: ['agent_reports'] })
    render(<AgentReportsHost />)
    const offer = await screen.findByTestId('agent-reports-card-offer')
    fireEvent.click(within(offer).getByTestId('agent-reports-later'))
    await waitFor(() => expect(screen.queryByTestId('agent-reports-card-offer')).toBeNull())
    expect(posts).toEqual([['first-run/offer/dismiss', { offer: 'agent_reports' }]])
  })
})

describe('Settings → General → Send feedback', () => {
  it('says nobody has answered, with Choose now, for the owner or an admin', async () => {
    render(<SendFeedbackRows />)
    const group = await screen.findByTestId('send-feedback-agent-reports')
    expect(within(group).getByTestId('agent-reports-not-answered').textContent).toBe(agentReportsCopy().notAnswered)
    fireEvent.click(within(group).getByTestId('agent-reports-choose-now'))
    expect(useAgentReportsCardStore.getState().openAs).toBe('settings')
    expect(screen.queryByTestId('send-feedback-setting-agents')).toBeNull()
  })

  it('shows the answer, who and when, and saves each change as one write of all three', async () => {
    served = settings({
      agents: true,
      agentPlatform: false,
      agentAttach: ['k2-state'],
      agentsAsked: true,
      agentsAskedAt: Date.UTC(2026, 10, 20),
      agentsAskedBy: 'adam',
    })
    render(<SendFeedbackRows />)
    const group = await screen.findByTestId('send-feedback-agent-reports')
    expect(within(group).getByTestId('agent-reports-answered').textContent).toContain('Answered by adam on ')
    expect(within(group).getByTestId('agent-reports-answer-cli')).toHaveProperty('checked', true)
    expect(within(group).getByTestId('agent-reports-part-k2-state')).toHaveProperty('checked', true)
    fireEvent.click(within(group).getByTestId('agent-reports-answer-all'))
    await waitFor(() => expect(posts.length).toBe(1))
    expect(posts[0]).toEqual(['product-feedback/settings', { agents: true, agentPlatform: true, agentAttach: ['k2-state'] }])
    expect(answeredLine({ askedAt: null, askedBy: 'owner' })).toBe('Answered by the owner.')
  })

  it('is read-only for a Member, who is told the owner or an admin chooses', async () => {
    served = settings({ canEdit: false, agents: true, agentsAsked: true, agentsAskedBy: 'owner', agentsAskedAt: 1 })
    render(<SendFeedbackRows />)
    const group = await screen.findByTestId('send-feedback-agent-reports')
    expect(group.textContent).toContain(agentReportsCopy().members)
    for (const id of ['cli', 'all', 'off']) {
      expect(within(group).getByTestId(`agent-reports-answer-${id}`)).toHaveProperty('disabled', true)
    }
    cleanup()
    served = settings({ canEdit: false })
    render(<SendFeedbackRows />)
    const unanswered = await screen.findByTestId('send-feedback-agent-reports')
    expect(unanswered.textContent).toContain(agentReportsCopy().members)
    expect(within(unanswered).queryByTestId('agent-reports-choose-now')).toBeNull()
    expect(posts).toEqual([])
  })

  it('keeps the old switches for an older daemon', async () => {
    const old = settings()
    for (const k of ['agentPlatform', 'agentAttach', 'agentScopes', 'agentsAsked', 'agentsAskedAt', 'agentsAskedBy', 'attachParts']) {
      delete old[k]
    }
    served = old
    render(<SendFeedbackRows />)
    expect(await screen.findByTestId('send-feedback-setting-agents')).toBeTruthy()
    expect(screen.getByTestId('send-feedback-setting-agent-diagnostics')).toBeTruthy()
    expect(screen.queryByTestId('send-feedback-agent-reports')).toBeNull()
  })
})
