// @vitest-environment jsdom
//
// The one Connections picker (0.46.1, eval-connections-pickers-v1 b):
// This server, then each paired server, in the daemon's order; search over
// name, handle and server (every token); ↑/↓ + Enter; offline servers greyed
// with a Retry that asks that server only; the re-pair hint inline; agents
// already connected shown checked and not pickable; the picker stays open
// after an add and its row turns checked; errors in the footer; Esc
// closes; federation off and an older daemon (no peer-rosters) both say
// what is true. The real `lib/federation` runs against a stub
// `daemonCliGet`, so the wire shape is exercised too.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useMemo, useState } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

type Handler = (route: string, params?: Record<string, unknown>) => Promise<unknown>
const cli = vi.hoisted(() => ({
  handler: null as null | ((route: string, params?: Record<string, unknown>) => Promise<unknown>),
  calls: [] as Array<{ route: string; params?: Record<string, unknown> }>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    cli.calls.push({ route, params })
    if (route === 'projects/get-icon') return { found: false, dataUrl: null }
    if (!cli.handler) throw new Error(`no handler for ${route}`)
    return cli.handler(route, params)
  }),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  isPreFourFourFourMethod405: () => false,
}))

import { AddConnectionPicker } from './AddConnectionPicker'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
Element.prototype.scrollIntoView = vi.fn()

const SELF = 'ws-self'

function project(id: string, name: string, handle: string): Record<string, unknown> {
  return { id, name, handle, path: `/srv/${handle}`, color: '#336699', iconUrl: null, workspaces: [] }
}

const agent = (id: string, name: string, handle: string, aliases: string[] = []) => ({
  workspace_id: id,
  workspace_name: name,
  agent: handle,
  address: `${id}::${handle}`,
  aliases,
})

/** What a 0.46.1 daemon answers: already in order. */
const ROSTERS = {
  servers: [
    {
      kind: 'local',
      key: 'local',
      label: 'This server',
      host: '',
      status: 'ok',
      error: null,
      agents: [agent('ws-billing', 'Billing', 'billing'), agent('ws-cortana', 'Cortana', 'cortana'), agent(SELF, 'News desk', 'news')],
    },
    {
      kind: 'peer',
      key: 'fp-nsi',
      fingerprint: 'fp-nsi',
      label: 'NSI',
      subdomain: 'nsi',
      base_url: '',
      host: 'nsi.k2.dev',
      repair_recommended: true,
      status: 'ok',
      error: null,
      agents: [agent('n1', 'Adam', 'adam'), agent('n2', 'Jobs', 'jobs')],
    },
    {
      kind: 'peer',
      key: 'fp-old',
      fingerprint: 'fp-old',
      label: 'Old laptop',
      subdomain: '',
      base_url: 'http://192.168.1.50:38471',
      host: '192.168.1.50:38471',
      repair_recommended: false,
      status: 'offline',
      error: 'no answer within 5 s',
      agents: [],
    },
    {
      kind: 'peer',
      key: 'fp-rpm',
      fingerprint: 'fp-rpm',
      label: 'RPM',
      subdomain: 'rpm',
      base_url: '',
      host: 'rpm.k2.dev',
      repair_recommended: false,
      status: 'ok',
      error: null,
      agents: [agent('r1', 'Argus', 'argus'), agent('r2', 'Cortana', 'cortana', ['cora']), agent('r3', 'dannon', 'dannon')],
    },
  ],
  timeout_secs: 5,
}

const ok = (body: unknown): Handler => async (route) => {
  if (route === 'federation/peer-rosters') return body
  throw new DaemonHttpError('not found', 404, null)
}

function Harness(props: {
  remote?: string[]
  local?: string[]
  onAddLocal?: (id: string) => Promise<void>
  onAddRemote?: (t: string) => Promise<string | null>
  onClose?: () => void
}): React.JSX.Element {
  const [local, setLocal] = useState<string[]>(props.local ?? [])
  const [remote, setRemote] = useState<string[]>(props.remote ?? [])
  const [open, setOpen] = useState(true)
  const localSet = useMemo(() => new Set(local), [local])
  const remoteSet = useMemo(() => new Set(remote), [remote])
  if (!open) return <p>closed</p>
  return (
    <AddConnectionPicker
      projectId={SELF}
      connectedLocalIds={localSet}
      connectedRemoteAddrs={remoteSet}
      onAddLocal={async (id) => {
        await (props.onAddLocal ?? (async () => undefined))(id)
        setLocal((l) => [...l, id])
      }}
      onAddRemote={async (t) => {
        const w = await (props.onAddRemote ?? (async () => null))(t)
        setRemote((r) => [...r, t])
        return w
      }}
      onClose={() => {
        props.onClose?.()
        setOpen(false)
      }}
    />
  )
}

