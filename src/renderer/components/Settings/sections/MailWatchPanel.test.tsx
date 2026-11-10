// @vitest-environment jsdom
// Nothing wakes an agent until someone chooses, and both places a person can
// choose (the agent's Mail tab, an inbox's own panel) go to the same record.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const fetchMailWatch = vi.fn()
const setMailWatch = vi.fn()
const setWatchInbox = vi.fn()
const addWatchVip = vi.fn()
const removeWatchVip = vi.fn()
const pauseMailWatch = vi.fn()
const fetchInboxWatchers = vi.fn()

vi.mock('./mail-watch-api', async (orig) => ({
  ...(await orig<typeof import('./mail-watch-api')>()),
  fetchMailWatch: (...a: unknown[]) => fetchMailWatch(...a),
  setMailWatch: (...a: unknown[]) => setMailWatch(...a),
  setWatchInbox: (...a: unknown[]) => setWatchInbox(...a),
  addWatchVip: (...a: unknown[]) => addWatchVip(...a),
  removeWatchVip: (...a: unknown[]) => removeWatchVip(...a),
  pauseMailWatch: (...a: unknown[]) => pauseMailWatch(...a),
  fetchInboxWatchers: (...a: unknown[]) => fetchInboxWatchers(...a),
}))
vi.mock('./email-api', async (orig) => ({
  ...(await orig<typeof import('./email-api')>()),
  onWindowServerMailChanged: () => () => undefined,
}))

import { MailWatchPanel } from './MailWatchPanel'
import { InboxWakePanel } from './InboxWakePanel'
import type { Inbox } from './email-api'
import type { MailWatchStatus } from './mail-watch-api'

const base: MailWatchStatus = {
  enabled: false,
  vipOutsideWindow: true,
  snippets: true,
  vipSessionLine: true,
  pausedUntil: null,
  inboxes: [
    { address: 'sales@acme.test', source: 'hosted', wake: null, primary: true, personMailbox: false, lastPollAt: null, lastError: null, needsRelink: false },
    { address: 'it@acme.test', source: 'linked', wake: null, primary: false, personMailbox: false, lastPollAt: null, lastError: null, needsRelink: false },
  ],
  vips: [],
  digest: null,
  last24h: { vipDelivered: 0, digested: 0, failedAuth: 0 },
}

afterEach(() => cleanup())

beforeEach(() => {
  vi.clearAllMocks()
  fetchMailWatch.mockResolvedValue(base)
  setMailWatch.mockResolvedValue(base)
  setWatchInbox.mockResolvedValue(base)
  addWatchVip.mockResolvedValue(base)
  fetchInboxWatchers.mockResolvedValue({ agents: [] })
})

describe('MailWatchPanel (an agent\'s Mail tab)', () => {
  it('starts with no inbox waking the agent, even its own', async () => {
    render(<MailWatchPanel projectPath="/w/sales" />)
    await screen.findByText('sales@acme.test')
    expect(screen.getByRole('switch', { name: 'Tell this agent about mail' }).getAttribute('aria-checked')).toBe('false')
    expect(screen.getByRole('button', { name: 'How sales@acme.test wakes this agent' }).textContent).toContain('Off')
    expect(screen.getByRole('button', { name: 'How it@acme.test wakes this agent' }).textContent).toContain('Off')
  })

  it('turning it on says nothing about any inbox', async () => {
    render(<MailWatchPanel projectPath="/w/sales" />)
    fireEvent.click(await screen.findByRole('switch', { name: 'Tell this agent about mail' }))
    await waitFor(() => expect(setMailWatch).toHaveBeenCalledWith('/w/sales', { on: true }))
    expect(setWatchInbox).not.toHaveBeenCalled()
  })

  it('adds a VIP through the daemon', async () => {
    render(<MailWatchPanel projectPath="/w/sales" />)
    const input = await screen.findByLabelText('Add a VIP sender')
    fireEvent.change(input, { target: { value: 'jane@acme.test' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    await waitFor(() => expect(addWatchVip).toHaveBeenCalledWith('/w/sales', 'jane@acme.test'))
  })

  it('names a stopped inbox in plain words', async () => {
    fetchMailWatch.mockResolvedValue({
      ...base,
      inboxes: [{ ...base.inboxes[0], wake: 'all', needsRelink: true }],
    })
    render(<MailWatchPanel projectPath="/w/sales" />)
    await screen.findByText(/Its login was refused/)
  })
})

function inbox(): Inbox {
  const p = (projectId: string, workspace: string) => ({ projectId, workspace, level: 'read' as const, canManage: false, canDelete: false })
  return {
    address: 'sales@acme.test',
    source: 'hosted',
    displayName: null,
    status: 'running',
    primary: p('p1', 'sales'),
    grants: [p('p2', 'support')],
    yourLevel: 'send',
    maxLevel: 'send',
  } as unknown as Inbox
}

describe('InboxWakePanel (the inbox\'s own panel)', () => {
  it('shows every agent that can read the inbox as Off until someone chooses', async () => {
    render(<InboxWakePanel inbox={inbox()} canMutate />)
    expect((await screen.findByRole('button', { name: 'How this inbox wakes sales' })).textContent).toContain('Off')
    expect(screen.getByRole('button', { name: 'How this inbox wakes support' }).textContent).toContain('Off')
  })

  it('reads the same record as the Mail tab', async () => {
    fetchInboxWatchers.mockResolvedValue({ agents: [{ workspace: 'support', projectId: 'p2', wake: 'vip', watching: true }] })
    render(<InboxWakePanel inbox={inbox()} canMutate />)
    await waitFor(() => expect(screen.getByRole('button', { name: 'How this inbox wakes support' }).textContent).toContain('VIP senders only'))
    expect(screen.getByRole('button', { name: 'How this inbox wakes sales' }).textContent).toContain('Off')
  })

  it('is read-only without the right to change it', async () => {
    render(<InboxWakePanel inbox={inbox()} canMutate={false} />)
    const dd = await screen.findByRole('button', { name: 'How this inbox wakes sales' })
    await waitFor(() => expect(dd.hasAttribute('disabled')).toBe(true))
  })
})
