// @vitest-environment jsdom

// Settings → Access → Permission Cards (Beta) and the Agent tab's "Can ask
// for permission cards" row + "Asked for this" chip (PRD §2.1, §2.2, §8).
// The daemon is a route table here: every switch is one exact write, every
// fact is read back from the daemon, and nothing in the `/cli/permission/`
// family is called while the server switch reads off.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, act } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const h = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  role: vi.fn(),
  openOnAgentsPage: vi.fn(),
  syncHandlers: [] as Array<(e: { payload: unknown }) => void>,
}))

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.get, daemonCliPost: h.post }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: h.role }))
vi.mock('@/lib/page-selection', () => ({ openOnAgentsPage: h.openOnAgentsPage }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async (name: string, fn: (e: { payload: unknown }) => void) => {
    if (name === 'sync:settings') h.syncHandlers.push(fn)
    return () => {
      h.syncHandlers = h.syncHandlers.filter((x) => x !== fn)
    }
  }),
}))

import {
  AgentPermissionCardsRow,
  AskedChip,
  PERMISSION_CARDS_MANIFEST,
  PermissionAsksProvider,
  PermissionCardsSection,
  agentPermissionRowVisibility,
  globalSwitchCanFlip,
  requestStateLabel,
} from './PermissionCardsSection'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { settingsNav } from '../Settings'

type Daemon = {
  permissionCards: boolean | undefined
  agents: Array<{ workspace: string; handle: string; name: string; on: boolean }>
  recent: unknown[]
  pending: unknown[]
}
let daemon: Daemon

