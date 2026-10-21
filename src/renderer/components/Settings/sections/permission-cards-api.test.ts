// Permission cards (Beta) renderer API: the exact routes and bodies lane B
// and A1 built (b-asks.md "For D2", a1-asks-answers.md), the §17.8
// "Waiting on you" shape, and the action → Agent tab row map.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const h = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  listen: vi.fn(async (_name: string, _fn: (e: { payload: unknown }) => void): Promise<() => void> => () => {}),
  openOnAgentsPage: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.get, daemonCliPost: h.post }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'SCOPE' }))
vi.mock('@tauri-apps/api/event', () => ({ listen: h.listen, emit: vi.fn(async () => {}) }))
vi.mock('@/lib/page-selection', () => ({ openOnAgentsPage: h.openOnAgentsPage }))

import {
  PERMISSION_ACTION_ROWS,
  asksForRow,
  fetchPermissionAgents,
  fetchPermissionCardsSetting,
  fetchPermissionPending,
  isPermissionCardsSync,
  onPermissionCardsSync,
  openPermissionCard,
  parsePermissionAgents,
  parsePermissionPending,
  permissionCardsSetting,
  permissionErrorMessage,
  setAgentPermissionCards,
  setPermissionCardsSetting,
  type PermissionPending,
} from './permission-cards-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

// §17.8, verbatim field names.
const PENDING_ROW = {
  requestId: 'pr_7f3a2c',
  addr: 'billing',
  cardId: 'tc_7f3a2c90',
  seq: 41,
  action: 'compute.use',
  setting: 'Allow compute nodes',
  whatChanges: "Let billing run jobs on this server's compute nodes.",
  agent: 'billing',
  askedFor: 'dana',
  floor: 'admin',
  proof: { kind: 'click' },
  createdAt: 1761060000,
  expiresAt: 1761319200,
}