const search = (): HTMLInputElement => screen.getByRole('combobox') as HTMLInputElement
const sectionEl = (key: string): HTMLElement => {
  const el = document.querySelector(`[data-agent-list-section="${key}"]`)
  if (!el) throw new Error(`no section ${key}`)
  return el as HTMLElement
}
const sectionKeys = (): string[] =>
  [...document.querySelectorAll('[data-agent-list-section]')].map((e) => e.getAttribute('data-agent-list-section') ?? '')
const rowLabels = (key: string): string[] =>
  [...sectionEl(key).querySelectorAll('[role="option"]')].map((b) => b.querySelector('span.text-xs')?.textContent ?? '')
const option = (key: string, label: string): HTMLElement => {
  const hit = [...sectionEl(key).querySelectorAll('[role="option"]')].find(
    (b) => b.querySelector('span.text-xs')?.textContent === label,
  )
  if (!hit) throw new Error(`no row ${label} in ${key}`)
  return hit as HTMLElement
}

beforeEach(() => {
  cli.calls = []
  cli.handler = ok(ROSTERS)
  useProjectsStore.setState({
    projects: [
      project('ws-cortana', 'Cortana', 'cortana'),
      project('ws-billing', 'Billing', 'billing'),
      project(SELF, 'News desk', 'news'),
    ] as never,
  })
})

afterEach(() => cleanup())

describe('AddConnectionPicker: sections in the daemon order', () => {
  it('shows This server first, then each paired server, rows A to Z, without the workspace itself', async () => {
    render(<Harness />)
    await screen.findByText('RPM')
    expect(sectionKeys()).toEqual(['local', 'peer:fp-nsi', 'peer:fp-old', 'peer:fp-rpm'])
    expect(rowLabels('local')).toEqual(['Billing', 'Cortana'])
    expect(rowLabels('peer:fp-rpm')).toEqual(['Argus', 'Cortana', 'dannon'])
    // The handle is the row's detail; the host sits beside the server name.
    expect(within(option('peer:fp-rpm', 'Argus')).getByText('argus')).toBeTruthy()
    expect(within(sectionEl('peer:fp-rpm')).getByText('· rpm.k2.dev')).toBeTruthy()
    // One request paints it all.
    expect(cli.calls.filter((c) => c.route.startsWith('federation/')).map((c) => c.route)).toEqual(['federation/peer-rosters'])
  })

  it('greys an offline server with a Retry that asks that server only', async () => {
    render(<Harness />)
    await screen.findByText('RPM')
    const old = sectionEl('peer:fp-old')
    expect(old.hasAttribute('data-dim')).toBe(true)
    const status = old.querySelector('[data-listing="offline"]') as HTMLElement
    expect(status.textContent).toContain('Offline: no answer within 5 s.')

    cli.handler = async (route, params) => {
      expect(route).toBe('federation/peer-rosters')
      expect(params).toEqual({ peer: 'fp-old' })
      return { servers: [{ ...ROSTERS.servers[2], status: 'ok', error: null, agents: [agent('o1', 'Laptop agent', 'lappy')] }] }
    }
    fireEvent.click(within(status).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(rowLabels('peer:fp-old')).toEqual(['Laptop agent']))
    expect(sectionEl('peer:fp-old').hasAttribute('data-dim')).toBe(false)
    expect(sectionKeys()).toEqual(['local', 'peer:fp-nsi', 'peer:fp-old', 'peer:fp-rpm'])
  })

  it('shows the re-pair hint inline, and Re-pair opens Connected Servers', async () => {
    render(<Harness />)
    await screen.findByText('NSI')
    const hint = sectionEl('peer:fp-nsi').querySelector('[data-repair-hint]') as HTMLElement
    expect(hint.textContent).toContain('older pairing')
    expect(hint.getAttribute('title')).toContain('Paired with the older pairing code')
    expect(sectionEl('peer:fp-rpm').querySelector('[data-repair-hint]')).toBeNull()
    fireEvent.click(within(hint).getByRole('button', { name: 'Re-pair' }))
    expect(useSettingsStore.getState().activeSection).toBe('connections')
  })

  it('says a refusing server refused, and an empty roster is a setting on their side', async () => {
    cli.handler = ok({
      servers: [
        ROSTERS.servers[0],
        { ...ROSTERS.servers[1], status: 'rejected', error: 'it refused this server (HTTP 403)', agents: [] },
        { ...ROSTERS.servers[3], agents: [] },
      ],
    })
    render(<Harness />)
    await screen.findByText('RPM')
    expect(sectionEl('peer:fp-nsi').querySelector('[data-listing="rejected"]')?.textContent).toContain('NSI refused this server')
    expect(sectionEl('peer:fp-nsi').hasAttribute('data-dim')).toBe(true)
    expect(sectionEl('peer:fp-rpm').querySelector('[data-listing="empty"]')?.textContent).toContain('No connectable agents on this server')
  })
})