const PENDING = {
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

function familyCalls(): string[] {
  return [...h.get.mock.calls, ...h.post.mock.calls].map((c) => c[1] as string).filter((r) => r.startsWith('permission/'))
}

function emitSync(payload: unknown): void {
  act(() => {
    for (const fn of h.syncHandlers) fn({ payload })
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  h.syncHandlers = []
  useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
  useProjectsStore.setState({
    projects: [
      { id: 'p-billing', handle: 'billing', name: 'Billing', path: '/w/billing', workspaces: [] },
      { id: 'p-ops', handle: 'ops', name: 'Ops', path: '/w/ops', workspaces: [] },
    ],
  } as never)
  daemon = {
    permissionCards: false,
    agents: [
      { workspace: 'p-billing', handle: 'billing', name: 'Billing', on: false },
      { workspace: 'p-ops', handle: 'ops', name: 'Ops', on: false },
    ],
    recent: [],
    pending: [],
  }
  h.role.mockResolvedValue('owner')
  h.get.mockImplementation(async (_scope: unknown, route: string, params?: { workspace?: string }) => {
    if (route === 'settings/get') {
      return daemon.permissionCards === undefined ? { defaultAgent: 'claude' } : { permissionCards: daemon.permissionCards }
    }
    if (route === 'permission/agents') {
      const agents = params?.workspace ? daemon.agents.filter((a) => a.workspace === params.workspace) : daemon.agents
      return { agents, recent: daemon.recent }
    }
    if (route === 'permission/pending') return { requests: daemon.pending }
    throw new Error(`unexpected GET ${route}`)
  })
  h.post.mockImplementation(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    if (route === 'settings/update') {
      daemon.permissionCards = body.permissionCards as boolean
      return { permissionCards: daemon.permissionCards }
    }
    if (route === 'agent-access/set') {
      const a = daemon.agents.find((x) => x.workspace === body.workspace)
      if (!a) throw new DaemonHttpError('no workspace', 404, 'workspace_not_found')
      a.on = body.value as boolean
      return { ok: true, workspace: a.handle, toggle: 'permission_cards', agentsCanRequestPermissions: a.on }
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

describe('visibility rules', () => {
  it('only the owner flips the global switch', () => {
    expect(globalSwitchCanFlip('owner')).toBe(true)
    expect(globalSwitchCanFlip('admin')).toBe(false)
    expect(globalSwitchCanFlip('member')).toBe(false)
    expect(globalSwitchCanFlip(null)).toBe(false)
  })

  it('the Agent tab row shows only while the server switch is on, to the owner or an admin', () => {
    expect(agentPermissionRowVisibility('owner', true)).toEqual({ show: true, canFlip: true })
    expect(agentPermissionRowVisibility('admin', true)).toEqual({ show: true, canFlip: true })
    expect(agentPermissionRowVisibility(null, true)).toEqual({ show: true, canFlip: false })
    expect(agentPermissionRowVisibility('member', true)).toEqual({ show: false, canFlip: false })
    expect(agentPermissionRowVisibility('owner', false)).toEqual({ show: false, canFlip: false })
    expect(agentPermissionRowVisibility('owner', null)).toEqual({ show: false, canFlip: false })
  })

  it('state labels follow §17.2.4', () => {
    expect(requestStateLabel('already_done')).toBe('Already on')
    expect(requestStateLabel('approved')).toBe('Allowed')
    expect(requestStateLabel('weird')).toBe('weird')
  })
})

describe('Settings → Access → Permission Cards', () => {
  it('sits in Access with a Beta tag, and has a search entry', () => {
    const access = settingsNav({ zenGardens: false }).find((b) => b.kind === 'group' && b.title === 'Access')
    if (!access || access.kind !== 'group') throw new Error('no Access group')
    const item = access.items.find((i) => i.id === 'permission-cards')
    expect(item).toEqual({ id: 'permission-cards', label: 'Permission Cards', beta: true })
    expect(PERMISSION_CARDS_MANIFEST.every((e) => e.section === 'permission-cards')).toBe(true)
    const settings = readFileSync(join(__dirname, '..', 'Settings.tsx'), 'utf8')
    expect(settings).toContain("activeSection === 'permission-cards'")
    expect(settings).toContain('...PERMISSION_CARDS_MANIFEST,')
  })

  it('off (the default): the owner sees the switch off and nothing from the permission family is called', async () => {
    render(<PermissionCardsSection />)
    const sw = await screen.findByRole('switch', { name: 'Permission cards' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getAllByText('BETA').length).toBeGreaterThan(0)
    expect(screen.queryByText('Turn on for all agents')).toBeNull()
    expect(familyCalls()).toEqual([])
  })

  it('the owner turns it on: one settings/update {permissionCards:true}, then the agents and Waiting on you load', async () => {
    daemon.pending = [PENDING]
    render(<PermissionCardsSection />)
    const sw = await screen.findByRole('switch', { name: 'Permission cards' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await waitFor(() => expect(h.post).toHaveBeenCalledWith(expect.anything(), 'settings/update', { permissionCards: true }))
    expect(h.post).toHaveBeenCalledTimes(1)
    await screen.findByTestId('permission-agent-billing')
    expect(screen.getByRole('switch', { name: 'Billing can ask for permission cards' }).getAttribute('aria-checked')).toBe('false')
    await screen.findByTestId('permission-pending-pr_7f3a2c')
    expect(screen.getByText('billing asks: Allow compute nodes')).toBeTruthy()
  })

  it('a refused global write rolls back and shows the daemon message', async () => {
    h.post.mockRejectedValueOnce(new DaemonHttpError('updating "permissionCards" requires the Owner role', 403, null))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<PermissionCardsSection />)
    const sw = await screen.findByRole('switch', { name: 'Permission cards' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await screen.findByText('updating "permissionCards" requires the Owner role')
    expect(screen.getByRole('switch', { name: 'Permission cards' }).getAttribute('aria-checked')).toBe('false')
    expect(familyCalls()).toEqual([])
    spy.mockRestore()
  })

  it('an admin sees the global switch read-only, and manages agents while it is on', async () => {
    h.role.mockResolvedValue('admin')
    daemon.permissionCards = true
    render(<PermissionCardsSection />)
    await screen.findByTestId('permission-cards-owner-only')
    const sw = screen.getByRole('switch', { name: 'Permission cards' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(h.post).not.toHaveBeenCalledWith(expect.anything(), 'settings/update', expect.anything())
    const ops = await screen.findByRole('switch', { name: 'Ops can ask for permission cards' })
    fireEvent.click(ops)
    await waitFor(() =>
      expect(h.post).toHaveBeenCalledWith(expect.anything(), 'agent-access/set', { toggle: 'permission_cards', workspace: 'p-ops', value: true }),
    )
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Ops can ask for permission cards' }).getAttribute('aria-checked')).toBe('true'))
  })

  it('a member sees the state read-only and one line; nothing from the family is called', async () => {
    h.role.mockResolvedValue('member')
    daemon.permissionCards = true
    render(<PermissionCardsSection />)
    await screen.findByTestId('permission-cards-member')
    expect((screen.getByRole('switch', { name: 'Permission cards' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.queryByText('Turn on for all agents')).toBeNull()
    expect(familyCalls()).toEqual([])
  })

  it('a server without permission cards says to update it; the switch is disabled', async () => {
    daemon.permissionCards = undefined
    render(<PermissionCardsSection />)
    await screen.findByTestId('permission-cards-old-server')
    expect((screen.getByRole('switch', { name: 'Permission cards' }) as HTMLButtonElement).disabled).toBe(true)
    expect(familyCalls()).toEqual([])
  })

  it('Turn on for all agents: one write per agent that is off', async () => {
    daemon.permissionCards = true
    daemon.agents[0].on = true
    daemon.agents.push({ workspace: 'p-sales', handle: 'sales', name: 'Sales', on: false })
    render(<PermissionCardsSection />)
    const all = await screen.findByRole('button', { name: 'Turn on for all agents' })
    await waitFor(() => expect((all as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(all)
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Sales can ask for permission cards' }).getAttribute('aria-checked')).toBe('true'))
    const writes = h.post.mock.calls.filter((c) => c[1] === 'agent-access/set').map((c) => c[2])
    expect(writes).toEqual([
      { toggle: 'permission_cards', workspace: 'p-ops', value: true },
      { toggle: 'permission_cards', workspace: 'p-sales', value: true },
    ])
    expect(screen.getByRole('switch', { name: 'Ops can ask for permission cards' }).getAttribute('aria-checked')).toBe('true')
    await waitFor(() => expect((screen.getByRole('button', { name: 'Turn on for all agents' }) as HTMLButtonElement).disabled).toBe(true))
  })

  it('a refused agent write rolls back that agent and shows the hint', async () => {
    daemon.permissionCards = true
    h.post.mockImplementationOnce(async () => {
      throw new DaemonHttpError('only the owner or an admin login may change agent access', 403, 'forbidden')
    })
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<PermissionCardsSection />)
    fireEvent.click(await screen.findByRole('switch', { name: 'Billing can ask for permission cards' }))
    await screen.findByText('only the owner or an admin login may change agent access (forbidden)')
    expect(screen.getByRole('switch', { name: 'Billing can ask for permission cards' }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('the agents route missing is a loud error line, not an empty list', async () => {
    daemon.permissionCards = true
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    h.get.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'settings/get') return { permissionCards: true }
      if (route === 'permission/pending') return { requests: [] }
      throw new DaemonHttpError('daemon /cli/permission/agents 404', 404, null)
    })
    render(<PermissionCardsSection />)
    await screen.findByTestId('permission-cards-agents-error')
    expect(screen.getByTestId('permission-cards-agents-error').textContent).toContain('404')
    spy.mockRestore()
  })

  it('recent requests show their state; Open card on a waiting one opens the agent', async () => {
    daemon.permissionCards = true
    daemon.recent = [
      { id: 'pr_1', agent: 'billing', workspace: 'p-billing', action: 'compute.use', setting: 'Allow compute nodes', state: 'approved', askedAt: 100, answeredBy: 'rosson', answeredAt: 150, addr: 'billing', card: 'tc_1' },
    ]
    daemon.pending = [PENDING]
    render(<PermissionCardsSection />)
    await screen.findByTestId('permission-recent-pr_1')
    expect(screen.getByTestId('permission-recent-pr_1').textContent).toContain('Allowed by rosson')
    fireEvent.click(screen.getByRole('button', { name: 'Open card' }))
    expect(h.openOnAgentsPage).toHaveBeenCalledWith('p-billing')
  })

  it('re-reads when the daemon says the switch moved (another window, a terminal)', async () => {
    render(<PermissionCardsSection />)
    const sw = await screen.findByRole('switch', { name: 'Permission cards' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    daemon.permissionCards = true
    await waitFor(() => expect(h.syncHandlers.length).toBeGreaterThan(0))
    emitSync({ permissionCards: true, feature: 'permission-cards-v1', rehandshake: true })
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Permission cards' }).getAttribute('aria-checked')).toBe('true'))
    await screen.findByTestId('permission-agent-billing')
  })
})

describe('Agent tab: Can ask for permission cards', () => {
  function Tab({ handle = 'billing', id = 'p-billing' }: { handle?: string; id?: string }): React.JSX.Element {
    return (
      <PermissionAsksProvider project={{ id, handle }}>
        <div>
          Allow compute nodes
          <AskedChip row="projects.agents-can-use-compute" />
        </div>
        <AgentPermissionCardsRow project={{ id }} />
      </PermissionAsksProvider>
    )
  }
  const NAME = 'Can ask for permission cards'

  it('hidden while the server switch is off; nothing from the family is called', async () => {
    render(<Tab />)
    await waitFor(() => expect(h.get).toHaveBeenCalledWith(expect.anything(), 'settings/get'))
    await waitFor(() => expect(h.role).toHaveBeenCalled())
    expect(screen.queryByRole('switch', { name: NAME })).toBeNull()
    expect(familyCalls()).toEqual([])
  })

  it('on: reads this agent\'s switch, and one write flips it', async () => {
    daemon.permissionCards = true
    render(<Tab />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(h.get).toHaveBeenCalledWith(expect.anything(), 'permission/agents', { workspace: 'p-billing' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText('Beta')).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() =>
      expect(h.post).toHaveBeenCalledWith(expect.anything(), 'agent-access/set', { toggle: 'permission_cards', workspace: 'p-billing', value: true }),
    )
    await waitFor(() => expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true'))
  })

  it('a refusal rolls back and shows the hint', async () => {
    daemon.permissionCards = true
    h.post.mockRejectedValueOnce(new DaemonHttpError('no workspace', 404, 'workspace_not_found'))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Tab />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await screen.findByTestId('agent-permission-cards-error')
    expect(screen.getByTestId('agent-permission-cards-error').textContent).toBe('no workspace (workspace_not_found)')
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('a member never sees it, and no family route is called', async () => {
    daemon.permissionCards = true
    h.role.mockResolvedValue('member')
    render(<Tab />)
    await waitFor(() => expect(h.role).toHaveBeenCalled())
    await waitFor(() => expect(h.get).toHaveBeenCalledWith(expect.anything(), 'settings/get'))
    expect(screen.queryByRole('switch', { name: NAME })).toBeNull()
    expect(familyCalls()).toEqual([])
  })

  it('an unreadable switch stays disabled with the error, never a guessed state', async () => {
    daemon.permissionCards = true
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    h.get.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'settings/get') return { permissionCards: true }
      if (route === 'permission/pending') return { requests: [] }
      throw new DaemonHttpError('daemon /cli/permission/agents 404', 404, null)
    })
    render(<Tab />)
    await screen.findByTestId('agent-permission-cards-load-error')
    expect((screen.getByRole('switch', { name: NAME }) as HTMLButtonElement).disabled).toBe(true)
    spy.mockRestore()
  })

  it('"Asked for this" sits on the switch this agent asked for; click opens the card', async () => {
    daemon.permissionCards = true
    daemon.pending = [PENDING, { ...PENDING, requestId: 'pr_ops', agent: 'ops', addr: 'ops' }]
    render(<Tab />)
    const chip = await screen.findByTestId('permission-asked-projects.agents-can-use-compute')
    expect(chip.textContent).toBe('Asked for this')
    await screen.findByTestId('agent-permission-cards-waiting')
    expect(screen.getByTestId('agent-permission-cards-waiting').textContent).toContain('1 request from this agent is waiting on you: Allow compute nodes.')
    fireEvent.click(chip)
    expect(h.openOnAgentsPage).toHaveBeenCalledWith('p-billing')
  })

  it('no chip for another agent\'s ask', async () => {
    daemon.permissionCards = true
    daemon.pending = [{ ...PENDING, agent: 'ops', addr: 'ops' }]
    render(<Tab />)
    await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect(h.get).toHaveBeenCalledWith(expect.anything(), 'permission/pending'))
    expect(screen.queryByTestId('permission-asked-projects.agents-can-use-compute')).toBeNull()
  })

  it('the chip draws nothing outside the provider', () => {
    render(<AskedChip row="projects.agents-can-use-compute" />)
    expect(screen.queryByText('Asked for this')).toBeNull()
    expect(h.get).not.toHaveBeenCalled()
  })
})