function pending(over: Partial<PermissionPending>): PermissionPending {
  return { ...parsePermissionPending({ requests: [PENDING_ROW] })[0], ...over }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('the global switch (settings/get, settings/update)', () => {
  it('reads permissionCards from settings/get; a server without the key has no permission cards', async () => {
    h.get.mockResolvedValueOnce({ permissionCards: true, defaultAgent: 'claude' })
    await expect(fetchPermissionCardsSetting()).resolves.toBe(true)
    expect(h.get).toHaveBeenCalledWith('SCOPE', 'settings/get')
    h.get.mockResolvedValueOnce({ defaultAgent: 'claude' })
    await expect(fetchPermissionCardsSetting()).resolves.toBeNull()
  })

  it('a non-boolean permissionCards is an error, not a guess', () => {
    expect(() => permissionCardsSetting({ permissionCards: 'yes' })).toThrow(/not true or false/)
    expect(permissionCardsSetting({ permissionCards: false })).toBe(false)
  })

  it('the owner write is one settings/update with only permissionCards; returns what the daemon stored', async () => {
    h.post.mockResolvedValueOnce({ permissionCards: true, defaultAgent: 'claude' })
    await expect(setPermissionCardsSetting(true)).resolves.toBe(true)
    expect(h.post).toHaveBeenCalledWith('SCOPE', 'settings/update', { permissionCards: true })
    expect(h.post).toHaveBeenCalledTimes(1)
  })

  it('a settings/update answer without the key throws', async () => {
    h.post.mockResolvedValueOnce({ defaultAgent: 'claude' })
    await expect(setPermissionCardsSetting(true)).rejects.toThrow(/did not answer with "permissionCards"/)
  })

  it('a 403 (not the owner) surfaces the daemon message', async () => {
    h.post.mockRejectedValueOnce(new DaemonHttpError('updating "permissionCards" requires the Owner role', 403, null))
    const err = await setPermissionCardsSetting(true).catch((e: unknown) => e)
    expect(permissionErrorMessage(err)).toBe('updating "permissionCards" requires the Owner role')
  })
})

describe('the per-agent switch (agent-access/set toggle=permission_cards)', () => {
  it('posts exactly lane B\'s body and returns agentsCanRequestPermissions', async () => {
    h.post.mockResolvedValueOnce({ ok: true, workspace: 'billing', toggle: 'permission_cards', agentsCanRequestPermissions: true })
    await expect(setAgentPermissionCards('p-billing', true)).resolves.toBe(true)
    expect(h.post).toHaveBeenCalledWith('SCOPE', 'agent-access/set', { toggle: 'permission_cards', workspace: 'p-billing', value: true })
  })

  it('an answer without agentsCanRequestPermissions throws', async () => {
    h.post.mockResolvedValueOnce({ ok: true })
    await expect(setAgentPermissionCards('p-billing', false)).rejects.toThrow(/agentsCanRequestPermissions/)
  })

  it('a coded refusal reads "hint (code)"', () => {
    expect(permissionErrorMessage(new DaemonHttpError('no workspace', 404, 'workspace_not_found'))).toBe('no workspace (workspace_not_found)')
    expect(permissionErrorMessage(new Error('plain'))).toBe('plain')
  })
})

describe('GET permission/agents (d2-asks.md)', () => {
  const BODY = {
    agents: [
      { workspace: 'p-billing', handle: 'billing', name: 'Billing', on: true },
      { workspace: 'p-ops', handle: 'ops', name: 'Ops', on: false },
      { workspace: 'p-bad', handle: 'bad', name: 'Bad', on: 'yes' },
      { handle: 'nows', on: true },
    ],
    recent: [
      { id: 'pr_1', agent: 'billing', workspace: 'p-billing', action: 'compute.use', setting: 'Allow compute nodes', state: 'approved', askedAt: 100, expiresAt: 200, answeredBy: 'rosson', answeredAt: 150, addr: 'billing', card: 'tc_1' },
      { id: 'pr_2', state: 'pending' },
    ],
  }

  it('parses agents and recent requests; drops malformed rows', () => {
    const out = parsePermissionAgents(BODY)
    expect(out.agents).toEqual([
      { workspace: 'p-billing', handle: 'billing', name: 'Billing', on: true },
      { workspace: 'p-ops', handle: 'ops', name: 'Ops', on: false },
    ])
    expect(out.recent).toEqual([
      { id: 'pr_1', agent: 'billing', workspace: 'p-billing', action: 'compute.use', setting: 'Allow compute nodes', state: 'approved', askedAt: 100, answeredBy: 'rosson', answeredAt: 150, addr: 'billing', card: 'tc_1' },
    ])
  })

  it('a body without the lists throws (loud, not empty)', () => {
    expect(() => parsePermissionAgents({ agents: [] })).toThrow(/"recent"/)
    expect(() => parsePermissionAgents({ ok: false })).toThrow(/"agents"/)
  })

  it('GETs permission/agents, with workspace= for one agent', async () => {
    h.get.mockResolvedValue(BODY)
    await fetchPermissionAgents()
    expect(h.get).toHaveBeenLastCalledWith('SCOPE', 'permission/agents', undefined)
    await fetchPermissionAgents('p-billing')
    expect(h.get).toHaveBeenLastCalledWith('SCOPE', 'permission/agents', { workspace: 'p-billing' })
  })
})

describe('GET permission/pending (§17.8)', () => {
  it('parses the §17.8 row', async () => {
    h.get.mockResolvedValueOnce({ requests: [PENDING_ROW, { requestId: 'pr_x' }] })
    const rows = await fetchPermissionPending()
    expect(h.get).toHaveBeenCalledWith('SCOPE', 'permission/pending')
    expect(rows).toEqual([
      {
        requestId: 'pr_7f3a2c',
        addr: 'billing',
        cardId: 'tc_7f3a2c90',
        action: 'compute.use',
        setting: 'Allow compute nodes',
        whatChanges: "Let billing run jobs on this server's compute nodes.",
        agent: 'billing',
        askedFor: 'dana',
        floor: 'admin',
        proofKind: 'click',
        createdAt: 1761060000,
        expiresAt: 1761319200,
      },
    ])
  })

  it('a body without requests throws', () => {
    expect(() => parsePermissionPending({})).toThrow(/"requests"/)
  })
})

describe('live updates (sync:settings)', () => {
  it('only permission frames count: the global flip and one agent\'s switch', () => {
    expect(isPermissionCardsSync({ permissionCards: true, feature: 'permission-cards-v1', rehandshake: true })).toBe(true)
    expect(isPermissionCardsSync({ permissionCards: 'agent', workspace: 'p-billing' })).toBe(true)
    expect(isPermissionCardsSync({ computePreview: true })).toBe(false)
    expect(isPermissionCardsSync(null)).toBe(false)
  })

  it('onPermissionCardsSync calls back on permission frames only, and unlistens', async () => {
    const unlisten = vi.fn()
    let handler: ((e: { payload: unknown }) => void) | null = null
    h.listen.mockImplementation(async (name: string, fn: (e: { payload: unknown }) => void) => {
      expect(name).toBe('sync:settings')
      handler = fn
      return unlisten
    })
    const cb = vi.fn()
    const off = onPermissionCardsSync(cb)
    await vi.waitFor(() => expect(handler).not.toBeNull())
    handler!({ payload: { computePreview: true } })
    expect(cb).not.toHaveBeenCalled()
    handler!({ payload: { permissionCards: 'agent', workspace: 'p-ops' } })
    expect(cb).toHaveBeenCalledTimes(1)
    off()
    expect(unlisten).toHaveBeenCalledTimes(1)
  })
})

describe('the "Asked for this" chip map', () => {
  it('every mapped row is a real Agent tab row id', () => {
    const projects = readFileSync(join(__dirname, 'ProjectsSection.tsx'), 'utf8')
    const apps = readFileSync(join(__dirname, 'AgentAppsAccess.tsx'), 'utf8')
    for (const row of new Set(Object.values(PERMISSION_ACTION_ROWS))) {
      const inProjects = projects.includes(`data-settings-id="${row}"`)
      const inApps = row === 'projects.agents-can-manage-skin' && apps.includes('projects.agents-can-manage-skin')
      expect(inProjects || inApps, row).toBe(true)
      // …and that row carries the chip.
      expect(projects.includes(`<AskedChip row="${row}" />`), `chip on ${row}`).toBe(true)
    }
  })

  it('maps the §5.2 switch actions; held requests have no row', () => {
    expect(PERMISSION_ACTION_ROWS['compute.use']).toBe('projects.agents-can-use-compute')
    expect(PERMISSION_ACTION_ROWS['db.agent-access']).toBe('projects.db-agent-create')
    expect(PERMISSION_ACTION_ROWS['apps.approve']).toBeUndefined()
    expect(PERMISSION_ACTION_ROWS['mail.approve']).toBeUndefined()
  })

  it('asksForRow matches this agent and this row only', () => {
    const rows = [
      pending({}),
      pending({ requestId: 'pr_2', agent: 'ops' }),
      pending({ requestId: 'pr_3', action: 'mail.manage' }),
      pending({ requestId: 'pr_4', action: 'apps.approve' }),
    ]
    expect(asksForRow(rows, 'billing', 'projects.agents-can-use-compute').map((r) => r.requestId)).toEqual(['pr_7f3a2c'])
    expect(asksForRow(rows, 'billing', 'projects.mail-manage-enabled').map((r) => r.requestId)).toEqual(['pr_3'])
    expect(asksForRow(rows, 'nobody', 'projects.agents-can-use-compute')).toEqual([])
  })
})

describe('openPermissionCard', () => {
  it('opens the asking agent on the Agents page and closes Settings', () => {
    useProjectsStore.setState({ projects: [{ id: 'p-billing', handle: 'billing', name: 'Billing', path: '/w/billing', workspaces: [] }] } as never)
    useSettingsStore.setState({ settingsOpen: true })
    expect(openPermissionCard('billing')).toBe(true)
    expect(h.openOnAgentsPage).toHaveBeenCalledWith('p-billing')
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
  })

  it('an unknown handle opens nothing', () => {
    useProjectsStore.setState({ projects: [] } as never)
    useSettingsStore.setState({ settingsOpen: true })
    expect(openPermissionCard('ghost')).toBe(false)
    expect(h.openOnAgentsPage).not.toHaveBeenCalled()
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
  })
})