describe('AddConnectionPicker: search and keyboard', () => {
  it('matches every token across name, handle, alias and server', async () => {
    render(<Harness />)
    await screen.findByText('RPM')
    fireEvent.change(search(), { target: { value: 'rpm cort' } })
    expect(sectionKeys()).toEqual(['peer:fp-rpm'])
    expect(rowLabels('peer:fp-rpm')).toEqual(['Cortana'])

    fireEvent.change(search(), { target: { value: 'cora' } })
    expect(rowLabels('peer:fp-rpm')).toEqual(['Cortana'])

    fireEvent.change(search(), { target: { value: 'nsi.k2' } })
    expect(sectionKeys()).toEqual(['peer:fp-nsi'])
    expect(rowLabels('peer:fp-nsi')).toEqual(['Adam', 'Jobs'])

    // Naming the offline server keeps its status (and Retry) on screen.
    fireEvent.change(search(), { target: { value: 'old laptop' } })
    expect(sectionKeys()).toEqual(['peer:fp-old'])

    fireEvent.change(search(), { target: { value: 'zzz' } })
    expect(screen.getByText('No agents match')).toBeTruthy()
  })

  it('↑/↓ walk the pickable rows across sections, skipping connected ones, and Enter connects', async () => {
    const onAddRemote = vi.fn(async () => null)
    const onAddLocal = vi.fn(async () => undefined)
    render(<Harness local={['ws-billing']} onAddLocal={onAddLocal} onAddRemote={onAddRemote} />)
    await screen.findByText('RPM')
    const box = search()
    // Billing is connected: the first pickable row is Cortana (This server).
    fireEvent.keyDown(box, { key: 'ArrowDown' })
    expect(box.getAttribute('aria-activedescendant')).toBe(option('local', 'Cortana').id)
    fireEvent.keyDown(box, { key: 'ArrowDown' })
    expect(box.getAttribute('aria-activedescendant')).toBe(option('peer:fp-nsi', 'Adam').id)
    fireEvent.keyDown(box, { key: 'ArrowUp' })
    fireEvent.keyDown(box, { key: 'Enter' })
    await waitFor(() => expect(onAddLocal).toHaveBeenCalledWith('ws-cortana'))
    expect(onAddRemote).not.toHaveBeenCalled()

    fireEvent.change(box, { target: { value: 'jobs' } })
    fireEvent.keyDown(box, { key: 'ArrowDown' })
    fireEvent.keyDown(box, { key: 'Enter' })
    await waitFor(() => expect(onAddRemote).toHaveBeenCalledWith('jobs::nsi.k2.dev'))
  })

  it('Esc closes the picker', async () => {
    const onClose = vi.fn()
    render(<Harness onClose={onClose} />)
    await screen.findByText('RPM')
    fireEvent.keyDown(search(), { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(screen.getByText('closed')).toBeTruthy()
  })
})

describe('AddConnectionPicker: connected agents and adding', () => {
  it('shows agents already connected checked, and clicking one does nothing', async () => {
    const onAddRemote = vi.fn(async () => null)
    render(<Harness local={['ws-cortana']} remote={['argus::rpm.k2.dev']} onAddRemote={onAddRemote} />)
    await screen.findByText('RPM')
    const argus = option('peer:fp-rpm', 'Argus')
    expect(argus.getAttribute('data-row-state')).toBe('checked')
    expect(argus.getAttribute('title')).toBe('Already connected to argus::rpm.k2.dev')
    expect(option('local', 'Cortana').getAttribute('data-row-state')).toBe('checked')
    expect(option('peer:fp-rpm', 'Cortana').getAttribute('data-row-state')).toBe('pickable')
    fireEvent.click(argus)
    expect(onAddRemote).not.toHaveBeenCalled()
  })

  it('stays open after each add; the row turns checked', async () => {
    const onAddRemote = vi.fn(async () => null)
    render(<Harness onAddRemote={onAddRemote} />)
    await screen.findByText('RPM')
    fireEvent.click(option('peer:fp-rpm', 'dannon'))
    await waitFor(() => expect(option('peer:fp-rpm', 'dannon').getAttribute('data-row-state')).toBe('checked'))
    expect(onAddRemote).toHaveBeenCalledWith('dannon::rpm.k2.dev')
    fireEvent.click(option('peer:fp-nsi', 'Jobs'))
    await waitFor(() => expect(option('peer:fp-nsi', 'Jobs').getAttribute('data-row-state')).toBe('checked'))
    expect(screen.getByRole('dialog', { name: 'Add connection' })).toBeTruthy()
    // The rosters were fetched once: adding never re-dials every server.
    expect(cli.calls.filter((c) => c.route === 'federation/peer-rosters')).toHaveLength(1)
  })

  it('shows an add error in the footer and keeps the picker open', async () => {
    render(
      <Harness
        onAddRemote={async () => {
          throw new Error('peer is not trusted')
        }}
      />,
    )
    await screen.findByText('RPM')
    fireEvent.click(option('peer:fp-rpm', 'Argus'))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('Couldn’t connect to Argus: peer is not trusted')
    expect(option('peer:fp-rpm', 'Argus').getAttribute('data-row-state')).toBe('pickable')
  })
})

describe('AddConnectionPicker: honest states', () => {
  it('paints This server at once while paired servers answer, with no blank spinner', async () => {
    let release: (v: unknown) => void = () => undefined
    cli.handler = () => new Promise((r) => (release = r))
    render(<Harness />)
    expect(rowLabels('local')).toEqual(['Billing', 'Cortana'])
    expect(sectionEl('peers-pending').textContent).toContain('Asking paired servers… (5 s at most each)')
    await act(async () => release(ROSTERS))
    await screen.findByText('RPM')
    expect(sectionKeys()).not.toContain('peers-pending')
  })

  it('federation off: This server still lists, and the note says how to turn it on', async () => {
    cli.handler = async () => {
      throw new DaemonHttpError('not found', 404, null)
    }
    render(<Harness />)
    await screen.findByText(/Federation is off on this server/)
    expect(rowLabels('local')).toEqual(['Billing', 'Cortana'])
    expect(cli.calls.map((c) => c.route)).toEqual(['federation/peer-rosters', 'federation/peers'])
  })

  it('a Member login: says it can’t list paired servers', async () => {
    cli.handler = async () => {
      throw new DaemonHttpError('forbidden', 403, null)
    }
    render(<Harness />)
    expect(await screen.findByText(/Your login can’t list paired servers’ agents/)).toBeTruthy()
  })

  it('an older daemon: asks each paired server, sorts A to Z, and a 502 is offline', async () => {
    cli.handler = async (route, params) => {
      if (route === 'federation/peer-rosters') throw new DaemonHttpError('not found', 404, null)
      if (route === 'federation/peers') {
        return {
          peers: [
            { fingerprint: 'fp-z', label: 'zulu', subdomain: 'zulu', trust: 'trusted', capabilities: [] },
            { fingerprint: 'fp-p', label: 'pending', subdomain: 'pending', trust: 'pending', capabilities: [] },
            { fingerprint: 'fp-a', label: 'Alpha', subdomain: 'alpha', trust: 'trusted', capabilities: [] },
          ],
        }
      }
      if (route === 'federation/peer-roster' && params?.peer === 'fp-a') {
        return { peer: 'fp-a', roster: { agents: [agent('a2', 'sales', 'sales'), agent('a1', 'Accounts', 'accounts')] } }
      }
      throw new DaemonHttpError('fetch peer roster: connection refused', 502, null)
    }
    render(<Harness />)
    await screen.findByText('zulu')
    expect(sectionKeys()).toEqual(['local', 'peer:fp-a', 'peer:fp-z'])
    expect(rowLabels('peer:fp-a')).toEqual(['Accounts', 'sales'])
    expect(sectionEl('peer:fp-z').querySelector('[data-listing="offline"]')).toBeTruthy()
    expect(sectionEl('peer:fp-z').hasAttribute('data-dim')).toBe(true)
  })

  it('a failed list says so with a Retry that asks again', async () => {
    cli.handler = async () => {
      throw new DaemonHttpError('daemon exploded', 500, null)
    }
    render(<Harness />)
    const status = await screen.findByText('Couldn’t list paired servers: daemon exploded')
    cli.handler = ok(ROSTERS)
    fireEvent.click(within(status.parentElement as HTMLElement).getByRole('button', { name: 'Retry' }))
    await screen.findByText('RPM')
  })
})
